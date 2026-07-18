//! Persistent note state: one scrollable markdown note PER WORKSPACE plus the
//! last-active mode, stored as a small JSON file beside herdr's own config
//! (`%APPDATA%\herdr\aa-notes\<workspace-id>.json` on Windows,
//! `$XDG_CONFIG_HOME/herdr/…` elsewhere) so the note survives computer
//! restarts. The key is the stable `HERDR_WORKSPACE_ID` herdr injects into
//! every managed pane; outside herdr (or on an id unsafe for a filename) the
//! pane falls back to the legacy single-note `herdr/aa-notes.json`, and the
//! first workspace to load notes MOVES that legacy file into its own slot.
//!
//! Loading is forgiving — a missing, hand-edited, or truncated file falls back
//! to an empty note and never panics. Saving is atomic (temp file + rename)
//! and best-effort: the pane keeps working for the session if persist fails.

use std::path::{Path, PathBuf};

/// Pane label the launcher assigns and the heartbeat re-asserts as the title.
pub const PANE_LABEL: &str = "Notes";

/// Source id for `pane.report_metadata`; its token marks a pane as the Notes
/// pane and doubles as the liveness heartbeat.
pub const METADATA_SOURCE: &str = "herdr-aa-notes";

/// Unix seconds now — the heartbeat clock for the pane identity token.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Preview,
    Edit,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Preview => "preview",
            Mode::Edit => "edit",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Note {
    /// Raw markdown of the single note.
    pub text: String,
    pub mode: Mode,
}

/// Platform config base (`%APPDATA%` / `$XDG_CONFIG_HOME` / `~/.config`),
/// same convention as the sidebar plugin's `aa-sidebar.json`. All path logic
/// below takes this as a parameter so tests can inject a temp dir.
fn config_base() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base
}

/// The workspace id herdr injects into every managed pane; the per-workspace
/// note key. Empty = unset (running outside herdr).
fn workspace_env() -> Option<String> {
    std::env::var("HERDR_WORKSPACE_ID").ok().filter(|id| !id.is_empty())
}

/// True when the workspace id is safe to embed in a filename. Stricter than
/// launch.rs's flag-safe check (which also admits `:` `.` `_` `-`): real ids
/// are plain alphanumeric ("w6"), and anything else — separators, dots,
/// anything path-traversal-shaped — falls back to the legacy path instead.
fn is_filename_safe(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Pre-per-workspace single-note file; also the fallback when no (safe)
/// workspace id is available.
fn legacy_path_in(base: &Path) -> PathBuf {
    base.join("herdr").join("aa-notes.json")
}

/// The note-FILE identity of a workspace id: `Some(key)` when the id gets its
/// own per-workspace file, `None` when it falls back to the shared legacy
/// `aa-notes.json`. Panes whose keys are EQUAL load and save the SAME file.
/// This is the identity the launcher's duplicate-instance guard (launch.rs)
/// compares — never raw workspace ids — so the guard can't drift from the
/// on-disk layout: unsafe/missing ids all coarsen to one legacy file, and on
/// Windows ASCII case is folded because NTFS filenames are case-insensitive
/// ("W6.json" and "w6.json" are one file).
pub fn note_key(workspace_id: Option<&str>) -> Option<String> {
    let id = workspace_id.filter(|id| is_filename_safe(id))?;
    #[cfg(windows)]
    let key = id.to_ascii_lowercase();
    #[cfg(not(windows))]
    let key = id.to_string();
    Some(key)
}

/// Pure path selection: `<base>/herdr/aa-notes/<note-key>.json` for a
/// filename-safe id, the legacy `<base>/herdr/aa-notes.json` otherwise.
/// Built from [`note_key`] so path identity and guard identity always agree.
fn state_path_in(base: &Path, workspace_id: Option<&str>) -> PathBuf {
    match note_key(workspace_id) {
        Some(key) => base.join("herdr").join("aa-notes").join(format!("{key}.json")),
        None => legacy_path_in(base),
    }
}

/// State file location for THIS process (env-derived base + workspace id).
pub fn state_path() -> Option<PathBuf> {
    Some(state_path_in(&config_base()?, workspace_env().as_deref()))
}

pub fn load() -> Note {
    let Some(base) = config_base() else { return Note::default() };
    load_in(&base, workspace_env().as_deref())
}

/// Load with one-time migration: when the per-workspace file does not exist
/// yet but the legacy single-note file does, MOVE the legacy file into this
/// workspace's slot — the first workspace to open notes inherits the old note.
/// If the rename fails the legacy file is read in place (not moved); when both
/// files exist the per-workspace one wins and the legacy file is untouched.
fn load_in(base: &Path, workspace_id: Option<&str>) -> Note {
    let path = state_path_in(base, workspace_id);
    let legacy = legacy_path_in(base);
    if path != legacy && !path.exists() && legacy.exists() {
        let moved = path.parent().is_some_and(|dir| {
            std::fs::create_dir_all(dir).is_ok() && std::fs::rename(&legacy, &path).is_ok()
        });
        if !moved {
            return read_note(&legacy);
        }
    }
    read_note(&path)
}

fn read_note(path: &Path) -> Note {
    std::fs::read_to_string(path).map(|json| parse(&json)).unwrap_or_default()
}

/// Forgiving parse: any missing/garbled field falls back to the default, so a
/// hand-edited or truncated file can never wedge the pane.
pub fn parse(json: &str) -> Note {
    let value: serde_json::Value = match serde_json::from_str(json.trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(_) => return Note::default(),
    };
    let text = value
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let mode = match value.get("mode").and_then(|v| v.as_str()) {
        Some("edit") => Mode::Edit,
        _ => Mode::Preview,
    };
    Note { text, mode }
}

/// The JSON that goes on disk: `{ "text": …, "mode": "preview"|"edit" }`.
pub fn to_json(note: &Note) -> String {
    serde_json::json!({
        "text": note.text,
        "mode": note.mode.name(),
    })
    .to_string()
}

/// Atomic best-effort persist: write a temp file, fsync it, then rename over
/// the real one (std's rename replaces existing files on Windows too). The
/// fsync BEFORE the rename matters: without it a crash or power loss can make
/// the rename durable ahead of the data, leaving an empty/truncated file the
/// forgiving loader would silently turn into an empty note.
pub fn save(note: &Note) {
    let Some(path) = state_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        use std::io::Write;
        f.write_all(to_json(note).as_bytes())?;
        f.sync_all()
    });
    if written.is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_text_and_mode() {
        let note = Note { text: "# one\n\ntwo `lines`\n".into(), mode: Mode::Edit };
        assert_eq!(parse(&to_json(&note)), note);
        let preview = Note { text: String::new(), mode: Mode::Preview };
        assert_eq!(parse(&to_json(&preview)), preview);
    }

    #[test]
    fn corrupt_or_missing_input_falls_back_to_empty_note() {
        assert_eq!(parse("garbage"), Note::default());
        assert_eq!(parse(""), Note::default());
        assert_eq!(parse("{}"), Note::default());
        assert_eq!(parse("{\"text\":123}"), Note::default());
        assert_eq!(parse("{\"text\":\"keep\",\"mode\":7}").text, "keep");
        assert_eq!(Note::default().text, "");
        assert_eq!(Note::default().mode, Mode::Preview);
    }

    #[test]
    fn bom_from_powershell_pipe_is_stripped() {
        let note = Note { text: "hi".into(), mode: Mode::Preview };
        let json = format!("\u{feff}{}", to_json(&note));
        assert_eq!(parse(&json), note);
    }

    #[test]
    fn unknown_mode_falls_back_to_preview() {
        assert_eq!(parse("{\"text\":\"a\",\"mode\":\"bogus\"}").mode, Mode::Preview);
        assert_eq!(parse("{\"text\":\"a\",\"mode\":\"edit\"}").mode, Mode::Edit);
    }

    /// Fresh per-test base dir under the OS temp dir — path logic takes the
    /// base as a parameter precisely so tests never touch the real APPDATA.
    fn temp_base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aa-notes-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("herdr")).unwrap();
        dir
    }

    fn write_note(path: &Path, text: &str) {
        std::fs::write(path, to_json(&Note { text: text.into(), mode: Mode::Preview })).unwrap();
    }

    #[test]
    fn state_path_keys_on_safe_workspace_ids_only() {
        let base = Path::new("base");
        assert_eq!(
            state_path_in(base, Some("w6")),
            base.join("herdr").join("aa-notes").join("w6.json")
        );
        // Unset (outside herdr) and filename-unsafe ids use the legacy path.
        let legacy = legacy_path_in(base);
        assert_eq!(state_path_in(base, None), legacy);
        for bad in ["", "w6:t1", "../evil", "a b", "-w6", "w6.json"] {
            assert_eq!(state_path_in(base, Some(bad)), legacy, "unsafe id {bad:?}");
        }
    }

    #[test]
    fn note_key_mirrors_file_identity() {
        assert_eq!(note_key(Some("w6")), Some("w6".to_string()));
        // Every id without its own file shares ONE key (None = legacy file).
        assert_eq!(note_key(None), None);
        for bad in ["", "w6:t1", "../evil", "a b", "-w6", "w6.json"] {
            assert_eq!(note_key(Some(bad)), None, "unsafe id {bad:?}");
        }
        // NTFS is case-insensitive: "W6" and "w6" hit the same file on
        // Windows, so their keys (and filenames) must fold together there.
        #[cfg(windows)]
        {
            assert_eq!(note_key(Some("W6")), Some("w6".to_string()));
            let base = Path::new("base");
            assert_eq!(state_path_in(base, Some("W6")), state_path_in(base, Some("w6")));
        }
        #[cfg(not(windows))]
        assert_eq!(note_key(Some("W6")), Some("W6".to_string()));
    }

    #[test]
    fn first_load_moves_the_legacy_note_into_the_workspace_slot() {
        let base = temp_base("migrate");
        write_note(&legacy_path_in(&base), "old note");
        assert_eq!(load_in(&base, Some("w6")).text, "old note");
        assert!(!legacy_path_in(&base).exists(), "legacy file was moved, not copied");
        assert!(state_path_in(&base, Some("w6")).exists());
        // Second load reads the migrated file; nothing left to migrate.
        assert_eq!(load_in(&base, Some("w6")).text, "old note");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn per_workspace_file_wins_over_a_lingering_legacy_file() {
        let base = temp_base("both");
        let ws_path = state_path_in(&base, Some("w6"));
        std::fs::create_dir_all(ws_path.parent().unwrap()).unwrap();
        write_note(&ws_path, "mine");
        write_note(&legacy_path_in(&base), "stale");
        assert_eq!(load_in(&base, Some("w6")).text, "mine");
        assert!(legacy_path_in(&base).exists(), "legacy file untouched when both exist");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unset_workspace_id_reads_the_legacy_file_in_place() {
        let base = temp_base("legacy");
        write_note(&legacy_path_in(&base), "global");
        assert_eq!(load_in(&base, None).text, "global");
        assert!(legacy_path_in(&base).exists(), "no migration without a workspace id");
        let _ = std::fs::remove_dir_all(&base);
        // Nothing on disk at all (any key) is still just an empty note.
        let empty = temp_base("empty");
        assert_eq!(load_in(&empty, Some("w9")), Note::default());
        assert_eq!(load_in(&empty, None), Note::default());
        let _ = std::fs::remove_dir_all(&empty);
    }
}
