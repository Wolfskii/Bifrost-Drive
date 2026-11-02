//! What the Explorer menu offers and what each choice asks Bifrost Drive to do. This is plain
//! logic with no COM, so it can be tested without Explorer.

use std::path::{Path, PathBuf};

use serde::Serialize;

pub const COPY_VERB: &str = "bifrost.copy";
pub const MOVE_VERB: &str = "bifrost.move";
pub const PASTE_VERB: &str = "bifrost.paste";
pub const DELETE_VERB: &str = "bifrost.delete";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Copy,
    Move,
    Delete,
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
    /// Items right-clicked in a folder window.
    Selection { items: Vec<PathBuf> },
}

impl Gesture {
    pub fn sources(&self) -> &[PathBuf] {
        match self {
            Self::Drop { sources, .. } | Self::Paste { sources, .. } => sources,
            Self::Selection { items } => items,
        }
    }

    fn target(&self) -> &Path {
        match self {
            Self::Drop { target, .. } | Self::Paste { target, .. } => target,
            // Deleting has no destination.
            Self::Selection { .. } => Path::new(""),
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

const DELETE_ENTRY: MenuEntry = MenuEntry {
    verb: DELETE_VERB,
    label: "Delete with Bifrost Drive",
    help: "Delete many files at once with Bifrost Drive's transfer window",
    mode: Mode::Delete,
};

/// The entries to add to the menu. Nothing is offered when the setting is off or when there is
/// nothing to transfer. Paste is offered wherever files wait on the clipboard, on any drive;
/// the right-drag items (which would crowd every ordinary drag) only when a Bifrost drive is
/// one of the two sides. Delete is offered only for items on a Bifrost drive (other drives have
/// a recycle bin and their own fast deletion) and never for a whole drive.
pub fn entries(
    gesture: &Gesture,
    enabled: bool,
    is_bifrost_drive: &dyn Fn(&Path) -> bool,
) -> Vec<MenuEntry> {
    if !enabled || gesture.sources().is_empty() {
        return Vec::new();
    }
    match gesture {
        Gesture::Drop { .. } => {
            // Explorer selections come from one folder, so the first item stands for all.
            let involves_drive =
                is_bifrost_drive(gesture.target()) || is_bifrost_drive(&gesture.sources()[0]);
            if involves_drive {
                vec![COPY_ENTRY, MOVE_ENTRY]
            } else {
                Vec::new()
            }
        }
        Gesture::Selection { items } => {
            let is_drive_root = |item: &PathBuf| item.parent().is_none();
            if items.iter().any(is_drive_root) || !is_bifrost_drive(&items[0]) {
                Vec::new()
            } else {
                vec![DELETE_ENTRY]
            }
        }
        Gesture::Paste { clipboard_mode, .. } => vec![MenuEntry {
            verb: PASTE_VERB,
            label: "Paste with Bifrost Drive",
            help: "Paste with Bifrost Drive's transfer window",
            mode: *clipboard_mode,
        }],
    }
}

/// The entries for every way the current click can be read.
pub fn entries_for(
    gestures: &[Gesture],
    enabled: bool,
    is_bifrost_drive: &dyn Fn(&Path) -> bool,
) -> Vec<MenuEntry> {
    gestures
        .iter()
        .flat_map(|gesture| entries(gesture, enabled, is_bifrost_drive))
        .collect()
}

/// The reading of the click that `entry` belongs to.
pub fn gesture_for_entry<'a>(gestures: &'a [Gesture], entry: &MenuEntry) -> Option<&'a Gesture> {
    gestures.iter().find(|gesture| match gesture {
        Gesture::Drop { .. } => entry.verb == COPY_VERB || entry.verb == MOVE_VERB,
        Gesture::Paste { .. } => entry.verb == PASTE_VERB,
        Gesture::Selection { .. } => entry.verb == DELETE_VERB,
    })
}

/// What the confirmation box says before anything is deleted.
pub fn delete_prompt(items: &[PathBuf]) -> String {
    let subject = match items {
        [only] => only.file_name().map_or_else(
            || format!("\u{201c}{}\u{201d}", only.display()),
            |name| format!("\u{201c}{}\u{201d}", name.to_string_lossy()),
        ),
        many => format!("these {} items", many.len()),
    };
    format!("Permanently delete {subject} from the server?\n\nThis can't be undone.")
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
    fn ordinary_drags_never_see_the_menu() {
        assert!(entries(&dropped(&[r"C:\a.mkv"], r"D:\Videos"), true, &drive).is_empty());
    }

    #[test]
    fn paste_is_offered_on_every_drive_not_only_bifrost_ones() {
        let offered = entries(
            &pasted(&[r"C:\a.mkv"], r"D:\Videos", Mode::Move),
            true,
            &drive,
        );

        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].label, "Paste with Bifrost Drive");
        assert_eq!(offered[0].mode, Mode::Move);
        assert!(entries(
            &pasted(&[r"C:\a.mkv"], r"D:\Videos", Mode::Copy),
            false,
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

    fn selected(items: &[&str]) -> Gesture {
        Gesture::Selection {
            items: items.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn delete_is_offered_only_for_items_on_a_bifrost_drive() {
        let offered = entries(
            &selected(&[r"Y:\Filmer\a.mkv", r"Y:\Filmer\b.mkv"]),
            true,
            &drive,
        );

        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].label, "Delete with Bifrost Drive");
        assert_eq!(offered[0].mode, Mode::Delete);
        assert!(entries(&selected(&[r"C:\Videos\a.mkv"]), true, &drive).is_empty());
        assert!(entries(&selected(&[r"Y:\a.mkv"]), false, &drive).is_empty());
    }

    #[test]
    fn a_whole_drive_is_never_offered_for_deletion() {
        assert!(entries(&selected(&[r"Y:\"]), true, &drive).is_empty());
        assert!(entries(&selected(&[r"Y:\a.mkv", r"Y:\"]), true, &drive).is_empty());
    }

    #[test]
    fn the_delete_request_names_no_destination() {
        let gesture = selected(&[r"Y:\Filmer\a.mkv"]);
        let offered = entries(&gesture, true, &drive);

        let json = serde_json::to_value(request_for(&gesture, &offered[0])).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "version": 1,
                "mode": "delete",
                "sources": [r"Y:\Filmer\a.mkv"],
                "destination": "",
            })
        );
    }

    #[test]
    fn the_confirmation_names_one_item_or_counts_many() {
        assert_eq!(
            delete_prompt(&[PathBuf::from(r"Y:\Filmer\a.mkv")]),
            "Permanently delete \u{201c}a.mkv\u{201d} from the server?\n\nThis can't be undone."
        );
        assert!(
            delete_prompt(&[PathBuf::from(r"Y:\a"), PathBuf::from(r"Y:\b")])
                .starts_with("Permanently delete these 2 items from the server?")
        );
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
