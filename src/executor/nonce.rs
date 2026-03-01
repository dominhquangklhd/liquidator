// Nonce Manager
//
// Thread-safe nonce management for parallel transaction submission

use ethers::providers::{Provider, Http, Middleware};
use ethers::types::{Address, U256};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;
use std::collections::HashMap;
use anyhow::Result;

/// Transaction status for pending nonces
#[derive(Debug, Clone, PartialEq)]
pub enum TxStatus {
    Pending,
    Confirmed,
    Failed,
    Timeout,
}

/// Manages nonces for transaction submission
/// 
/// Handles parallel transaction submission without nonce conflicts
pub struct NonceManager {
    /// Current nonce (atomically managed)
    current_nonce: AtomicU64,
    
    /// Pending transactions by nonce
    pending: Arc<Mutex<HashMap<u64, TxStatus>>>,
    
    /// Provider for on-chain nonce queries
    provider: Arc<Provider<Http>>,
    
    /// Our wallet address
    address: Address,
    
    /// Last synced block
    last_sync_block: AtomicU64,
}

impl NonceManager {
    /// Create new nonce manager
    pub async fn new(provider: Arc<Provider<Http>>, address: Address) -> Result<Self> {
        let onchain_nonce = provider
            .get_transaction_count(address, None)
            .await?
            .as_u64();
        
        tracing::info!("NonceManager initialized with nonce: {}", onchain_nonce);
        
        Ok(Self {
            current_nonce: AtomicU64::new(onchain_nonce),
            pending: Arc::new(Mutex::new(HashMap::new())),
            provider,
            address,
            last_sync_block: AtomicU64::new(0),
        })
    }
    
    /// Get next nonce for transaction
    pub async fn get_next(&self) -> u64 {
        let nonce = self.current_nonce.fetch_add(1, Ordering::SeqCst);
        
        // Mark as pending
        let mut pending = self.pending.lock().await;
        pending.insert(nonce, TxStatus::Pending);
        
        tracing::debug!("Allocated nonce: {}", nonce);
        nonce
    }
    
    /// Get current nonce without incrementing
    pub fn peek(&self) -> u64 {
        self.current_nonce.load(Ordering::SeqCst)
    }
    
    /// Mark transaction as confirmed
    pub async fn confirm(&self, nonce: u64) {
        let mut pending = self.pending.lock().await;
        pending.insert(nonce, TxStatus::Confirmed);
        tracing::debug!("Nonce {} confirmed", nonce);
    }
    
    /// Mark transaction as failed (can be reused)
    pub async fn fail(&self, nonce: u64) {
        let mut pending = self.pending.lock().await;
        pending.insert(nonce, TxStatus::Failed);
        tracing::debug!("Nonce {} failed", nonce);
    }
    
    /// Release a nonce (transaction was cancelled before sending)
    pub async fn release(&self, nonce: u64) {
        let mut pending = self.pending.lock().await;
        pending.remove(&nonce);
        
        // If this was the highest nonce, we can rewind
        let current = self.current_nonce.load(Ordering::SeqCst);
        if nonce == current - 1 {
            self.current_nonce.compare_exchange(
                current,
                nonce,
                Ordering::SeqCst,
                Ordering::SeqCst
            ).ok();
        }
        
        tracing::debug!("Nonce {} released", nonce);
    }
    
    /// Sync with on-chain state
    pub async fn sync(&self) -> Result<()> {
        let onchain_nonce = self.provider
            .get_transaction_count(self.address, None)
            .await?
            .as_u64();
        
        let local_nonce = self.current_nonce.load(Ordering::SeqCst);
        
        if onchain_nonce > local_nonce {
            // On-chain is ahead - some transactions confirmed externally
            self.current_nonce.store(onchain_nonce, Ordering::SeqCst);
            tracing::info!("Nonce synced: {} -> {}", local_nonce, onchain_nonce);
        } else if onchain_nonce < local_nonce {
            // We have pending transactions
            let pending_count = local_nonce - onchain_nonce;
            tracing::debug!("Pending transactions: {}", pending_count);
        }
        
        // Cleanup old confirmed/failed entries
        let mut pending = self.pending.lock().await;
        pending.retain(|&nonce, status| {
            nonce >= onchain_nonce && *status == TxStatus::Pending
        });
        
        Ok(())
    }
    
    /// Get count of pending transactions
    pub async fn pending_count(&self) -> usize {
        let pending = self.pending.lock().await;
        pending.values().filter(|s| **s == TxStatus::Pending).count()
    }
    
    /// Check if we have too many pending transactions
    pub async fn is_congested(&self, max_pending: usize) -> bool {
        self.pending_count().await >= max_pending
    }
    
    /// Reset nonce to on-chain value (use when stuck)
    pub async fn reset(&self) -> Result<()> {
        let onchain_nonce = self.provider
            .get_transaction_count(self.address, None)
            .await?
            .as_u64();
        
        self.current_nonce.store(onchain_nonce, Ordering::SeqCst);
        
        let mut pending = self.pending.lock().await;
        pending.clear();
        
        tracing::warn!("Nonce reset to: {}", onchain_nonce);
        Ok(())
    }
}
