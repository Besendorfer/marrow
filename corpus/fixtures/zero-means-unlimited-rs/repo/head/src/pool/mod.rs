pub struct Pool {
    max: usize,
    in_use: usize,
    idle_timeout_secs: u64,
}

#[derive(Debug)]
pub enum PoolError {
    Exhausted,
}

impl Pool {
    pub fn new(max: usize, idle_timeout_secs: u64) -> Pool {
        Pool { max, in_use: 0, idle_timeout_secs }
    }

    pub fn acquire(&mut self) -> Result<Conn, PoolError> {
        if self.in_use >= self.max {
            return Err(PoolError::Exhausted);
        }
        self.in_use += 1;
        Ok(Conn::open())
    }
}
