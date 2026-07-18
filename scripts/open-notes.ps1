# open-notes.ps1 -- Windows launcher for the herdr-aa-notes pane.
#
# Idempotent "launch-or-focus, toggle on repeat", scoped to the FOCUSED
# pane's workspace (each workspace has its own note file; the binary matches
# panes by note-FILE identity, see state.rs note_key):
#   - no Notes pane on that note file       -> open one in the current tab,
#     DOCKED ON THE RIGHT edge (any-tab scope within the workspace: a second
#     live instance on the same note file would clobber it on save)
#   - a Notes pane exists but isn't focused -> focus it
#   - the focused pane IS the Notes pane    -> close it (toggle off)
#   - Notes pane with a stale heartbeat     -> close the corpse, open fresh
#
# Right dock: split the tab's RIGHTMOST pane to the right. `pane split --ratio`
# is the ORIGINAL pane's share, so ~0.7 leaves the new Notes pane ~0.3 on the
# right edge — no `pane swap` needed (unlike the sidebar's left dock).
#
# herdr cannot spawn a relative [[panes]] command on Windows, so the binary is
# spawned BY ABSOLUTE PATH via `pane split` + `pane run`; pane-id / target /
# ratio decisions come from the binary's tested stdin modes
# (--launch-decision / --focused-pane / --open-plan), never ad-hoc parsing.

$ErrorActionPreference = 'Continue'

# PowerShell 5.1 otherwise decodes herdr's UTF-8 JSON with the legacy console
# code page; non-ASCII pane titles or paths would corrupt the JSON.
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $Utf8NoBom
$OutputEncoding = $Utf8NoBom

$HerdrBin = if ($env:HERDR_BIN_PATH) { $env:HERDR_BIN_PATH } else { 'herdr' }

function Strip-Verbatim([string]$p) {
    if ($p -and $p.StartsWith('\\?\')) { return $p.Substring(4) }
    return $p
}
$PluginRoot = Strip-Verbatim (Split-Path -Parent $PSScriptRoot)
$Bin = Join-Path $PluginRoot 'target\release\herdr-aa-notes.exe'

if (-not (Test-Path $Bin)) {
    Write-Error "herdr-aa-notes.exe not found at $Bin -- run 'cargo build --release' in the plugin directory first."
    exit 1
}

# Extract the first `pane_id` from a herdr CLI JSON reply.
function Get-PaneId([string]$json) {
    return ([regex]'"pane_id":"([^"]+)"').Match($json).Groups[1].Value
}

# ALWAYS the GLOBAL pane list — never scoped with `--workspace
# $env:HERDR_WORKSPACE_ID`. Focus is global (exactly one focused pane across
# ALL workspaces) and this shell's spawn-time env id can diverge from the
# focused pane's actual workspace (pane moved between workspaces, action
# invoked under another workspace's env): a scoped list would then omit the
# focused pane entirely, the decision would degrade to OPEN, and a duplicate
# Notes pane would spawn beside the focused workspace's live one — two
# instances clobbering one note file. The binary does the scoping instead:
# --launch-decision keys off the FOCUSED pane's own workspace_id field,
# matching panes by note-FILE identity (state.rs note_key).
$PanesJson = (& $HerdrBin pane list | Out-String)

function Open-Pane {
    # Focused pane = where the user is working; its cwd carries over.
    $fp = ($PanesJson | & $Bin --focused-pane).Trim()
    if (-not $fp) {
        # No focused pane known: best-effort plain split beside the current pane.
        $out = (& $HerdrBin pane split --current --direction right --ratio 0.7 | Out-String)
        $np = Get-PaneId $out
        if ($np) {
            & $HerdrBin pane run $np "& \`"$Bin\`"; exit"
            & $HerdrBin pane rename $np 'Notes' *> $null
        }
        exit 0
    }
    $FocusedId, $FocusedCwd = $fp -split "`t", 2

    # Right-dock plan: rightmost pane of the focused tab + the original-pane share.
    $Target = $FocusedId
    $Ratio = '0.70'
    $plan = ((& $HerdrBin pane layout --pane $FocusedId | Out-String) | & $Bin --open-plan).Trim()
    if ($plan) { $Target, $Ratio = $plan -split "`t", 2 }

    $splitArgs = @('pane', 'split', $Target, '--direction', 'right', '--ratio', $Ratio, '--no-focus')
    if ($FocusedCwd) { $splitArgs += @('--cwd', $FocusedCwd) }
    $out = (& $HerdrBin @splitArgs | Out-String)
    $np = Get-PaneId $out
    if (-not $np) { exit 1 }

    # The split already put the new pane on the right edge — no swap needed.
    # Absolute path via the PowerShell CALL OPERATOR; the `\"` escaping
    # survives PS 5.1's native-arg quote-stripping so spaces in the install
    # path reach herdr intact. The trailing `; exit` closes the pane's shell
    # (and so the pane) the moment the TUI quits — `q` must never strand a
    # dead PowerShell prompt still labeled "Notes".
    & $HerdrBin pane run $np "& \`"$Bin\`"; exit"
    & $HerdrBin pane rename $np 'Notes' *> $null
    # herdr has no focus-by-id; a zoom on/off cycle focuses deterministically.
    & $HerdrBin pane zoom $np --on *> $null
    & $HerdrBin pane zoom $np --off *> $null
    exit 0
}

$Decision = ($PanesJson | & $Bin --launch-decision 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $Decision) { $Decision = 'OPEN' }
$Decision = $Decision.Trim()

if ($Decision -like 'FOCUS *') {
    $PaneId = $Decision.Substring(6)
    & $HerdrBin pane zoom $PaneId --on *> $null
    & $HerdrBin pane zoom $PaneId --off
    exit $LASTEXITCODE
} elseif ($Decision -like 'CLOSE *') {
    $PaneId = $Decision.Substring(6)
    # Graceful save+quit before the close: Esc leaves edit mode (which saves),
    # q quits from preview. `pane close` alone kills the TUI, losing any
    # keystrokes still inside the 2s autosave debounce window.
    & $HerdrBin pane send-keys $PaneId Escape q *> $null
    Start-Sleep -Milliseconds 400
    # The quitting TUI's `; exit` normally closes the pane by itself; this
    # close is cleanup for a wedged TUI and often finds the pane already
    # gone — that is success, not an error.
    & $HerdrBin pane close $PaneId *> $null
    exit 0
} elseif ($Decision -like 'REPLACE *') {
    # Dead pane (stale heartbeat): close the corpse, then dock a fresh one.
    # The Esc+q is a best-effort save in case the pane is alive after all
    # (e.g. just woken from a suspend); harmless on a real corpse.
    $PaneId = $Decision.Substring(8)
    & $HerdrBin pane send-keys $PaneId Escape q *> $null
    Start-Sleep -Milliseconds 400
    & $HerdrBin pane close $PaneId *> $null
    # Re-list panes AFTER the close: the corpse may have been the focused
    # pane, and Open-Pane derives its split target from this snapshot — a
    # stale one made `pane layout`/`pane split` fail with pane_not_found.
    $PanesJson = (& $HerdrBin pane list | Out-String)
    Open-Pane
} else {
    Open-Pane
}
