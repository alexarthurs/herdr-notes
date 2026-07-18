# herdr-aa-notes

**A scratch note that lives beside your agents.** One markdown note in a
dockable [herdr] pane — rendered preview, plain-text editing, and it never
forgets: everything autosaves and survives computer restarts.

<img src="docs/media/hero.png" alt="The Notes pane docked beside a running test suite: rendered markdown with headings, checkboxes, code and quotes" width="900">

## Why

Terminals are where the work happens, and the work generates thoughts:
half-finished todo lists, commands you keep retyping, things to ask about
later. This gives those thoughts a permanent home one keypress away — no
editor window, no stray `notes.txt`, no saving.

- **Rendered markdown** — headings, checkboxes, lists, quotes, code blocks
  and inline styles, drawn natively in the terminal with a scrollbar.
- **Zero-friction editing** — `e` to type, `Esc` to go back. That's it.
- **Actually persistent** — atomic autosaves to a JSON file in herdr's
  config directory. Close the pane, kill the terminal, reboot: it comes back.
- **A polite pane** — one toggle action opens, focuses, or closes it;
  a heartbeat token means a dead pane gets replaced, never duplicated.

## Install

From a checkout of this repo:

```
cargo build --release
herdr plugin link .
```

Or straight from GitHub:

```
herdr plugin install alexarthurs/herdr-aa-notes
```

## Open

One toggle action, scoped to the current tab — it opens the pane docked on
the right edge, focuses it if it's already open, and closes it if it's focused:

```
herdr plugin action invoke herdr-aa-notes.open-notes-windows   # windows
herdr plugin action invoke herdr-aa-notes.open-notes           # linux / macos
```

First run greets you with the keymap:

<img src="docs/media/welcome.png" alt="Empty note showing the built-in key reference" width="900">

## Keys

Preview (default):

| Key | Action |
| --- | --- |
| `e` / `Enter` | edit the note |
| `Up` `Down` `PgUp` `PgDn` | scroll |
| `g` / `G` | jump to top / bottom |
| `x` | clear the note (y/N confirm) |
| `q` | quit |

Edit:

| Key | Action |
| --- | --- |
| `Esc` | back to preview (saves) |
| `Ctrl+S` | save now (autosave runs anyway, ~2s after the last keystroke) |

`Esc` never exits the app.

<img src="docs/media/edit.png" alt="Edit mode: the same note as plain markdown with a block cursor" width="900">

## Persistence

State lives in `%APPDATA%\herdr\aa-notes.json` (Windows) or
`$XDG_CONFIG_HOME/herdr/aa-notes.json` / `~/.config/herdr/aa-notes.json`
(unix): `{ "text": "...", "mode": "preview"|"edit" }`. Saves are atomic
(temp file + fsync + rename) and happen on leaving edit mode, clear, quit,
and debounced while typing. A missing or corrupt file falls back to an
empty note — it never wedges the pane.

## Hacking

`CLAUDE.md` has the build/dev workflow and the hard-won herdr/Windows
gotchas (pane spawning, heartbeats, PowerShell 5.1 quirks). The short
version: `cargo build --release`, `cargo test`,
`cargo clippy --all-targets -- -D warnings`, all green before shipping.

[herdr]: https://github.com/ogulcancelik/herdr
