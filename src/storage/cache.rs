// Hot Cache Layer - In-Memory Storage for Top N Targets
//
// Uses BTreeMap sorted by health factor for O(log n) insert/remove
// and O(1) access to lowest HF targets.

use super::models::LiquidationTarget;
use std::collections::{BTreeMap, HashMap};
use ordered_float::OrderedFloat;

/// Hot cache for liquidation targets
/// 
/// Maintains top N targets with lowest health factors in memory
/// for ultra-fast access (< 1ms).
pub struct HotCache {
    /// Targets sorted by health factor (lowest first)
    /// Key: (health_factor, user_address) for stable sorting
    targets: BTreeMap<(OrderedFloat<f64>, String), LiquidationTarget>,
    
    /// Index: user_address -> health_factor (for fast lookup)
    user_index: HashMap<String, OrderedFloat<f64>>,
    
    /// Maximum cache size
    max_size: usize,
    
    /// Health factor threshold for entry
    threshold: f64,
}

impl HotCache {
    /// Create new hot cache
    pub fn new(max_size: usize, threshold: f64) -> Self {
        Self {
            targets: BTreeMap::new(),
            user_index: HashMap::new(),
            max_size,
            threshold,
        }
    }
    
    /// Insert or update a target
    pub fn insert(&mut self, target: LiquidationTarget) {
        // Only cache if below threshold
        if target.health_factor >= self.threshold {
            // Remove if exists (user improved their HF)
            self.remove(&target.user_address);
            return;
        }
        
        // Remove old entry if exists (HF might have changed)
        if let Some(old_hf) = self.user_index.get(&target.user_address) {
            self.targets.remove(&(*old_hf, target.user_address.clone()));
        }
        
        // Insert new entry
        let hf = OrderedFloat(target.health_factor);
        let user = target.user_address.clone();
        
        self.targets.insert((hf, user.clone()), target);
        self.user_index.insert(user, hf);
        
        // Evict if over capacity
        self.evict_if_needed();
    }
    
    /// Remove a target
    pub fn remove(&mut self, user_address: &str) -> Option<LiquidationTarget> {
        if let Some(hf) = self.user_index.remove(user_address) {
            self.targets.remove(&(hf, user_address.to_string()))
        } else {
            None
        }
    }
    
    /// Get a specific target
    pub fn get(&self, user_address: &str) -> Option<&LiquidationTarget> {
        let hf = self.user_index.get(user_address)?;
        self.targets.get(&(*hf, user_address.to_string()))
    }
    
    /// Check if target exists in cache
    pub fn contains(&self, user_address: &str) -> bool {
        self.user_index.contains_key(user_address)
    }
    
    /// Get top N targets (lowest health factors)
    pub fn get_top(&self, n: usize) -> Vec<LiquidationTarget> {
        self.targets
            .iter()
            .take(n)
            .map(|(_, target)| target.clone())
            .collect()
    }
    
    /// Get all targets
    pub fn get_all(&self) -> Vec<LiquidationTarget> {
        self.targets.values().cloned().collect()
    }
    
    /// Get cache size
    pub fn len(&self) -> usize {
        self.targets.len()
    }
    
    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
    
    /// Evict least risky targets if over capacity
    fn evict_if_needed(&mut self) {
        while self.targets.len() > self.max_size {
            // Remove target with highest HF (least risky)
            if let Some(((hf, user), _)) = self.targets.pop_last() {
                self.user_index.remove(&user);
                tracing::debug!("Evicted {} (HF: {}) from hot cache", user, hf);
            }
        }
    }
    
    /// Clear all entries
    pub fn clear(&mut self) {
        self.targets.clear();
        self.user_index.clear();
    }
    
    /// Get statistics
    pub fn stats(&self) -> CacheStats {
        let avg_hf = if self.targets.is_empty() {
            0.0
        } else {
            self.targets.values().map(|t| t.health_factor).sum::<f64>() 
                / self.targets.len() as f64
        };
        
        let lowest_hf = self.targets.first_key_value()
            .map(|((hf, _), _)| hf.0)
            .unwrap_or(0.0);
        
        let highest_hf = self.targets.last_key_value()
            .map(|((hf, _), _)| hf.0)
            .unwrap_or(0.0);
        
        CacheStats {
            size: self.targets.len(),
            max_size: self.max_size,
            avg_health_factor: avg_hf,
            lowest_health_factor: lowest_hf,
            highest_health_factor: highest_hf,
        }
    }
}

#[derive(Debug)]
pub struct CacheStats {
    pub size: usize,
    pub max_size: usize,
    pub avg_health_factor: f64,
    pub lowest_health_factor: f64,
    pub highest_health_factor: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = HotCache::new(3, 1.2);
        
        let mut target = LiquidationTarget::new("user1".to_string());
        target.health_factor = 1.05;
        cache.insert(target);
        
        assert_eq!(cache.len(), 1);
        assert!(cache.contains("user1"));
        
        let retrieved = cache.get("user1").unwrap();
        assert_eq!(retrieved.health_factor, 1.05);
    }
    
    #[test]
    fn test_cache_sorting() {
        let mut cache = HotCache::new(10, 1.5);
        
        let mut t1 = LiquidationTarget::new("user1".to_string());
        t1.health_factor = 1.1;
        cache.insert(t1);
        
        let mut t2 = LiquidationTarget::new("user2".to_string());
        t2.health_factor = 0.95;
        cache.insert(t2);
        
        let mut t3 = LiquidationTarget::new("user3".to_string());
        t3.health_factor = 1.2;
        cache.insert(t3);
        
        let top = cache.get_top(2);
        assert_eq!(top[0].user_address, "user2"); // Lowest HF first
        assert_eq!(top[1].user_address, "user1");
    }
    
    #[test]
    fn test_cache_eviction() {
        let mut cache = HotCache::new(2, 2.0);
        
        let mut t1 = LiquidationTarget::new("user1".to_string());
        t1.health_factor = 1.0;
        cache.insert(t1);
        
        let mut t2 = LiquidationTarget::new("user2".to_string());
        t2.health_factor = 1.1;
        cache.insert(t2);
        
        let mut t3 = LiquidationTarget::new("user3".to_string());
        t3.health_factor = 0.95;
        cache.insert(t3);
        
        // Cache size should be 2 (evicted highest HF)
        assert_eq!(cache.len(), 2);
        assert!(cache.contains("user1"));
        assert!(cache.contains("user3"));
        assert!(!cache.contains("user2")); // Evicted
    }
    
    #[test]
    fn test_threshold_filtering() {
        let mut cache = HotCache::new(10, 1.2);
        
        let mut t1 = LiquidationTarget::new("user1".to_string());
        t1.health_factor = 1.1; // Below threshold
        cache.insert(t1);
        
        let mut t2 = LiquidationTarget::new("user2".to_string());
        t2.health_factor = 1.5; // Above threshold
        cache.insert(t2);
        
        assert_eq!(cache.len(), 1);
        assert!(cache.contains("user1"));
        assert!(!cache.contains("user2"));
    }
}
