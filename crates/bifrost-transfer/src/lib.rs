use bifrost_common::{ConnectionId, RemotePath, TransferId};
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Download,
    Upload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TransferJob {
    pub id: TransferId,
    pub connection_id: ConnectionId,
    pub path: RemotePath,
    pub direction: TransferDirection,
    pub total_bytes: Option<u64>,
    pub transferred_bytes: u64,
    pub attempts: u32,
    pub status: TransferStatus,
    pub next_retry_at: Option<SystemTime>,
    sequence: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransferError {
    #[error("transfer {0} was not found")]
    NotFound(String),
    #[error("transfer {0} cannot be changed while {1:?}")]
    InvalidTransition(String, TransferStatus),
}

#[derive(Debug)]
pub struct TransferQueue {
    max_concurrent: usize,
    max_attempts: u32,
    active: usize,
    next_sequence: u64,
    jobs: HashMap<TransferId, TransferJob>,
}

impl TransferQueue {
    pub fn new(max_concurrent: usize, max_attempts: u32) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
            max_attempts: max_attempts.max(1),
            active: 0,
            next_sequence: 0,
            jobs: HashMap::new(),
        }
    }

    pub fn enqueue(
        &mut self,
        connection_id: ConnectionId,
        path: RemotePath,
        direction: TransferDirection,
        total_bytes: Option<u64>,
    ) -> TransferId {
        let id = TransferId::new();
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.jobs.insert(
            id,
            TransferJob {
                id,
                connection_id,
                path,
                direction,
                total_bytes,
                transferred_bytes: 0,
                attempts: 0,
                status: TransferStatus::Pending,
                next_retry_at: None,
                sequence,
            },
        );
        id
    }

    pub fn start_available(&mut self, now: SystemTime) -> Vec<TransferId> {
        let capacity = self.max_concurrent.saturating_sub(self.active);
        let mut pending: Vec<_> = self
            .jobs
            .values_mut()
            .filter(|job| {
                job.status == TransferStatus::Pending
                    && job.next_retry_at.is_none_or(|retry_at| retry_at <= now)
            })
            .collect();
        pending.sort_by_key(|job| job.sequence);
        pending.truncate(capacity);

        let started: Vec<_> = pending
            .into_iter()
            .map(|job| {
                job.status = TransferStatus::Running;
                job.attempts += 1;
                job.next_retry_at = None;
                job.id
            })
            .collect();
        self.active += started.len();
        started
    }

    pub fn update_progress(
        &mut self,
        id: TransferId,
        transferred_bytes: u64,
    ) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        if job.status != TransferStatus::Running {
            return Err(TransferError::InvalidTransition(id.to_string(), job.status));
        }
        job.transferred_bytes = match job.total_bytes {
            Some(total) => transferred_bytes.min(total),
            None => transferred_bytes,
        };
        Ok(())
    }

    pub fn complete(&mut self, id: TransferId) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        if job.status != TransferStatus::Running {
            return Err(TransferError::InvalidTransition(id.to_string(), job.status));
        }
        job.status = TransferStatus::Completed;
        self.active = self.active.saturating_sub(1);
        Ok(())
    }

    pub fn fail(
        &mut self,
        id: TransferId,
        retryable: bool,
        now: SystemTime,
    ) -> Result<(), TransferError> {
        let (status, attempts) = {
            let job = self.get(id)?;
            (job.status, job.attempts)
        };
        if status != TransferStatus::Running {
            return Err(TransferError::InvalidTransition(id.to_string(), status));
        }
        self.active = self.active.saturating_sub(1);
        let max_attempts = self.max_attempts;
        let job = self.job_mut(id)?;
        if retryable && attempts < max_attempts {
            let exponent = attempts.min(7);
            job.status = TransferStatus::Pending;
            job.next_retry_at = Some(now + Duration::from_secs(2u64.pow(exponent)));
        } else {
            job.status = TransferStatus::Failed;
        }
        Ok(())
    }

    pub fn pause(&mut self, id: TransferId) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        match job.status {
            TransferStatus::Pending => job.status = TransferStatus::Paused,
            TransferStatus::Running => {
                job.status = TransferStatus::Paused;
                self.active = self.active.saturating_sub(1);
            }
            status => return Err(TransferError::InvalidTransition(id.to_string(), status)),
        }
        Ok(())
    }

    pub fn resume(&mut self, id: TransferId) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        if job.status != TransferStatus::Paused {
            return Err(TransferError::InvalidTransition(id.to_string(), job.status));
        }
        job.status = TransferStatus::Pending;
        Ok(())
    }

    pub fn cancel(&mut self, id: TransferId) -> Result<(), TransferError> {
        let job = self.job_mut(id)?;
        match job.status {
            TransferStatus::Pending | TransferStatus::Paused => {
                job.status = TransferStatus::Cancelled
            }
            TransferStatus::Running => {
                job.status = TransferStatus::Cancelled;
                self.active = self.active.saturating_sub(1);
            }
            status => return Err(TransferError::InvalidTransition(id.to_string(), status)),
        }
        Ok(())
    }

    pub fn get(&self, id: TransferId) -> Result<&TransferJob, TransferError> {
        self.jobs
            .get(&id)
            .ok_or_else(|| TransferError::NotFound(id.to_string()))
    }

    fn job_mut(&mut self, id: TransferId) -> Result<&mut TransferJob, TransferError> {
        self.jobs
            .get_mut(&id)
            .ok_or_else(|| TransferError::NotFound(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{TransferDirection, TransferQueue, TransferStatus};
    use bifrost_common::{ConnectionId, RemotePath};
    use std::time::{Duration, SystemTime};

    #[test]
    fn starts_only_within_the_concurrency_limit() {
        let mut queue = TransferQueue::new(1, 3);
        let first = queue.enqueue(
            ConnectionId::new(),
            RemotePath::root(),
            TransferDirection::Download,
            Some(5),
        );
        let second = queue.enqueue(
            ConnectionId::new(),
            RemotePath::root(),
            TransferDirection::Upload,
            None,
        );
        let started = queue.start_available(SystemTime::now());
        assert_eq!(started, vec![first]);
        assert_eq!(queue.get(second).unwrap().status, TransferStatus::Pending);
    }

    #[test]
    fn retry_uses_capped_exponential_backoff_and_eventually_fails() {
        let now = SystemTime::UNIX_EPOCH;
        let mut queue = TransferQueue::new(1, 2);
        let id = queue.enqueue(
            ConnectionId::new(),
            RemotePath::root(),
            TransferDirection::Download,
            None,
        );
        queue.start_available(now);
        queue.fail(id, true, now).unwrap();
        assert_eq!(queue.get(id).unwrap().status, TransferStatus::Pending);
        assert_eq!(
            queue.get(id).unwrap().next_retry_at,
            Some(now + Duration::from_secs(2))
        );
        queue.start_available(now + Duration::from_secs(2));
        queue.fail(id, true, now + Duration::from_secs(2)).unwrap();
        assert_eq!(queue.get(id).unwrap().status, TransferStatus::Failed);
    }

    #[test]
    fn pause_resume_cancel_and_progress_are_explicit_state_changes() {
        let mut queue = TransferQueue::new(2, 1);
        let id = queue.enqueue(
            ConnectionId::new(),
            RemotePath::root(),
            TransferDirection::Download,
            Some(10),
        );
        queue.start_available(SystemTime::now());
        queue.update_progress(id, 12).unwrap();
        assert_eq!(queue.get(id).unwrap().transferred_bytes, 10);
        queue.pause(id).unwrap();
        queue.resume(id).unwrap();
        queue.cancel(id).unwrap();
        assert_eq!(queue.get(id).unwrap().status, TransferStatus::Cancelled);
    }
}
