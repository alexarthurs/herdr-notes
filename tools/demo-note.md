# Sprint notes

Quick thoughts while the agents run. Everything here **autosaves**
and survives restarts — close the pane, reboot, it comes back.

---

## Today

- [x] Wire up the release pipeline
- [x] Fix the flaky auth test
- [ ] Review Codex's refactor of `routes.rs`
- [ ] Ship 0.4.0

## Snippets

Rebuild and relink the plugin after changes:

```
cargo build --release
herdr plugin link .
```

> Ratios clamp at 0.1 — a pane can never get narrower
> than 10% of the tab. Stop fighting it.

## Ideas

1. Pin common commands to the top of the note
2. *Maybe* a search key — `/` feels natural
3. Theme the headings to match the terminal

## Parking lot

- The `pane swap` focus quirk follows the slot, not the pane
- `send-keys` has no PageDown — that is why `g`/`G` exist
- Ask about a global scratch note vs per-workspace
