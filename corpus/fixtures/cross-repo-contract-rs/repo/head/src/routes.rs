use crate::jobs::Job;

/// GET /jobs/:id — serialized with serde_json.
pub fn get_job(id: u64) -> Job {
    unimplemented!("{id}")
}
