use serde::{Deserialize, Serialize};

/// Lifecycle of a background job, as serialized in the /jobs API.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    InProgress,
    Succeeded,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub status: JobStatus,
}
