//! What the Explorer menu offers and what each choice asks Bifrost Drive to do. This is plain
//! logic with no COM, so it can be tested without Explorer.

use std::path::{Path, PathBuf};

use serde::Serialize;

pub const COPY_VERB: &str = "bifrost.copy";
pub const MOVE_VERB: &str = "bifrost.move";
pub const PASTE_VERB: &str = "bifrost.paste";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Copy,
    Move,
}

/// What the user did in Explorer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gesture {
    /// Files dragged with the right mouse button and dropped on `target`.
    Drop {
        sources: Vec<PathBuf>,
        target: PathBuf,
    },
    /// Files waiting on the clipboard, to be pasted into `target`.
    Paste {
        sources: Vec<PathBuf>,
        target: PathBuf,
        clipboard_mode: Mode,
    },
}

impl Gesture {
    fn sources(&self) -> &[PathBuf] {
        match self {
            Self::Drop { sources, .. } | Self::Paste { sources, .. } => sources,
        }
    }

    fn target(&self) -> &Path {
        match self {
            Self::Drop { target, .. } | Self::Paste { target, .. } => target,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuEntry {
    pub verb: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub mode: Mode,
}

const COPY_ENTRY: MenuEntry = MenuEntry {
    verb: COPY_VERB,
    label: "Copy here with Bifrost Drive",
    help: "Copy with Bifrost Drive's transfer window",
    mode: Mode::Copy,
};
const MOVE_ENTRY: MenuEntry = MenuEntry {
    verb: MOVE_VERB,
    label: "Move here with Bifrost Drive",
    help: "Move with Bifrost Drive's transfer window",
    mode: Mode::Move,
};

/// The entries to add to the menu. Nothing is offered when the setting is off, when there is
/// nothing to transfer, or when neither side is on a Bifrost drive, so ordinary copies never
/// see the extension.
pub fn entries(
    gesture: &Gesture,
    enabled: bool,
    is_bifrost_drive: &dyn Fn(&Path) -> bool,
) -> Vec<MenuEntry> {
    if !enabled || gesture.sources().is_empty() {
        return Vec::new();
    }
    // Explorer selections come from one folder, so the first item stands for all of them.
    let involves_drive =
        is_bifrost_drive(gesture.target()) || is_bifrost_drive(&gesture.sources()[0]);
    if !involves_drive {
        return Vec::new();
    }
    match gesture {
        Gesture::Drop { .. } => vec![COPY_ENTRY, MOVE_ENTRY],
        Gesture::Paste { clipboard_mode, .. } => vec![MenuEntry {
            verb: PASTE_VERB,
            label: "Paste with Bifrost Drive",
            help: "Paste with Bifrost Drive's transfer window",
            mode: *clipboard_mode,
        }],
    }
}

/// The entry a verb name (from `InvokeCommand`) refers to, among those offered.
pub fn entry_for_verb(offered: &[MenuEntry], verb: &str) -> Option<MenuEntry> {
    offered.iter().copied().find(|entry| entry.verb == verb)
}

/// The request handed to Bifrost Drive.
#[derive(Debug, Serialize)]
pub struct Request {
    pub version: u32,
    pub mode: Mode,
    pub sources: Vec<String>,
    pub destination: String,
}

pub const REQUEST_VERSION: u32 = 1;

pub fn request_for(gesture: &Gesture, entry: &MenuEntry) -> Request {
    Request {
        version: REQUEST_VERSION,
        mode: entry.mode,
        sources: gesture
            .sources()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        destination: gesture.target().to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(path: &Path) -> bool {
        path.to_string_lossy().to_lowercase().starts_with("y:")
    }

    fn dropped(sources: &[&str], target: &str) -> Gesture {
        Gesture::Drop {
            sources: sources.iter().map(PathBuf::from).collect(),
            target: PathBuf::from(target),
        }
    }

    fn pasted(sources: &[&str], target: &str, mode: Mode) -> Gesture {
        Gesture::Paste {
            sources: sources.iter().map(PathBuf::from).collect(),
            target: PathBuf::from(target),
            clipboard_mode: mode,
        }
    }

    #[test]
    fn right_drag_to_a_drive_offers_copy_and_move() {
        let offered = entries(&dropped(&[r"C:\Videos\a.mkv"], r"Y:\Filmer"), true, &drive);

        assert_eq!(
            offered.iter().map(|entry| entry.label).collect::<Vec<_>>(),
            vec![
                "Copy here with Bifrost Drive",
                "Move here with Bifrost Drive"
            ]
        );
    }

    #[test]
    fn right_drag_from_a_drive_to_the_pc_is_offered_too() {
        assert_eq!(
            entries(&dropped(&[r"Y:\Filmer\a.mkv"], r"C:\Videos"), true, &drive).len(),
            2
        );
    }

    #[test]
    fn paste_follows_whether_the_clipboard_holds_a_cut_or_a_copy() {
        let cut = entries(
            &pasted(&[r"C:\a.mkv"], r"Y:\Filmer", Mode::Move),
            true,
            &drive,
        );
        let copy = entries(
            &pasted(&[r"C:\a.mkv"], r"Y:\Filmer", Mode::Copy),
            true,
            &drive,
        );

        assert_eq!(cut[0].label, "Paste with Bifrost Drive");
        assert_eq!(cut[0].mode, Mode::Move);
        assert_eq!(copy[0].mode, Mode::Copy);
    }

    #[test]
    fn ordinary_copies_never_see_the_menu() {
        assert!(entries(&dropped(&[r"C:\a.mkv"], r"D:\Videos"), true, &drive).is_empty());
        assert!(entries(
            &pasted(&[r"C:\a.mkv"], r"D:\Videos", Mode::Copy),
            true,
            &drive
        )
        .is_empty());
    }

    #[test]
    fn the_setting_and_an_empty_selection_hide_the_menu() {
        assert!(entries(&dropped(&[r"C:\a.mkv"], r"Y:\Filmer"), false, &drive).is_empty());
        assert!(entries(&dropped(&[], r"Y:\Filmer"), true, &drive).is_empty());
    }

    #[test]
    fn verbs_resolve_only_among_the_offered_entries() {
        let offered = entries(&dropped(&[r"C:\a.mkv"], r"Y:\Filmer"), true, &drive);

        assert_eq!(
            entry_for_verb(&offered, MOVE_VERB).unwrap().mode,
            Mode::Move
        );
        assert_eq!(entry_for_verb(&offered, PASTE_VERB), None);
    }

    #[test]
    fn the_request_carries_every_path_and_the_chosen_mode() {
        let gesture = dropped(&[r"C:\a.mkv", r"C:\b.mkv"], r"Y:\Filmer");
        let offered = entries(&gesture, true, &drive);

        let json = serde_json::to_value(request_for(&gesture, &offered[1])).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "version": 1,
                "mode": "move",
                "sources": [r"C:\a.mkv", r"C:\b.mkv"],
                "destination": r"Y:\Filmer",
            })
        );
    }
}
