use bifrost_common::SyncState;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub fingerprint: String,
}

impl Revision {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            fingerprint: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationInput {
    pub base: Option<Revision>,
    pub local: Option<Revision>,
    pub remote: Option<Revision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    KeepLocal,
    KeepRemote,
    KeepBoth,
    RenameConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncDecision {
    UpToDate,
    DownloadRemote,
    UploadLocal,
    DeleteLocal,
    DeleteRemote,
    Conflict,
    Resolved(ConflictResolution),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateTransition {
    pub from: SyncState,
    pub to: SyncState,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SyncError {
    #[error("conflict requires an explicit resolution")]
    ConflictRequiresResolution,
    #[error("invalid synchronization state transition from {from:?} to {to:?}")]
    InvalidTransition { from: SyncState, to: SyncState },
}

pub fn reconcile(input: &ReconciliationInput) -> SyncDecision {
    match (&input.base, &input.local, &input.remote) {
        (_base, local, remote) if local == remote => SyncDecision::UpToDate,
        (None, Some(_), None) => SyncDecision::UploadLocal,
        (None, None, Some(_)) => SyncDecision::DownloadRemote,
        (Some(base), Some(local), Some(remote)) if local == base && remote != base => {
            SyncDecision::DownloadRemote
        }
        (Some(base), Some(local), Some(remote)) if remote == base && local != base => {
            SyncDecision::UploadLocal
        }
        (Some(base), None, Some(remote)) if remote == base => SyncDecision::DeleteLocal,
        (Some(base), Some(local), None) if local == base => SyncDecision::DeleteRemote,
        _ => SyncDecision::Conflict,
    }
}

pub fn resolve(
    input: &ReconciliationInput,
    resolution: Option<ConflictResolution>,
) -> Result<SyncDecision, SyncError> {
    if reconcile(input) != SyncDecision::Conflict {
        return Ok(reconcile(input));
    }
    resolution
        .map(SyncDecision::Resolved)
        .ok_or(SyncError::ConflictRequiresResolution)
}

pub fn transition(from: SyncState, to: SyncState) -> Result<StateTransition, SyncError> {
    let valid = match (from, to) {
        (state, next) if state == next => true,
        (SyncState::Online, SyncState::Syncing)
        | (SyncState::Offline, SyncState::Online)
        | (SyncState::Syncing, SyncState::Downloading)
        | (SyncState::Syncing, SyncState::Uploading)
        | (SyncState::Downloading, SyncState::UpToDate)
        | (SyncState::Uploading, SyncState::UpToDate)
        | (SyncState::Syncing, SyncState::Conflict)
        | (SyncState::Syncing, SyncState::Error)
        | (SyncState::Error, SyncState::Syncing)
        | (SyncState::Conflict, SyncState::Syncing)
        | (SyncState::UpToDate, SyncState::Syncing)
        | (SyncState::Online, SyncState::Offline) => true,
        _ => false,
    };
    if valid {
        Ok(StateTransition { from, to })
    } else {
        Err(SyncError::InvalidTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        reconcile, resolve, transition, ConflictResolution, ReconciliationInput, Revision,
        SyncDecision,
    };
    use bifrost_common::SyncState;

    fn revision(value: &str) -> Option<Revision> {
        Some(Revision::new(value))
    }

    #[test]
    fn detects_one_sided_changes() {
        let base = revision("base");
        assert_eq!(
            reconcile(&ReconciliationInput {
                base: base.clone(),
                local: base.clone(),
                remote: revision("remote")
            }),
            SyncDecision::DownloadRemote
        );
        assert_eq!(
            reconcile(&ReconciliationInput {
                base: base.clone(),
                local: revision("local"),
                remote: base
            }),
            SyncDecision::UploadLocal
        );
    }

    #[test]
    fn refuses_to_overwrite_when_both_sides_changed() {
        let input = ReconciliationInput {
            base: revision("base"),
            local: revision("local"),
            remote: revision("remote"),
        };
        assert_eq!(reconcile(&input), SyncDecision::Conflict);
        assert_eq!(
            resolve(&input, None),
            Err(super::SyncError::ConflictRequiresResolution)
        );
        assert_eq!(
            resolve(&input, Some(ConflictResolution::KeepBoth)).unwrap(),
            SyncDecision::Resolved(ConflictResolution::KeepBoth)
        );
    }

    #[test]
    fn validates_conservative_state_transitions() {
        assert!(transition(SyncState::Online, SyncState::Syncing).is_ok());
        assert!(transition(SyncState::Downloading, SyncState::Uploading).is_err());
    }
}
