#!/usr/bin/env bash
# open-notes.sh — unix launcher for the herdr-aa-notes pane.
#
# Idempotent "launch-or-focus, toggle on repeat":
#   - no Notes pane anywhere                -> open one in the current tab,
#     DOCKED ON THE RIGHT edge (any-tab scope: the note is one global document,
#     a second live instance would clobber it on save)
#   - a Notes pane exists but isn't focused -> focus it
#   - the focused pane IS the Notes pane    -> close it (toggle off)
#   - Notes pane with a stale heartbeat     -> close the corpse, open fresh
#
# Right dock: split the tab's RIGHTMOST pane to the right. `pane split --ratio`
# is the ORIGINAL pane's share, so ~0.7 leaves the new Notes pane ~0.3 on the
# right edge — no `pane swap` needed.
#
# All ids/ratios come from the binary's unit-tested stdin modes
# (--launch-decision / --focused-pane / --open-plan), never ad-hoc JSON
# parsing; the ids it emits are validated flag-safe before reaching an argv.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
bin="$script_dir/../target/release/herdr-aa-notes"

# Without the binary there is no decision logic; fall back to herdr's
# declarative pane open (right split — degraded but functional).
if [ ! -x "$bin" ]; then
  exec "$herdr_bin" plugin pane open \
    --plugin herdr-aa-notes \
    --entrypoint notes \
    --placement split \
    --direction right \
    --focus
fi

panes="$("$herdr_bin" pane list 2>/dev/null || true)"

open_pane() {
  local fp fid fcwd plan target ratio out np
  fp="$(printf '%s' "$panes" | "$bin" --focused-pane 2>/dev/null || true)"
  fid="${fp%%	*}"
  fcwd="${fp#*	}"
  if [ -z "$fid" ]; then
    exec "$herdr_bin" plugin pane open --plugin herdr-aa-notes \
      --entrypoint notes --placement split --direction right --focus
  fi

  target="$fid"
  ratio="0.70"
  plan="$("$herdr_bin" pane layout --pane "$fid" 2>/dev/null | "$bin" --open-plan 2>/dev/null || true)"
  if [ -n "$plan" ]; then
    target="${plan%%	*}"
    ratio="${plan#*	}"
  fi

  out="$("$herdr_bin" pane split "$target" --direction right --ratio "$ratio" \
    ${fcwd:+--cwd "$fcwd"} --no-focus 2>/dev/null || true)"
  np="$(printf '%s' "$out" | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p' | head -n1)"
  [ -n "$np" ] || exit 1

  # The split already put the new pane on the right edge — no swap needed.
  "$herdr_bin" pane run "$np" "exec \"$bin\""
  "$herdr_bin" pane rename "$np" "Notes" >/dev/null 2>&1 || true
  # herdr has no focus-by-id; a zoom on/off cycle focuses deterministically.
  "$herdr_bin" pane zoom "$np" --on >/dev/null 2>&1 || true
  exec "$herdr_bin" pane zoom "$np" --off
}

decision="OPEN"
if [ -n "$panes" ]; then
  decision="$(printf '%s' "$panes" | "$bin" --launch-decision 2>/dev/null || echo OPEN)"
fi

case "$decision" in
  "FOCUS "*)
    pid="${decision#FOCUS }"
    "$herdr_bin" pane zoom "$pid" --on >/dev/null 2>&1 || true
    exec "$herdr_bin" pane zoom "$pid" --off
    ;;
  "CLOSE "*)
    pid="${decision#CLOSE }"
    # Graceful save+quit before the close: Esc leaves edit mode (which saves),
    # q quits from preview. `pane close` alone kills the TUI, losing any
    # keystrokes still inside the 2s autosave debounce window.
    "$herdr_bin" pane send-keys "$pid" Escape q >/dev/null 2>&1 || true
    sleep 0.4
    exec "$herdr_bin" pane close "$pid"
    ;;
  "REPLACE "*)
    # Dead pane (stale heartbeat): close the corpse, then dock a fresh one.
    # The Esc+q is a best-effort save in case the pane is alive after all
    # (e.g. just woken from a suspend); harmless on a real corpse.
    pid="${decision#REPLACE }"
    "$herdr_bin" pane send-keys "$pid" Escape q >/dev/null 2>&1 || true
    sleep 0.4
    "$herdr_bin" pane close "$pid" >/dev/null 2>&1 || true
    # Re-list panes AFTER the close: the corpse may have been the focused
    # pane, and open_pane derives its split target from this snapshot — a
    # stale one made `pane layout`/`pane split` fail with pane_not_found.
    panes="$("$herdr_bin" pane list 2>/dev/null || true)"
    open_pane
    ;;
  *)
    open_pane
    ;;
esac
