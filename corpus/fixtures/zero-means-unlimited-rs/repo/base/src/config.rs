/// Connection pool settings, loaded from `pool.toml`.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_connections: usize,
    pub idle_timeout_secs: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig {
            max_connections: 10,
            idle_timeout_secs: 300,
        }
    }
}
