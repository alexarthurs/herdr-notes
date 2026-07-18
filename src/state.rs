//! Persistent note state: one scrollable markdown note plus the last-active
//! mode, stored as a small JSON file beside herdr's own config
//! (`%APPDATA%\herdr\aa-notes.json` on Windows, `$XDG_CONFIG_HOME/herdr/…`
//! elsewhere) so the note survives computer restarts.
//!
//! Loading is forgiving — a missing, hand-edited, or truncated file falls back
//! to an empty note and never panics. Saving is atomic (temp file + rename)
//! and best-effort: the pane keeps working for the session if persist fails.

use std::path::PathBuf;

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

/// State file location, beside herdr's own config (same convention as the
/// sidebar plugin's `aa-sidebar.json`).
pub fn state_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    Some(base?.join("herdr").join("aa-notes.json"))
}

pub fn load() -> Note {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|json| parse(&json))
        .unwrap_or_default()
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
}
