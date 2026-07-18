# herdr-aa-notes

A single herdr plugin: a persistent markdown notes pane (single scrollable note,
preview/edit modes). Standalone Rust crate — the repo root IS the plugin root
(`herdr plugin link .` from here).

**Living doc**: when you discover a non-obvious herdr/Windows/TUI behavior the hard
way, record it in the Gotchas section below before finishing the task. The fuller
findings doc (and the reference implementation, `herdr-aa-sidebar`) lives in
`C:/Users/Alex/Projects/herdr/CLAUDE.md` — read it before deep herdr integration work.

## Layout

- `src/main.rs` — event loop (500ms poll); `--launch-decision` / `--focused-pane` /
  `--open-plan` stdin modes used by the launcher scripts
- `src/app.rs` — App state: preview/edit, clear-confirm overlay, 2s debounced
  autosave, 5s heartbeat, scrollbars
- `src/markdown.rs` — hand-rolled renderer (headings, lists, checkboxes, quotes,
  code, bold/italic, hr) + display-width wrapping
- `src/state.rs` — `{text, mode}` JSON at `%APPDATA%\herdr\aa-notes.json`
  (unix: `$XDG_CONFIG_HOME|~/.config/herdr/`); forgiving parse, atomic save
  (temp + `sync_all` + rename)
- `src/launch.rs` — OPEN/FOCUS/CLOSE/REPLACE toggle decisions (20s stale heartbeat
  → REPLACE); prefers a same-tab Notes pane but matches any tab so a second
  instance is never spawned (two live instances = last-writer-wins data loss)
- `src/ipc.rs` — socket client: named pipe `\\.\pipe\<HERDR_SOCKET_PATH>` on
  Windows, unix socket elsewhere; one NDJSON request per connection
- `scripts/open-notes.ps1` / `open-notes.sh` — toggle launchers (right-dock);
  Windows entry goes through the inline-powershell action in `herdr-plugin.toml`

## Build / test / lint

```
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

All three must pass before shipping. `cargo build --release` fails with os error 5
while the TUI is running in a pane — quit/close the pane first (and
`Get-Process herdr-aa-notes | Stop-Process` for stragglers).

## Plugin dev workflow

- `herdr plugin link .` registers this checkout; `herdr plugin list --json` shows it.
- Open/toggle: `herdr plugin action invoke herdr-aa-notes.open-notes-windows`
  (unix: `herdr-aa-notes.open-notes`).
- `herdr plugin log list --plugin herdr-aa-notes` shows action/spawn logs.
- After a rebuild, close any open Notes pane and re-invoke the action (stale panes
  keep the old binary).
- End-to-end verification: drive the real binary in a throwaway pane —
  `herdr pane split` + `pane run` + `pane send-keys` + `pane read --source visible`,
  then check `%APPDATA%\herdr\aa-notes.json`. Cheap, catches what unit tests can't.

## Gotchas (verified against herdr 0.7.1)

Inherited from the sidebar plugin's findings:

- Windows herdr can NOT spawn a relative `[[panes]]` command (resolves against
  herdr's own dir) — Windows launches go through the action's inline powershell,
  which locates the plugin root via `herdr plugin list --json` (strip the `\\?\`
  prefix) and spawns the exe by absolute path.
- Action ids must be globally unique across platforms — hence the `-windows`
  suffix, both variants gated by the item-level `platforms` key.
- herdr panes run Windows PowerShell 5.1: chain with `;` / `if ($?)`, never `&&`.
  PS 5.1 prepends a UTF-8 BOM when piping into a native exe's stdin — everything
  parsing herdr JSON from stdin strips a leading `\u{feff}` (see `state.rs`/`launch.rs`).
- `pane split --ratio` is the ORIGINAL pane's share (the new pane gets 1 − ratio);
  ratios clamp to a 0.1 floor.
- Metadata token values must be STRINGS (numbers rejected silently); the heartbeat
  token (`herdr-aa-notes` = unix-time string) re-stamps every ~5s so launchers can
  tell a live pane from a corpse (>20s stale → REPLACE).
- Esc must NEVER exit the TUI (only `q` quits); modifier+Enter is indistinguishable
  from plain Enter in herdr panes; avoid emoji with VS16 variation selectors.

Learned building this plugin:

- `herdr pane send-keys` rejects Home/End AND all PageDown/PageUp spellings —
  every scroll action needs a single-char fallback (`g`/`G` here) to stay drivable.
- A `pane list` snapshot goes stale the moment you close a pane: the REPLACE path
  must re-run `pane list` after closing the corpse before deriving split targets,
  or the split targets a dead pane id and the action exits 1.
- `herdr pane close` kills the process with no signal — a dirty debounce buffer is
  lost. Launcher CLOSE/REPLACE paths first send `pane send-keys <id> Escape q`
  (graceful save-and-quit from any mode), sleep ~400ms, then close as cleanup.
- Heartbeat/autosave must run every event-loop iteration, not only on poll timeout:
  sustained input (<500ms gaps — auto-repeat, long paste) otherwise starves them
  until the launcher declares the live pane stale and REPLACEs it mid-edit.
- crossterm on Windows reports AltGr as CONTROL|ALT — treat CONTROL|ALT chars as
  text insertion or AltGr layouts can't type `@ { [ ] } \`.
- Wrap and horizontal cursor math must budget by display columns (unicode-width),
  not char count — CJK/emoji are double-width and get clipped otherwise.
