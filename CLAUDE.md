# herdr-aa-notes

A single herdr plugin: a persistent markdown notes pane (one scrollable note
per workspace, preview/edit modes). Standalone Rust crate — the repo root IS
the plugin root (`herdr plugin link .` from here).

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
- `src/state.rs` — `{text, mode}` JSON, one file PER WORKSPACE at
  `%APPDATA%\herdr\aa-notes\<workspace-id>.json` (unix:
  `$XDG_CONFIG_HOME|~/.config/herdr/aa-notes/`), keyed by the stable
  `HERDR_WORKSPACE_ID` herdr injects into every pane (survives workspace
  renames; a closed workspace leaves a harmless orphan file). Unset or
  filename-unsafe (non-alphanumeric) id → legacy single-note
  `herdr/aa-notes.json`; first per-workspace load MOVES a lingering legacy
  file into the workspace's slot (read-in-place if the rename fails; the
  per-workspace file wins when both exist). `note_key` exposes the note-FILE
  identity of a workspace id (None = shared legacy file; Windows folds ASCII
  case because NTFS filenames are case-insensitive) — the launcher guard
  compares THESE keys so it can never drift from the on-disk layout.
  Forgiving parse, atomic save (temp + `sync_all` + rename); path logic
  takes an injected base dir so tests never touch the real APPDATA
- `src/launch.rs` — OPEN/FOCUS/CLOSE/REPLACE toggle decisions (20s stale heartbeat
  → REPLACE); prefers a same-tab Notes pane but matches any pane whose
  `note_key` EQUALS the focused pane's, so a second instance on the same note
  file is never spawned (two live instances = last-writer-wins data loss) even
  when different raw workspace ids coarsen to one file (unsafe/missing ids →
  legacy, NTFS case folding); Notes panes on other note files are ignored
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
  then check `%APPDATA%\herdr\aa-notes\<workspace-id>.json` (the pane's
  `HERDR_WORKSPACE_ID`). Cheap, catches what unit tests can't.

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
- Plain `herdr pane list` is GLOBAL — panes from EVERY workspace, exactly one
  `focused` pane in the whole list. The launchers deliberately pass this
  GLOBAL list: scoping with `--workspace $HERDR_WORKSPACE_ID` uses the
  launcher shell's SPAWN-TIME env id, which can diverge from the focused
  pane's actual workspace (pane moved between workspaces, action invoked
  under another workspace's env) — the scoped list then omits the focused
  pane, `--launch-decision` degrades to OPEN, and a duplicate Notes pane
  spawns beside the focused workspace's live one. All scoping happens in the
  binary off each pane's `workspace_id` FIELD, compared by note-file identity
  (`state::note_key`) so the guard matches exactly the panes that share a file.
- `herdr plugin action invoke` runs the action in the GLOBALLY focused
  workspace context, not the invoking pane's. Keybinding use is fine (the
  focused workspace IS the intended one), but a background/scripted invoke
  races with the user switching workspaces: it toggles Notes in — and can
  legacy-migrate a note into — whatever workspace happens to be focused.
  Scripted invocations MUST focus the target workspace first and verify it
  stayed focused.
- A pane created with `pane run "<shell command>"` keeps its shell alive after
  the command exits — quitting the TUI with `q` left a dead PowerShell prompt
  still labeled "Notes". The ps1 launcher appends `; exit` to the pane run
  command (unix `exec`s) so the pane closes itself when the TUI quits; the
  CLOSE paths therefore treat `pane close` as best-effort cleanup (`*> $null`
  / `|| true`, exit 0) because the pane is usually already gone.
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

## README screenshots (Alex's criteria — follow on every reshoot)

The three shots in `docs/media/` (hero / edit / welcome) must show:

- **A 2×2 grid of agents beside the Notes pane: exactly 2 Claude Code + 2
  OpenAI Codex** (mixed diagonally looks best). NO Sidebar/explorer panel in
  any shot — the notes pane and the agents are the subject.
- **The CLI harness graphics must be visible** — Claude Code's logo art +
  version banner, Codex's boxed model/directory banner — with **some text in
  the agents**: type a realistic prompt into a couple of composers via
  `pane send-text` (NOT `pane run` — text must sit unsubmitted so no agent
  actually runs and no tokens burn).
- Exactly ONE title per pane: the border label says "Notes"; the in-app
  header shows only `[preview]`/`[edit]` + scroll position (user-reported
  duplicate — do not reintroduce).
- The note pinned to the TOP (`send-keys <pane> g` immediately before the
  capture — a mouse wheel over the focused pane can scroll it between steps,
  so keep g→capture in ONE command) showing the demo note with headings,
  checkboxes, a code fence, a quote, and the scrollbar visible.

- **Shared dummy backdrop** (agreed with the herdr-aa-sidebar Coordinator
  agent — both repos' screenshots use the SAME roster; keep them in sync):
  herdr's left chrome must show the fictional acme universe, never Alex's
  real projects. Spaces: `acme-app` [main, 1↑ — the real demo repo built by
  the monorepo's `tools/screenshots/setup_demo.sh`], `acme-api` [main],
  `acme-web` [dev], `billing-service` [main] (backdrop cwds are throwaway
  git-init'd temp dirs so branch sublabels render). Agents panel: the four
  visible acme-app grid panes labeled `auth-refactor` (claude),
  `checkout-tests` (codex), `api-docs` (codex), `rate-limiter` (claude),
  plus FAKE background rows `flaky-tests` (codex, working, acme-api),
  `reviewer` (claude, idle, acme-web), `migrations` (codex, working,
  billing-service). Fake rows are reported via the socket API
  `pane.report_agent {pane_id, source, agent, state}` on plain shell panes
  (no CLI spawned, persists over detection); in-universe composer texts:
  "Draft OpenAPI docs for the billing endpoints" (api-docs), "Add a
  sliding-window rate limiter to the gateway" (rate-limiter).
- **Staging happens in an isolated named session**, never Alex's real one:
  `herdr --session shoot server` (headless), then point HERDR_SOCKET_PATH
  at `%APPDATA%\herdr\sessions\shoot\herdr.sock` for every CLI/RPC call.
  Display window: a separate WT window running a script that CLEARS the
  inherited HERDR_* env (herdr refuses "nested" otherwise) then
  `herdr session attach shoot`; find/resize/capture that window BY TITLE
  (WT is single-process, MainWindowHandle is ambiguous). Helper scripts
  (capture_titled.ps1, resize_titled.ps1, attach_shoot.ps1, herdr_rpc.py —
  JSON params via stdin, PS 5.1 mangles quoted JSON argv) live in the
  monorepo's tools/screenshots/. The shoot session shares the real
  config dir: its w2 note file is `aa-notes\w2.json` — back up/clean up.

Hard constraints learned live:

- **The user's email must never appear**: Claude Code's welcome banner
  includes it when the pane is ≳60 columns wide. Keep agent grid columns
  ≲60 cols. Verify every image before shipping; `blur_region.py` is the
  fallback. (First name "Alex" in the compact banner is acceptable.)
- Procedure/tools: monorepo `tools/screenshots/` — `resize_wt.ps1 1760 996`
  (note the printed "was" size and restore it), stage a `--focus` tab in
  THIS workspace, seed the demo note into `aa-notes\<ws>.json` (backup and
  restore the user's file; read the seed markdown with
  `Get-Content -Raw -Encoding UTF8` or em-dashes mojibake), close any
  existing same-workspace Notes pane first (the launcher would FOCUS it
  instead of opening in the staging tab), `capture.ps1` → `crop.ps1 8 48
  1744 940` → frame via `frame_pil.frame` into `docs/media/`. Keep framed
  titles/filenames stable (hero/edit/welcome).
