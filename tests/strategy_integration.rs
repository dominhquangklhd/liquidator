// Strategy Integration Tests
//
// Test tích hợp giữa Strategy Decider với các module khác:
// - I1: Profit → Strategy pipeline
// - I2: Strategy decision consistency (Direct vs Skip)
// - I3: Multi-target batch ordering (5+ targets)
// - I4: Circuit breaker → plan rejection
// - I5: Wallet state changes → method re-evaluation
// - I6: Full pipeline: targets → profit → strategy → plan

use liquidator::strategy::{StrategyDecider, StrategyConfig, ExecutionMethod};
use liquidator::profit::{ProfitEstimate, ProfitBreakdown, LiquidationPair};
use liquidator::storage::LiquidationTarget;

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Tạo ProfitEstimate mẫu
fn mock_estimate(user: &str, profit: f64, debt: f64, debt_asset: &str) -> ProfitEstimate {
    ProfitEstimate {
        user_address: user.to_string(),
        pair: LiquidationPair {
            collateral_asset: "ETH".to_string(),
            debt_asset: debt_asset.to_string(),
            bonus_pct: 5.0,
            collateral_price_usd: 2000.0,
            debt_price_usd: 1.0,
            collateral_amount: 10.0,
            debt_amount: debt,
            score: profit,
        },
        debt_to_cover_usd: debt * 0.5,
        collateral_received_usd: debt * 0.5 * 1.05,
        gross_profit_usd: profit + 30.0,
        gas_cost_usd: 30.0,
        slippage_cost_usd: 0.0,
        flash_loan_fee_usd: 0.0,
        net_profit_usd: profit,
        roi_pct: if profit > 0.0 { profit / 30.0 * 100.0 } else { 0.0 },
        is_profitable: profit > 0.0,
        reject_reason: if profit <= 0.0 { Some("Not profitable".to_string()) } else { None },
        breakdown: ProfitBreakdown::default(),
    }
}

/// Tạo LiquidationTarget mẫu
fn mock_target(user: &str, hf: f64, debt_usd: f64) -> LiquidationTarget {
    let mut target = LiquidationTarget::new(user.to_string());
    target.health_factor = hf;
    target.total_debt_usd = debt_usd;
    target.total_collateral_usd = debt_usd * 1.5;
    target.collateral.insert("ETH".to_string(), debt_usd * 1.5 / 2000.0);
    target.debt.insert("USDC".to_string(), debt_usd);
    target
}

// ============================================================================
// I1: Profit → Strategy Pipeline
// ProfitCalculator tạo estimates → StrategyDecider nhận → tạo ExecutionPlan
// ============================================================================

#[tokio::test]
async fn test_i1_profit_estimates_to_strategy_plan() {
    // Giả lập output từ ProfitCalculator (profitable estimates)
    let estimates = vec![
        ("0xuser_high", 800.0, 20_000.0),  // Profit cao
        ("0xuser_mid", 300.0, 10_000.0),   // Profit trung bình
        ("0xuser_low", 50.0, 5_000.0),     // Profit thấp
        ("0xuser_loss", -20.0, 8_000.0),   // Không profitable
    ];
    
    let targets_with_estimates: Vec<(LiquidationTarget, ProfitEstimate)> = estimates
        .iter()
        .map(|(user, profit, debt)| {
            (mock_target(user, 0.90, *debt), mock_estimate(user, *profit, *debt, "USDC"))
        })
        .collect();
    
    // Strategy Decider với config chuẩn
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 100_000.0).await;
    
    let plan = decider.create_plan(targets_with_estimates).await.unwrap();
    
    // ── Verify pipeline output ──
    assert_eq!(plan.total_input, 4, "Tổng input = 4 targets");
    assert_eq!(plan.skip_count, 1, "1 target unprofitable → skip");
    assert_eq!(plan.execute_count, 3, "3 targets profitable → execute");
    
    // Targets profitable phải có method Direct (vì có đủ token)
    let executable: Vec<_> = plan.targets.iter()
        .filter(|pt| pt.decision.should_execute())
        .collect();
    assert_eq!(executable.len(), 3);
    
    for pt in &executable {
        assert!(matches!(pt.decision.method, ExecutionMethod::Direct { .. }),
            "User {} should use Direct (enough USDC)", pt.decision.user_address);
        assert!(pt.decision.adjusted_profit_usd > 0.0,
            "User {} profit should be positive", pt.decision.user_address);
    }
    
    // Target loss phải bị skip
    let skipped: Vec<_> = plan.targets.iter()
        .filter(|pt| !pt.decision.should_execute())
        .collect();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].decision.user_address, "0xuser_loss");
}

#[tokio::test]
async fn test_i1_empty_profit_results_empty_plan() {
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);
    
    // ProfitCalculator trả về 0 profitable targets
    let plan = decider.create_plan(vec![]).await.unwrap();
    
    assert_eq!(plan.total_input, 0);
    assert_eq!(plan.execute_count, 0);
    assert!(plan.targets.is_empty());
}

// ============================================================================
// I2: Strategy Decision Consistency — Direct vs Skip
// Verify method selection thay đổi đúng khi wallet state thay đổi
// ============================================================================

#[tokio::test]
async fn test_i2_method_changes_with_wallet_state() {
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);
    
    let target = mock_target("0xuser", 0.88, 20_000.0);
    let estimate = mock_estimate("0xuser", 500.0, 20_000.0, "USDC");
    
    // ── Trạng thái 1: Không có USDC → Skip ──
    decider.update_wallet_balance(5.0).await;
    let d1 = decider.decide_single(&target, &estimate).await;
    assert!(matches!(d1.method, ExecutionMethod::Skip { .. }),
        "No USDC should trigger Skip");
    let skip_profit = d1.adjusted_profit_usd;
    
    // ── Trạng thái 2: Thêm đủ USDC → Direct ──
    decider.update_token_balance("USDC".to_string(), 20_000.0).await;
    let d2 = decider.decide_single(&target, &estimate).await;
    assert!(matches!(d2.method, ExecutionMethod::Direct { .. }),
        "Enough USDC should trigger Direct");
    let direct_profit = d2.adjusted_profit_usd;
    
    // Direct profit > Skip profit (skip = 0)
    assert!(direct_profit > skip_profit,
        "Direct profit ${:.2} should > Skip profit ${:.2}",
        direct_profit, skip_profit);
    
    // ── Trạng thái 3: ETH quá thấp → Skip ──
    decider.update_wallet_balance(0.001).await;
    let d3 = decider.decide_single(&target, &estimate).await;
    assert!(matches!(d3.method, ExecutionMethod::Skip { .. }),
        "Low ETH should Skip");
}

#[tokio::test]
async fn test_i2_debt_too_large_skips() {
    let mut config = StrategyConfig::local_fork();
    config.direct_max_debt_usd = 10_000.0;
    let decider = StrategyDecider::new(config);
    
    decider.update_wallet_balance(5.0).await;
    decider.update_token_balance("USDC".to_string(), 200_000.0).await;
    
    // debt_to_cover = 100_000 * 0.5 = $50k > direct_max_debt_usd($10k) → skip
    let target = mock_target("0xuser", 0.90, 100_000.0);
    let estimate = mock_estimate("0xuser", 200.0, 100_000.0, "USDC");
    
    let decision = decider.decide_single(&target, &estimate).await;
    assert!(matches!(decision.method, ExecutionMethod::Skip { .. }),
        "Debt above direct max should Skip");
}

// ============================================================================
// I3: Multi-target Batch Ordering (5+ targets)
// Verify sorting algorithm với nhiều targets, đa dạng HF/profit/debt
// ============================================================================

#[tokio::test]
async fn test_i3_multi_target_batch_ordering_5_targets() {
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 200_000.0).await;
    
    // 5 targets với đặc điểm khác nhau
    let inputs = vec![
        // User A: HF rất thấp (urgent), profit trung bình
        (mock_target("0xA", 0.70, 15_000.0), mock_estimate("0xA", 400.0, 15_000.0, "USDC")),
        // User B: HF cao (ít urgent), profit rất cao
        (mock_target("0xB", 0.98, 30_000.0), mock_estimate("0xB", 1200.0, 30_000.0, "USDC")),
        // User C: HF thấp, profit thấp
        (mock_target("0xC", 0.75, 4_000.0), mock_estimate("0xC", 80.0, 4_000.0, "USDC")),
        // User D: HF trung bình, profit trung bình
        (mock_target("0xD", 0.88, 12_000.0), mock_estimate("0xD", 350.0, 12_000.0, "USDC")),
        // User E: Unprofitable
        (mock_target("0xE", 0.92, 6_000.0), mock_estimate("0xE", -10.0, 6_000.0, "USDC")),
    ];
    
    let plan = decider.create_plan(inputs).await.unwrap();
    
    assert_eq!(plan.total_input, 5);
    assert_eq!(plan.execute_count, 4, "4 profitable execute");
    assert_eq!(plan.skip_count, 1, "1 unprofitable skip");
    
    // Verify sorted by priority DESC
    let executable: Vec<_> = plan.targets.iter()
        .filter(|pt| pt.decision.should_execute())
        .collect();
    
    for i in 0..executable.len() - 1 {
        assert!(
            executable[i].decision.priority_score >= executable[i + 1].decision.priority_score,
            "Target #{} (score {:.2}) should >= #{} (score {:.2})",
            executable[i].rank, executable[i].decision.priority_score,
            executable[i + 1].rank, executable[i + 1].decision.priority_score,
        );
    }
    
    // User B (profit $1200, cao nhất) hoặc User A (HF 0.70, urgent nhất) 
    // nên ở top — phụ thuộc weights
    let top_user = &executable[0].decision.user_address;
    assert!(
        top_user == "0xB" || top_user == "0xA",
        "Top priority should be 0xB (highest profit) or 0xA (most urgent HF), got {}",
        top_user
    );
    
    // User E phải bị skip
    let skipped: Vec<_> = plan.targets.iter()
        .filter(|pt| !pt.decision.should_execute())
        .collect();
    assert!(skipped.iter().any(|pt| pt.decision.user_address == "0xE"));
}

#[tokio::test]
async fn test_i3_batch_ordering_with_concurrent_limit() {
    let mut config = StrategyConfig::local_fork();
    config.max_concurrent_liquidations = 3; // Chỉ top 3
    let decider = StrategyDecider::new(config);
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 200_000.0).await;
    
    // 6 profitable targets
    let inputs = vec![
        (mock_target("0x1", 0.85, 20_000.0), mock_estimate("0x1", 800.0, 20_000.0, "USDC")),
        (mock_target("0x2", 0.88, 15_000.0), mock_estimate("0x2", 600.0, 15_000.0, "USDC")),
        (mock_target("0x3", 0.90, 12_000.0), mock_estimate("0x3", 400.0, 12_000.0, "USDC")),
        (mock_target("0x4", 0.92, 10_000.0), mock_estimate("0x4", 300.0, 10_000.0, "USDC")),
        (mock_target("0x5", 0.94, 8_000.0), mock_estimate("0x5", 200.0, 8_000.0, "USDC")),
        (mock_target("0x6", 0.96, 5_000.0), mock_estimate("0x6", 100.0, 5_000.0, "USDC")),
    ];
    
    let plan = decider.create_plan(inputs).await.unwrap();
    
    assert_eq!(plan.total_input, 6);
    assert_eq!(plan.execute_count, 3, "Only top 3 should execute");
    assert_eq!(plan.skip_count, 3, "Bottom 3 skipped due to concurrent limit");
    
    // Top 3 phải là những target có priority cao nhất
    let executable: Vec<_> = plan.targets.iter()
        .filter(|pt| pt.decision.should_execute())
        .collect();
    assert_eq!(executable.len(), 3);
    
    // Verify ranks
    for pt in &executable {
        assert!(pt.rank <= 3, "Executable target rank {} should <= 3", pt.rank);
    }
}

// ============================================================================
// I4: Circuit Breaker → Plan Rejection
// Nhiều failures → circuit breaker trip → plan rỗng → recovery
// ============================================================================

#[tokio::test]
async fn test_i4_circuit_breaker_blocks_entire_plan() {
    let mut config = StrategyConfig::local_fork();
    config.circuit_breaker_threshold = 3;
    config.circuit_breaker_cooldown_secs = 1;
    let decider = StrategyDecider::new(config);
    
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 100_000.0).await;
    
    let make_inputs = || vec![
        (mock_target("0x1", 0.85, 20_000.0), mock_estimate("0x1", 800.0, 20_000.0, "USDC")),
        (mock_target("0x2", 0.90, 10_000.0), mock_estimate("0x2", 400.0, 10_000.0, "USDC")),
        (mock_target("0x3", 0.92, 8_000.0), mock_estimate("0x3", 200.0, 8_000.0, "USDC")),
    ];
    
    // ── Phase 1: Hoạt động bình thường ──
    let plan1 = decider.create_plan(make_inputs()).await.unwrap();
    assert_eq!(plan1.execute_count, 3, "All should execute normally");
    
    // ── Phase 2: Trip circuit breaker (3 failures) ──
    decider.report_failure().await;
    decider.report_failure().await;
    decider.report_failure().await;
    
    let plan2 = decider.create_plan(make_inputs()).await.unwrap();
    assert_eq!(plan2.execute_count, 0, "All should be blocked by circuit breaker");
    
    let stats = decider.get_stats().await;
    assert_eq!(stats.circuit_breaker_trips, 1, "1 circuit breaker trip recorded");
    
    // ── Phase 3: Đợi cooldown → recovery ──
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    
    let plan3 = decider.create_plan(make_inputs()).await.unwrap();
    assert_eq!(plan3.execute_count, 3, "Should recover after cooldown");
}

#[tokio::test]
async fn test_i4_circuit_breaker_stats_accumulate() {
    let mut config = StrategyConfig::local_fork();
    config.circuit_breaker_threshold = 2;
    config.circuit_breaker_cooldown_secs = 1;
    let decider = StrategyDecider::new(config);
    
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 50_000.0).await;
    
    // Trip 1: 2 failures → trips (consecutive_failures stays at 2)
    decider.report_failure().await;
    decider.report_failure().await; // trip_count = 1
    
    // Wait cooldown, then create a plan to trigger reset
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let inputs = vec![
        (mock_target("0x1", 0.85, 20_000.0), mock_estimate("0x1", 500.0, 20_000.0, "USDC")),
    ];
    let _ = decider.create_plan(inputs).await.unwrap(); // triggers reset
    
    // Trip 2: 2 more failures → trips again (counter was reset by create_plan)
    decider.report_failure().await;
    decider.report_failure().await; // trip_count = 2
    
    let stats = decider.get_stats().await;
    assert_eq!(stats.circuit_breaker_trips, 2, "Should record 2 trips total");
}

// ============================================================================
// I5: Wallet State Changes → Method Re-evaluation
// Verify toàn bộ plan thay đổi khi wallet state thay đổi
// ============================================================================

#[tokio::test]
async fn test_i5_plan_changes_with_token_availability() {
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);
    
    decider.update_wallet_balance(5.0).await;
    
    let make_inputs = || vec![
        (mock_target("0x1", 0.85, 20_000.0), mock_estimate("0x1", 800.0, 20_000.0, "USDC")),
        (mock_target("0x2", 0.90, 10_000.0), mock_estimate("0x2", 400.0, 10_000.0, "USDC")),
    ];
    
    // ── Không có token → tất cả Skip ──
    let plan1 = decider.create_plan(make_inputs()).await.unwrap();
    assert_eq!(plan1.execute_count, 0, "All should skip without token");
    assert_eq!(plan1.skip_count, 2, "All should skip without token");
    assert_eq!(plan1.direct_count, 0);
    
    // ── Thêm đủ token → tất cả Direct ──
    decider.update_token_balance("USDC".to_string(), 100_000.0).await;
    let plan2 = decider.create_plan(make_inputs()).await.unwrap();
    assert_eq!(plan2.direct_count, 2, "All should use Direct with token");
    assert_eq!(plan2.skip_count, 0);
    
    // Direct plan phải có total profit cao hơn
    assert!(plan2.total_estimated_profit > plan1.total_estimated_profit,
        "Direct plan profit ${:.2} > Skip plan profit ${:.2}",
        plan2.total_estimated_profit, plan1.total_estimated_profit);
}

#[tokio::test]
async fn test_i5_gas_price_blocks_all_targets() {
    let mut config = StrategyConfig::local_fork();
    config.max_gas_price_gwei = 50.0;
    let decider = StrategyDecider::new(config);
    
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 100_000.0).await;
    
    let inputs = vec![
        (mock_target("0x1", 0.85, 20_000.0), mock_estimate("0x1", 800.0, 20_000.0, "USDC")),
        (mock_target("0x2", 0.90, 10_000.0), mock_estimate("0x2", 400.0, 10_000.0, "USDC")),
        (mock_target("0x3", 0.92, 8_000.0), mock_estimate("0x3", 200.0, 8_000.0, "USDC")),
    ];
    
    // Gas bình thường → tất cả execute
    decider.update_gas_price(30.0).await;
    let plan1 = decider.create_plan(inputs.clone()).await.unwrap();
    assert_eq!(plan1.execute_count, 3, "All execute at normal gas");
    
    // Gas spike → tất cả skip
    decider.update_gas_price(200.0).await;
    let plan2 = decider.create_plan(inputs.clone()).await.unwrap();
    assert_eq!(plan2.execute_count, 0, "All skip at high gas");
    assert_eq!(plan2.skip_count, 3);
    
    // Gas giảm → execute lại
    decider.update_gas_price(40.0).await;
    let plan3 = decider.create_plan(inputs).await.unwrap();
    assert_eq!(plan3.execute_count, 3, "All execute after gas drops");
}

// ============================================================================
// I6: Full Pipeline — Targets → Profit filter → Strategy → Plan
// Mô phỏng luồng hoàn chỉnh như trong executor_worker
// ============================================================================

#[tokio::test]
async fn test_i6_full_pipeline_mixed_targets() {
    // ── Bước 1: "Storage" trả về danh sách targets ──
    let all_targets = vec![
        mock_target("0xsafe", 1.50, 10_000.0),       // HF > 1.0, không liquidatable
        mock_target("0xrisky", 0.85, 25_000.0),       // Liquidatable, profitable
        mock_target("0xborderline", 0.99, 5_000.0),   // HF ranh giới
        mock_target("0xdangerous", 0.70, 15_000.0),   // HF rất thấp
        mock_target("0xtiny", 0.90, 500.0),           // Debt quá nhỏ
    ];
    
    // ── Bước 2: Filter liquidatable (HF < 1.0) ──
    let liquidatable: Vec<_> = all_targets.into_iter()
        .filter(|t| t.health_factor < 1.0)
        .collect();
    assert_eq!(liquidatable.len(), 4, "4 targets have HF < 1.0");
    
    // ── Bước 3: "ProfitCalculator" evaluate → chỉ giữ profitable ──
    let profitable_pairs: Vec<(LiquidationTarget, ProfitEstimate)> = liquidatable.into_iter()
        .map(|t| {
            let debt = t.total_debt_usd;
            let user = t.user_address.clone();
            // Giả lập profit calculation: targets nhỏ ($500) không đủ cover gas
            let profit = if debt < 1000.0 { -5.0 } else { debt * 0.02 }; // ~2% profit
            let estimate = mock_estimate(&user, profit, debt, "USDC");
            (t, estimate)
        })
        .collect();
    
    // ── Bước 4: Strategy Decider tạo plan ──
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 100_000.0).await;
    
    let plan = decider.create_plan(profitable_pairs).await.unwrap();
    
    // ── Verify kết quả pipeline ──
    assert_eq!(plan.total_input, 4);
    
    // 0xtiny unprofitable → skip
    let skipped: Vec<_> = plan.targets.iter()
        .filter(|pt| !pt.decision.should_execute())
        .collect();
    assert!(skipped.iter().any(|pt| pt.decision.user_address == "0xtiny"),
        "Tiny debt target should be skipped (unprofitable)");
    
    // 0xrisky, 0xborderline, 0xdangerous should execute
    let executable: Vec<_> = plan.targets.iter()
        .filter(|pt| pt.decision.should_execute())
        .collect();
    assert_eq!(executable.len(), 3, "3 profitable targets should execute");
    
    // 0xdangerous có HF thấp nhất (0.70) → nên có urgency score cao
    // 0xrisky có debt lớn nhất (25k) → nên có profit cao nhất
    // Verify sorted correctly
    assert!(executable[0].decision.priority_score >= executable[1].decision.priority_score);
    assert!(executable[1].decision.priority_score >= executable[2].decision.priority_score);
}

#[tokio::test]
async fn test_i6_pipeline_with_exposure_limits() {
    let mut config = StrategyConfig::local_fork();
    config.max_total_exposure_usd = 20_000.0; // Max $20k total
    let decider = StrategyDecider::new(config);
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 200_000.0).await;
    
    // 3 targets mỗi cái debt_to_cover ~ $10k (20k * 0.5)
    let inputs = vec![
        (mock_target("0x1", 0.85, 20_000.0), mock_estimate("0x1", 500.0, 20_000.0, "USDC")),
        (mock_target("0x2", 0.88, 20_000.0), mock_estimate("0x2", 400.0, 20_000.0, "USDC")),
        (mock_target("0x3", 0.90, 20_000.0), mock_estimate("0x3", 300.0, 20_000.0, "USDC")),
    ];
    
    let plan = decider.create_plan(inputs).await.unwrap();
    
    // Total exposure limit $20k, mỗi target $10k → chỉ 2 fit
    assert_eq!(plan.execute_count, 2, "Only 2 targets fit within $20k exposure");
    assert_eq!(plan.skip_count, 1, "1 target exceeds total exposure");
    assert!(plan.total_exposure_usd <= 20_000.0,
        "Total exposure ${:.0} should <= $20,000", plan.total_exposure_usd);
}

#[tokio::test]
async fn test_i6_pipeline_stats_consistency() {
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);
    decider.update_wallet_balance(10.0).await;
    decider.update_token_balance("USDC".to_string(), 100_000.0).await;
    
    // Tạo 3 plans liên tiếp
    for i in 0..3 {
        let inputs = vec![
            (mock_target(&format!("0x{}_a", i), 0.85, 20_000.0), 
             mock_estimate(&format!("0x{}_a", i), 500.0, 20_000.0, "USDC")),
            (mock_target(&format!("0x{}_b", i), 0.90, 10_000.0), 
             mock_estimate(&format!("0x{}_b", i), 200.0, 10_000.0, "USDC")),
        ];
        let _plan = decider.create_plan(inputs).await.unwrap();
    }
    
    let stats = decider.get_stats().await;
    assert_eq!(stats.total_plans, 3, "3 plans created");
    assert_eq!(stats.total_decisions, 6, "6 decisions total (2 per plan × 3)");
    assert_eq!(stats.direct_count, 6, "All 6 should be Direct");
    assert_eq!(stats.skip_count, 0, "No skips");
}
