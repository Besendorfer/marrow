use crate::config::PoolConfig;
use crate::pool::Pool;

pub fn pool_from_config(cfg: &PoolConfig) -> Pool {
    Pool::new(cfg.max_connections, cfg.idle_timeout_secs)
}
