pub struct Executor;

impl Executor {
    pub fn liquidate(user_id: &str) {
        tracing::warn!("EXECUTING LIQUIDATION FOR USER {}", user_id);
    }
}
