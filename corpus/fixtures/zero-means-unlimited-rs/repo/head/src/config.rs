/// Connection pool settings, loaded from `pool.toml`.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Upper bound on open connections; 0 means no limit.
    pub max_connections: usize,
    pub idle_timeout_secs: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig {
            max_connections: 0,
            idle_timeout_secs: 300,
        }
    }
}
