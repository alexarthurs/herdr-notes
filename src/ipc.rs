//! Minimal client for herdr's socket API: newline-delimited JSON, one
//! request/response per connection (`{"id":..,"method":"pane.report_metadata",
//! "params":{..}}`).
//!
//! On Windows the socket is a named pipe at `\\.\pipe\<HERDR_SOCKET_PATH>`
//! (herdr feeds the whole path through interprocess' namespaced naming), which
//! a plain `File` can speak. On unix it is an ordinary unix domain socket.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// `HERDR_SOCKET_PATH` (injected into hook/action commands), falling back to
/// herdr's default socket location.
pub fn socket_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HERDR_SOCKET_PATH") {
        return Some(path.into());
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("herdr").join("herdr.sock"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Send one request; return the raw response line. Errors are for the caller
/// to ignore — running outside herdr must keep working.
pub fn call_text(method: &str, params: serde_json::Value) -> std::io::Result<String> {
    let path = socket_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no herdr socket path")
    })?;
    let request = serde_json::json!({
        "id": format!("herdr-notes:{method}"),
        "method": method,
        "params": params,
    });
    roundtrip(&path, &request.to_string())
}

/// Stamp the Notes identity (heartbeat token + title) onto `pane_id` NOW.
/// Two callers: the TUI's periodic heartbeat, and the launcher's `--stamp`
/// mode — which runs synchronously right after `pane split`, BEFORE the TUI
/// process spawns, so a fresh pane is never observable in the
/// label-without-token state that launch.rs replaces as a restart corpse.
pub fn stamp_identity(pane_id: &str) -> std::io::Result<String> {
    // Token value MUST be a string; numbers are rejected as invalid_request.
    let now = crate::state::unix_now().to_string();
    call_text(
        "pane.report_metadata",
        serde_json::json!({
            "pane_id": pane_id,
            "source": crate::state::METADATA_SOURCE,
            "title": crate::state::PANE_LABEL,
            "tokens": { crate::state::METADATA_SOURCE: now },
        }),
    )
}

#[cfg(windows)]
fn roundtrip(path: &std::path::Path, request: &str) -> std::io::Result<String> {
    let pipe = format!(r"\\.\pipe\{}", path.display());
    let stream = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)?;
    exchange(stream, request)
}

#[cfg(unix)]
fn roundtrip(path: &std::path::Path, request: &str) -> std::io::Result<String> {
    let stream = std::os::unix::net::UnixStream::connect(path)?;
    exchange(stream, request)
}

fn exchange<S: std::io::Read + Write>(mut stream: S, request: &str) -> std::io::Result<String> {
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(line)
}
