//! Notes TUI: preview/edit modes over one scrollable markdown note, autosaved
//! to this workspace's note file (see state.rs), heartbeating a pane identity
//! token so the launcher can toggle / focus / replace the pane.
//!
//! There is no manual save workflow — everything autosaves — and the only
//! destructive action (`x`, clear the note) sits behind a y/N confirm.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use unicode_width::UnicodeWidthChar;

use crate::markdown::render_markdown;
use crate::state::{self, METADATA_SOURCE, Mode, Note, PANE_LABEL};

/// Debounce for the edit-mode autosave.
const AUTOSAVE_AFTER: Duration = Duration::from_secs(2);
/// Identity re-stamp interval (launcher stale threshold is 20s).
const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);

/// Shown in preview when the note is empty; doubles as the quick-start help.
const EMPTY_HELP: &str = "(empty note)\n\n  e or Enter        start writing\n  Esc               back to preview (saves)\n  Up/Dn PgUp/PgDn   scroll, g/G top/bottom\n  x                 clear the note (asks first)\n  q                 quit\n\nEverything autosaves and survives restarts.";

pub struct App {
    note: Note,
    /// The note split into lines while editing.
    lines: Vec<String>,
    row: usize,
    col: usize,
    edit_scroll: usize,
    preview_scroll: usize,
    confirm_clear: bool,
    dirty: bool,
    last_edit: Instant,
    /// Body height from the last draw, for PgUp/PgDn and scroll clamping.
    body_height: usize,
    pane_id: Option<String>,
    last_beat: Instant,
    /// Disabled in unit tests so exercising keys never touches disk.
    persist: bool,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self::with_note(state::load(), true);
        app.pane_id = std::env::var("HERDR_PANE_ID").ok().filter(|id| !id.is_empty());
        app.report_tokens();
        app
    }

    fn with_note(note: Note, persist: bool) -> Self {
        let mut app = Self {
            note,
            lines: Vec::new(),
            row: 0,
            col: 0,
            edit_scroll: 0,
            preview_scroll: 0,
            confirm_clear: false,
            dirty: false,
            last_edit: Instant::now(),
            body_height: 20,
            pane_id: None,
            last_beat: Instant::now(),
            persist,
        };
        if app.note.mode == Mode::Edit {
            app.enter_edit();
        }
        app
    }

    // ----- persistence & heartbeat -------------------------------------

    fn save(&self) {
        if self.persist {
            state::save(&self.note);
        }
    }

    /// Copy the edit buffer back into the note.
    fn commit(&mut self) {
        if self.note.mode == Mode::Edit {
            self.note.text = self.lines.join("\n");
        }
    }

    /// Debounced autosave: flush ~2s after the last edit keystroke.
    pub fn maybe_flush(&mut self) {
        if self.dirty && self.last_edit.elapsed() >= AUTOSAVE_AFTER {
            self.commit();
            self.save();
            self.dirty = false;
        }
    }

    /// Final save on the way out.
    pub fn finalize(&mut self) {
        self.commit();
        self.save();
    }

    /// Re-stamp the identity token so the launcher knows this pane is alive.
    /// Cheap (one socket round-trip); the event loop calls this every few
    /// seconds. Silently a no-op outside herdr.
    pub fn heartbeat(&mut self) {
        if self.last_beat.elapsed() < HEARTBEAT_EVERY {
            return;
        }
        self.last_beat = Instant::now();
        self.report_tokens();
    }

    fn report_tokens(&self) {
        let Some(pane_id) = &self.pane_id else { return };
        // Token value MUST be a string; numbers are rejected as invalid_request.
        let now = state::unix_now().to_string();
        let _ = crate::ipc::call_text(
            "pane.report_metadata",
            serde_json::json!({
                "pane_id": pane_id,
                "source": METADATA_SOURCE,
                "title": PANE_LABEL,
                "tokens": { METADATA_SOURCE: now },
            }),
        );
    }

    // ----- keys --------------------------------------------------------

    /// Returns true when the app should quit. Esc NEVER quits — it closes the
    /// confirm overlay or leaves edit mode at most.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        if self.confirm_clear {
            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                self.note.text.clear();
                self.preview_scroll = 0;
                self.save();
            }
            self.confirm_clear = false;
            return false;
        }
        match self.note.mode {
            Mode::Preview => self.on_key_preview(key),
            Mode::Edit => {
                self.on_key_edit(key);
                false
            }
        }
    }

    fn on_key_preview(&mut self, key: KeyEvent) -> bool {
        let page = self.body_height.max(1);
        match key.code {
            KeyCode::Char('q') => {
                self.save();
                return true;
            }
            KeyCode::Char('e') | KeyCode::Enter => self.enter_edit(),
            KeyCode::Up => self.preview_scroll = self.preview_scroll.saturating_sub(1),
            KeyCode::Down => self.preview_scroll = self.preview_scroll.saturating_add(1),
            KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(page),
            KeyCode::PageDown => self.preview_scroll = self.preview_scroll.saturating_add(page),
            // g/G because herdr `pane send-keys` rejects Home/End.
            KeyCode::Home | KeyCode::Char('g') => self.preview_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.preview_scroll = usize::MAX, // clamped in draw
            KeyCode::Char('x') => self.confirm_clear = true,
            _ => {}
        }
        false
    }

    fn enter_edit(&mut self) {
        self.lines = self.note.text.split('\n').map(String::from).collect();
        self.row = 0;
        self.col = 0;
        self.edit_scroll = 0;
        self.note.mode = Mode::Edit;
    }

    fn leave_edit(&mut self) {
        self.commit();
        self.note.mode = Mode::Preview;
        self.dirty = false;
        self.save();
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.last_edit = Instant::now();
    }

    fn on_key_edit(&mut self, key: KeyEvent) {
        // AltGr arrives from Windows as CONTROL|ALT on a plain character
        // (@ { [ ] } \ on German/French/Nordic layouts) — that is text to
        // insert, not a Ctrl shortcut.
        let altgr = key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT);
        if key.modifiers.contains(KeyModifiers::CONTROL) && !altgr {
            if matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S')) {
                self.commit();
                self.save();
                self.dirty = false;
            }
            return;
        }
        let line_len = clen(&self.lines[self.row]);
        match key.code {
            KeyCode::Esc => self.leave_edit(),
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = clen(&self.lines[self.row]);
                }
            }
            KeyCode::Right => {
                if self.col < line_len {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            KeyCode::Up => {
                if self.row > 0 {
                    self.row -= 1;
                    self.col = self.col.min(clen(&self.lines[self.row]));
                }
            }
            KeyCode::Down => {
                if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = self.col.min(clen(&self.lines[self.row]));
                }
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = line_len,
            KeyCode::PageUp => {
                self.row = self.row.saturating_sub(self.body_height.max(1));
                self.col = self.col.min(clen(&self.lines[self.row]));
            }
            KeyCode::PageDown => {
                self.row = (self.row + self.body_height.max(1)).min(self.lines.len() - 1);
                self.col = self.col.min(clen(&self.lines[self.row]));
            }
            KeyCode::Enter => {
                let at = byte_idx(&self.lines[self.row], self.col);
                let rest = self.lines[self.row].split_off(at);
                self.lines.insert(self.row + 1, rest);
                self.row += 1;
                self.col = 0;
                self.touch();
            }
            KeyCode::Backspace => {
                if self.col > 0 {
                    let at = byte_idx(&self.lines[self.row], self.col - 1);
                    self.lines[self.row].remove(at);
                    self.col -= 1;
                    self.touch();
                } else if self.row > 0 {
                    let tail = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = clen(&self.lines[self.row]);
                    self.lines[self.row].push_str(&tail);
                    self.touch();
                }
            }
            KeyCode::Delete => {
                if self.col < line_len {
                    let at = byte_idx(&self.lines[self.row], self.col);
                    self.lines[self.row].remove(at);
                    self.touch();
                } else if self.row + 1 < self.lines.len() {
                    let tail = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&tail);
                    self.touch();
                }
            }
            KeyCode::Tab => {
                let at = byte_idx(&self.lines[self.row], self.col);
                self.lines[self.row].insert_str(at, "  ");
                self.col += 2;
                self.touch();
            }
            KeyCode::Char(c) => {
                let at = byte_idx(&self.lines[self.row], self.col);
                self.lines[self.row].insert(at, c);
                self.col += 1;
                self.touch();
            }
            _ => {}
        }
    }

    // ----- drawing -----------------------------------------------------

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let [title_a, body_a, hint_a] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
                .areas(area);
        self.body_height = usize::from(body_a.height);

        // Body first: the preview reports a scroll hint for the title line.
        let (mode, scroll_hint) = match self.note.mode {
            Mode::Preview => ("preview", self.draw_preview(frame, body_a)),
            Mode::Edit => {
                self.draw_edit(frame, body_a);
                ("edit", None)
            }
        };

        // The pane border already says "Notes" (metadata title) — repeating it
        // here read as a duplicate, so the header carries only mode + scroll.
        let mut title = vec![Span::styled(
            format!(" [{mode}]"),
            Style::default().fg(Color::Cyan),
        )];
        if let Some(hint) = scroll_hint {
            title.push(Span::styled(
                format!("  {hint}"),
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(title)), title_a);

        let hints = match self.note.mode {
            Mode::Preview => " e/Enter edit   Up/Dn scroll   g/G top/end   x clear   q quit",
            Mode::Edit => " Esc preview (saves)   Ctrl+S save",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hints, Style::default().add_modifier(Modifier::DIM))),
            hint_a,
        );

        if self.confirm_clear {
            draw_confirm(frame, area);
        }
    }

    /// Renders the preview body; returns a "top-line/total" scroll hint when
    /// the content overflows the pane.
    fn draw_preview(&mut self, frame: &mut Frame, area: Rect) -> Option<String> {
        if self.note.text.trim().is_empty() {
            self.preview_scroll = 0;
            frame.render_widget(
                Paragraph::new(EMPTY_HELP).style(Style::default().add_modifier(Modifier::DIM)),
                area,
            );
            return None;
        }
        // The rightmost column is reserved for the overflow scrollbar so text
        // never sits underneath it.
        let text_w = usize::from(area.width).saturating_sub(1).max(1);
        let lines = render_markdown(&self.note.text, text_w);
        let total = lines.len();
        let max = total.saturating_sub(usize::from(area.height));
        self.preview_scroll = self.preview_scroll.min(max);
        let scroll = u16::try_from(self.preview_scroll).unwrap_or(u16::MAX);
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
        draw_scrollbar(frame, area, max, self.preview_scroll);
        (max > 0).then(|| format!("{}/{total}", self.preview_scroll + 1))
    }

    fn draw_edit(&mut self, frame: &mut Frame, area: Rect) {
        let h = usize::from(area.height).max(1);
        // Rightmost column reserved for the overflow scrollbar.
        let w = usize::from(area.width).saturating_sub(1).max(2);
        if self.row < self.edit_scroll {
            self.edit_scroll = self.row;
        }
        if self.row >= self.edit_scroll + h {
            self.edit_scroll = self.row + 1 - h;
        }
        // Horizontal shift keeps the cursor visible on overlong lines. The
        // shift is found in display COLUMNS (wide CJK/emoji chars count 2),
        // otherwise the REVERSED cursor cell could sit past the pane edge.
        let widths: Vec<usize> =
            self.lines[self.row].chars().map(|c| c.width().unwrap_or(0)).collect();
        let cursor_w = widths.get(self.col).copied().unwrap_or(1).max(1);
        let mut h_off = 0;
        let mut visible: usize = widths[..self.col].iter().sum();
        while visible + cursor_w > w && h_off < self.col {
            visible -= widths[h_off];
            h_off += 1;
        }
        let mut lines: Vec<Line> = Vec::new();
        for (i, line) in self.lines.iter().enumerate().skip(self.edit_scroll).take(h) {
            let chars: Vec<char> = line.chars().skip(h_off).collect();
            if i == self.row {
                let col = self.col - h_off;
                let before: String = chars.iter().take(col).collect();
                let at: String = chars.get(col).map_or(" ".to_string(), |c| c.to_string());
                let after: String = chars.iter().skip(col + 1).collect();
                lines.push(Line::from(vec![
                    Span::raw(before),
                    Span::styled(at, Style::default().add_modifier(Modifier::REVERSED)),
                    Span::raw(after),
                ]));
            } else {
                lines.push(Line::from(chars.into_iter().collect::<String>()));
            }
        }
        frame.render_widget(Paragraph::new(lines), area);
        draw_scrollbar(frame, area, self.lines.len().saturating_sub(h), self.edit_scroll);
    }
}

/// Vertical scrollbar on the right edge; hidden when everything fits.
fn draw_scrollbar(frame: &mut Frame, area: Rect, max_scroll: usize, position: usize) {
    if max_scroll == 0 {
        return;
    }
    let mut state = ScrollbarState::new(max_scroll).position(position.min(max_scroll));
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight),
        area,
        &mut state,
    );
}

fn draw_confirm(frame: &mut Frame, area: Rect) {
    let w = 30.min(area.width);
    let h = 3.min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(" Clear the note? y/N").block(Block::bordered().title(" Clear ")),
        rect,
    );
}

fn clen(s: &str) -> usize {
    s.chars().count()
}

fn byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app(text: &str) -> App {
        App::with_note(
            Note { text: text.to_string(), mode: Mode::Preview },
            false, // never touch the real state file from tests
        )
    }

    #[test]
    fn edit_roundtrip_insert_newline_backspace() {
        let mut a = app("ab");
        a.on_key(key(KeyCode::Enter)); // enter edit mode
        assert_eq!(a.note.mode, Mode::Edit);
        a.on_key(key(KeyCode::End));
        a.on_key(key(KeyCode::Char('c')));
        a.on_key(key(KeyCode::Enter)); // newline
        a.on_key(key(KeyCode::Char('d')));
        a.on_key(key(KeyCode::Esc)); // back to preview, committing
        assert_eq!(a.note.mode, Mode::Preview);
        assert_eq!(a.note.text, "abc\nd");

        a.on_key(key(KeyCode::Char('e')));
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::End));
        a.on_key(key(KeyCode::Backspace)); // delete 'd'
        a.on_key(key(KeyCode::Backspace)); // join lines
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.note.text, "abc");
    }

    #[test]
    fn esc_never_quits_and_q_quits_only_in_preview() {
        let mut a = app("x");
        assert!(!a.on_key(key(KeyCode::Esc)), "Esc in preview must not quit");
        a.on_key(key(KeyCode::Char('e')));
        assert!(!a.on_key(key(KeyCode::Esc)), "Esc in edit leaves edit, not the app");
        assert_eq!(a.note.mode, Mode::Preview);
        // 'q' typed while editing is just a character.
        a.on_key(key(KeyCode::Char('e')));
        assert!(!a.on_key(key(KeyCode::Char('q'))));
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.note.text, "qx");
        assert!(a.on_key(key(KeyCode::Char('q'))), "q in preview quits");
    }

    #[test]
    fn clear_requires_confirmation() {
        let mut a = app("keep me");
        a.on_key(key(KeyCode::Char('x')));
        assert!(a.confirm_clear);
        assert!(!a.on_key(key(KeyCode::Esc)), "Esc closes the overlay, not the app");
        assert_eq!(a.note.text, "keep me");
        a.on_key(key(KeyCode::Char('x')));
        a.on_key(key(KeyCode::Char('n')));
        assert_eq!(a.note.text, "keep me", "declined clear keeps the note");
        assert!(!a.confirm_clear);
        a.on_key(key(KeyCode::Char('x')));
        a.on_key(key(KeyCode::Char('y')));
        assert_eq!(a.note.text, "", "confirmed clear empties the note");
    }

    #[test]
    fn preview_scroll_keys_move_and_clamp_at_top() {
        let mut a = app("line\nline\nline");
        a.on_key(key(KeyCode::Up));
        assert_eq!(a.preview_scroll, 0, "scrolling above the top clamps");
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Down));
        assert_eq!(a.preview_scroll, 2);
        a.on_key(key(KeyCode::Char('g')));
        assert_eq!(a.preview_scroll, 0);
        a.on_key(key(KeyCode::Char('G')));
        assert_eq!(a.preview_scroll, usize::MAX, "jump to end; draw clamps to content");
    }

    #[test]
    fn altgr_chars_insert_but_ctrl_shortcuts_do_not() {
        let mut a = app("");
        a.on_key(key(KeyCode::Char('e')));
        // AltGr = CONTROL|ALT on Windows: a printable char, must insert.
        a.on_key(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        // Plain Ctrl+char stays a shortcut, never inserts.
        a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.note.text, "@");
    }

    #[test]
    fn debounced_autosave_commits_the_buffer() {
        let mut a = app("");
        a.on_key(key(KeyCode::Char('e')));
        a.on_key(key(KeyCode::Char('z')));
        assert!(a.dirty);
        a.last_edit = Instant::now() - AUTOSAVE_AFTER;
        a.maybe_flush();
        assert!(!a.dirty);
        assert_eq!(a.note.text, "z", "flush committed the edit buffer");
    }

    #[test]
    fn startup_in_edit_mode_loads_the_buffer() {
        let mut a = App::with_note(Note { text: "a\nb".into(), mode: Mode::Edit }, false);
        assert_eq!(a.lines, vec!["a".to_string(), "b".to_string()]);
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.note.text, "a\nb", "leaving edit commits losslessly");
    }
}
