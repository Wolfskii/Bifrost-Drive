use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bifrost_db::Database;
use bifrost_windows_winfsp::{
    DriveFailure, DriveFailureReason, DriveFailureSink, DriveOperation, FailureHandling,
    UploadRecovery,
};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;

pub const RECOVERED_UPLOADS_FOLDER: &str = "Recovered uploads";
pub const UPLOAD_FAILED_ACTIVITY: &str = "drive_upload_failed";
pub const OPERATION_FAILED_ACTIVITY: &str = "drive_operation_failed";
/// Repeated failures on one drive, such as a multi-file copy into a refused folder, share
/// one notification; every failure still reaches the activity history.
const NOTIFICATION_QUIET_PERIOD: Duration = Duration::from_secs(20);
const MAX_DETAIL_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureMessage {
    pub title: String,
    pub body: String,
}

/// Words a failure for people: which file, what went wrong, what to do, and where data went.
pub fn describe(failure: &DriveFailure, connection: &str, drive_letter: char) -> FailureMessage {
    let windows_path = failure.path.as_str().replace('/', "\\");
    let name = windows_path
        .rsplit('\\')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("the drive");
    let folder = match windows_path.rsplit_once('\\') {
        Some((parent, _)) => format!(r"{drive_letter}:\{parent}"),
        None => format!(r"{drive_letter}:\"),
    };
    let title = match failure.operation {
        DriveOperation::CreateFile => format!("Couldn't create “{name}”"),
        DriveOperation::CreateFolder => format!("Couldn't create the folder “{name}”"),
        DriveOperation::Save => format!("Couldn't save “{name}”"),
        DriveOperation::Upload => format!("“{name}” wasn't uploaded"),
        DriveOperation::Delete => format!("Couldn't delete “{name}”"),
        DriveOperation::Rename => format!("Couldn't move or rename “{name}”"),
    };
    let reason = match &failure.reason {
        DriveFailureReason::PermissionDenied => format!(
            "{connection} doesn't allow changes in {folder}. Check that the server account has write permission there."
        ),
        DriveFailureReason::AuthenticationFailed => format!(
            "{connection} rejected the sign-in. Update the connection's credentials in Bifrost Drive."
        ),
        DriveFailureReason::Network(detail) => format!(
            "The connection to {connection} was lost ({}). Check that the server is reachable.",
            shorten(detail)
        ),
        DriveFailureReason::Unsupported => format!("{connection} doesn't support this action."),
        DriveFailureReason::Remote(detail) => {
            format!("{connection} reported a problem: {}.", shorten(detail))
        }
        DriveFailureReason::Local(detail) => {
            format!("A problem on this PC stopped it: {}.", shorten(detail))
        }
    };
    let body = match &failure.recovery {
        Some(UploadRecovery::Saved(path)) => {
            format!("{reason} Your data is safe in {}.", path.display())
        }
        Some(UploadRecovery::Lost(detail)) => format!(
            "{reason} Bifrost Drive also couldn't keep a local copy ({}), so these changes may be lost.",
            shorten(detail)
        ),
        None => reason,
    };
    FailureMessage { title, body }
}

fn shorten(detail: &str) -> String {
    let detail = detail.trim().trim_end_matches('.');
    if detail.chars().count() <= MAX_DETAIL_CHARS {
        return detail.to_owned();
    }
    let mut short = detail
        .chars()
        .take(MAX_DETAIL_CHARS - 1)
        .collect::<String>();
    short.push('…');
    short
}

pub fn recovery_root(local_data_dir: &Path) -> PathBuf {
    local_data_dir.join(RECOVERED_UPLOADS_FOLDER)
}

fn recovery_dir(local_data_dir: &Path, connection: &str) -> PathBuf {
    let folder = connection
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();
    let folder = folder.trim().trim_end_matches('.');
    recovery_root(local_data_dir).join(if folder.is_empty() { "Drive" } else { folder })
}

#[derive(Default)]
struct QuietPeriod {
    last_shown: Option<Instant>,
    suppressed: usize,
}

impl QuietPeriod {
    /// Returns how many failures were held back since the last notification, or `None` to stay
    /// quiet. Failures that kept data aside always notify so the user learns where it is.
    fn admit(&mut self, now: Instant, always: bool) -> Option<usize> {
        let quiet = self
            .last_shown
            .is_some_and(|shown| now.duration_since(shown) < NOTIFICATION_QUIET_PERIOD);
        if quiet && !always {
            self.suppressed += 1;
            return None;
        }
        self.last_shown = Some(now);
        Some(std::mem::take(&mut self.suppressed))
    }
}

/// Builds the WinFsp failure handling for one mounted connection.
pub fn failure_handling<R: Runtime>(
    app: &AppHandle<R>,
    local_data_dir: &Path,
    connection: String,
    drive_letter: char,
) -> FailureHandling {
    let app = app.clone();
    let quiet = Arc::new(Mutex::new(QuietPeriod::default()));
    let connection_name = connection.clone();
    let sink: DriveFailureSink = Arc::new(move |failure: DriveFailure| {
        let message = describe(&failure, &connection_name, drive_letter);
        let recovered = failure.recovery.is_some();
        let admitted = quiet
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .admit(Instant::now(), recovered);
        if let Some(suppressed) = admitted {
            let body = if suppressed == 0 {
                message.body.clone()
            } else {
                format!(
                    "{} {suppressed} more problem{} on {connection_name} are listed in Activity.",
                    message.body,
                    if suppressed == 1 { "" } else { "s" }
                )
            };
            if let Err(error) = app
                .notification()
                .builder()
                .title(&message.title)
                .body(body)
                .show()
            {
                tracing::warn!(%error, "could not show drive failure notification");
            }
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let kind = if recovered {
                UPLOAD_FAILED_ACTIVITY
            } else {
                OPERATION_FAILED_ACTIVITY
            };
            let details = format!("{}. {}", message.title, message.body);
            if let Err(error) = app
                .state::<Database>()
                .insert_activity(kind, Some(&details), "failed")
                .await
            {
                tracing::warn!(%error, "could not record drive failure in activity history");
            }
        });
    });
    FailureHandling {
        recovery_dir: recovery_dir(local_data_dir, &connection),
        sink: Some(sink),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_common::RemotePath;

    fn failure(
        operation: DriveOperation,
        path: &str,
        reason: DriveFailureReason,
        recovery: Option<UploadRecovery>,
    ) -> DriveFailure {
        DriveFailure {
            operation,
            path: RemotePath::parse(path).unwrap(),
            reason,
            recovery,
        }
    }

    #[test]
    fn refused_upload_names_the_folder_and_where_the_data_is() {
        let message = describe(
            &failure(
                DriveOperation::Upload,
                "Filmer/2026-10-31_16-10-30.mkv",
                DriveFailureReason::PermissionDenied,
                Some(UploadRecovery::Saved(PathBuf::from(
                    r"C:\Recovered uploads\Yggdrasil SFTP\Filmer\2026-10-31_16-10-30.mkv",
                ))),
            ),
            "Yggdrasil SFTP",
            'Y',
        );

        assert_eq!(message.title, "“2026-10-31_16-10-30.mkv” wasn't uploaded");
        assert_eq!(
            message.body,
            r"Yggdrasil SFTP doesn't allow changes in Y:\Filmer. Check that the server account has write permission there. Your data is safe in C:\Recovered uploads\Yggdrasil SFTP\Filmer\2026-10-31_16-10-30.mkv."
        );
    }

    #[test]
    fn files_in_the_drive_root_name_the_drive() {
        let message = describe(
            &failure(
                DriveOperation::CreateFolder,
                "Projects",
                DriveFailureReason::PermissionDenied,
                None,
            ),
            "NAS",
            'Y',
        );

        assert_eq!(message.title, "Couldn't create the folder “Projects”");
        assert!(message
            .body
            .starts_with(r"NAS doesn't allow changes in Y:\."));
    }

    #[test]
    fn lost_data_is_stated_plainly_and_long_details_are_shortened() {
        let message = describe(
            &failure(
                DriveOperation::Upload,
                "notes.txt",
                DriveFailureReason::Remote("x".repeat(400)),
                Some(UploadRecovery::Lost("disk full".to_owned())),
            ),
            "NAS",
            'Y',
        );

        assert!(message
            .body
            .contains("couldn't keep a local copy (disk full)"));
        assert!(message.body.contains('…'));
        assert!(message.body.len() < 400);
    }

    #[test]
    fn quiet_period_collapses_bursts_but_always_shows_recovered_uploads() {
        let mut quiet = QuietPeriod::default();
        let start = Instant::now();

        assert_eq!(quiet.admit(start, false), Some(0));
        assert_eq!(quiet.admit(start + Duration::from_secs(1), false), None);
        assert_eq!(quiet.admit(start + Duration::from_secs(2), false), None);
        assert_eq!(quiet.admit(start + Duration::from_secs(3), true), Some(2));
        assert_eq!(
            quiet.admit(start + NOTIFICATION_QUIET_PERIOD * 2, false),
            Some(0)
        );
    }

    #[test]
    fn recovery_folders_are_per_connection_and_windows_safe() {
        assert_eq!(
            recovery_dir(Path::new(r"C:\Data"), "Yggdrasil: SFTP?"),
            PathBuf::from(r"C:\Data\Recovered uploads\Yggdrasil_ SFTP_")
        );
    }
}
