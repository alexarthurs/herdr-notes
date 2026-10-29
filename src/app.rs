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
use crate::state::{self, Mode, Note};

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
        let _ = crate::ipc::stamp_identity(pane_id);
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
        if self.edit_shortcut(key) {
            return;
        }
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

    fn edit_shortcut(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        let alt = key.modifiers == KeyModifiers::ALT;
        let command = key.modifiers == KeyModifiers::SUPER
            || key.modifiers == KeyModifiers::META;
        match key.code {
            KeyCode::Char('a') if ctrl => self.col = 0,
            KeyCode::Left if command => self.col = 0,
            KeyCode::Char('e') if ctrl => self.col = clen(&self.lines[self.row]),
            KeyCode::Right if command => self.col = clen(&self.lines[self.row]),
            KeyCode::Char('u') if ctrl => self.delete_to(self.row, 0),
            KeyCode::Backspace if command => self.delete_to(self.row, 0),
            KeyCode::Char('k') if ctrl => self.delete_to(self.row, clen(&self.lines[self.row])),
            KeyCode::Delete if command => self.delete_to(self.row, clen(&self.lines[self.row])),
            KeyCode::Char('b') if alt => self.move_word(true),
            KeyCode::Left if ctrl || alt => self.move_word(true),
            KeyCode::Char('f') if alt => self.move_word(false),
            KeyCode::Right if ctrl || alt => self.move_word(false),
            KeyCode::Char('w') if ctrl => self.delete_word(true),
            KeyCode::Backspace if ctrl || alt => self.delete_word(true),
            KeyCode::Char('d') if alt => self.delete_word(false),
            KeyCode::Delete if ctrl || alt => self.delete_word(false),
            _ => return false,
        }
        true
    }

    fn word_position(&self, backward: bool) -> (usize, usize) {
        let text = self.lines.join("\n");
        let cursor = self.lines[..self.row].iter().map(|line| clen(line) + 1).sum::<usize>()
            + self.col;
        let boundary = word_boundary(&text, cursor, backward);
        text.chars().take(boundary).fold((0, 0), |(row, col), ch| {
            if ch == '\n' { (row + 1, 0) } else { (row, col + 1) }
        })
    }

    fn move_word(&mut self, backward: bool) {
        (self.row, self.col) = self.word_position(backward);
    }

    fn delete_word(&mut self, backward: bool) {
        let (row, col) = self.word_position(backward);
        self.delete_to(row, col);
    }

    fn delete_to(&mut self, row: usize, col: usize) {
        let cursor = (self.row, self.col);
        let target = (row, col);
        if cursor == target {
            return;
        }
        let (start, end) = if cursor < target { (cursor, target) } else { (target, cursor) };
        let suffix = self.lines[end.0][byte_idx(&self.lines[end.0], end.1)..].to_owned();
        let prefix_end = byte_idx(&self.lines[start.0], start.1);
        self.lines[start.0].truncate(prefix_end);
        self.lines[start.0].push_str(&suffix);
        self.lines.drain(start.0 + 1..end.0 + 1);
        (self.row, self.col) = start;
        self.touch();
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

fn word_boundary(text: &str, cursor: usize, backward: bool) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let class = |ch: char| {
        if ch.is_whitespace() { 0 } else if ch.is_alphanumeric() || ch == '_' { 1 } else { 2 }
    };
    let mut boundary = cursor;
    if backward {
        while boundary > 0 && chars[boundary - 1].is_whitespace() {
            boundary -= 1;
        }
        if boundary > 0 {
            let run = class(chars[boundary - 1]);
            while boundary > 0 && class(chars[boundary - 1]) == run {
                boundary -= 1;
            }
        }
    } else {
        while boundary < chars.len() && chars[boundary].is_whitespace() {
            boundary += 1;
        }
        if boundary < chars.len() {
            let run = class(chars[boundary]);
            while boundary < chars.len() && class(chars[boundary]) == run {
                boundary += 1;
            }
        }
    }
    boundary
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
    fn line_shortcuts_move_within_the_current_line() {
        for (code, modifiers, col) in [
            (KeyCode::Char('a'), KeyModifiers::CONTROL, 0),
            (KeyCode::Left, KeyModifiers::SUPER, 0),
            (KeyCode::Left, KeyModifiers::META, 0),
            (KeyCode::Char('e'), KeyModifiers::CONTROL, 13),
            (KeyCode::Right, KeyModifiers::SUPER, 13),
            (KeyCode::Right, KeyModifiers::META, 13),
        ] {
            let mut a = app("before\none two three\nafter");
            a.enter_edit();
            (a.row, a.col) = (1, 7);
            a.on_key(KeyEvent::new(code, modifiers));
            assert_eq!((a.row, a.col), (1, col), "{code:?} {modifiers:?}");
            assert!(!a.dirty);
            a.leave_edit();
            assert_eq!(a.note.text, "before\none two three\nafter");
        }
    }

    #[test]
    fn word_shortcuts_move_without_inserting_shortcut_letters() {
        for (code, modifiers, col) in [
            (KeyCode::Char('b'), KeyModifiers::ALT, 4),
            (KeyCode::Left, KeyModifiers::ALT, 4),
            (KeyCode::Left, KeyModifiers::CONTROL, 4),
            (KeyCode::Char('f'), KeyModifiers::ALT, 13),
            (KeyCode::Right, KeyModifiers::ALT, 13),
            (KeyCode::Right, KeyModifiers::CONTROL, 13),
        ] {
            let mut a = app("one two three");
            a.enter_edit();
            a.col = 7;
            a.on_key(KeyEvent::new(code, modifiers));
            assert_eq!((a.row, a.col), (0, col), "{code:?} {modifiers:?}");
            assert!(!a.dirty);
            a.leave_edit();
            assert_eq!(a.note.text, "one two three");
        }
    }

    #[test]
    fn deletion_shortcuts_remove_the_requested_range_and_mark_dirty() {
        for (code, modifiers, line, col) in [
            (KeyCode::Char('u'), KeyModifiers::CONTROL, " three", 0),
            (KeyCode::Backspace, KeyModifiers::SUPER, " three", 0),
            (KeyCode::Backspace, KeyModifiers::META, " three", 0),
            (KeyCode::Char('k'), KeyModifiers::CONTROL, "one two", 7),
            (KeyCode::Delete, KeyModifiers::SUPER, "one two", 7),
            (KeyCode::Delete, KeyModifiers::META, "one two", 7),
            (KeyCode::Char('w'), KeyModifiers::CONTROL, "one  three", 4),
            (KeyCode::Backspace, KeyModifiers::ALT, "one  three", 4),
            (KeyCode::Backspace, KeyModifiers::CONTROL, "one  three", 4),
            (KeyCode::Char('d'), KeyModifiers::ALT, "one two", 7),
            (KeyCode::Delete, KeyModifiers::ALT, "one two", 7),
            (KeyCode::Delete, KeyModifiers::CONTROL, "one two", 7),
        ] {
            let mut a = app("before\none two three\nafter");
            a.enter_edit();
            (a.row, a.col) = (1, 7);
            a.on_key(KeyEvent::new(code, modifiers));
            assert_eq!((a.row, a.col), (1, col), "{code:?} {modifiers:?}");
            assert!(a.dirty);
            a.leave_edit();
            assert_eq!(a.note.text, format!("before\n{line}\nafter"));
        }
    }

    #[test]
    fn word_deletion_handles_unicode_and_punctuation() {
        let mut a = app("src/café_name.rs  中文");
        a.enter_edit();
        a.on_key(key(KeyCode::End));
        for expected in ["src/café_name.rs  ", "src/café_name.", "src/café_name", "src/", "src", ""] {
            a.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
            a.commit();
            assert_eq!(a.note.text, expected);
            assert_eq!(a.col, clen(expected));
        }
    }

    #[test]
    fn word_shortcuts_cross_blank_lines_without_losing_the_suffix() {
        let mut a = app("one\n\n  two\nthree");
        a.enter_edit();
        (a.row, a.col) = (2, 2);
        a.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        a.commit();
        assert_eq!(a.note.text, "two\nthree");
        assert_eq!((a.row, a.col), (0, 0));

        let mut a = app("one\n\n  two\nthree");
        a.enter_edit();
        a.col = 3;
        a.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!((a.row, a.col), (2, 5));
        (a.row, a.col) = (0, 3);
        a.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT));
        a.leave_edit();
        assert_eq!(a.note.text, "one\nthree");
    }

    #[test]
    fn deletion_at_document_edges_is_a_noop() {
        let mut a = app("one\n");
        a.enter_edit();
        for (row, col, code, modifiers) in [
            (0, 0, KeyCode::Backspace, KeyModifiers::ALT),
            (0, 0, KeyCode::Char('u'), KeyModifiers::CONTROL),
            (1, 0, KeyCode::Delete, KeyModifiers::ALT),
            (1, 0, KeyCode::Char('k'), KeyModifiers::CONTROL),
        ] {
            (a.row, a.col) = (row, col);
            a.on_key(KeyEvent::new(code, modifiers));
            assert_eq!((a.row, a.col), (row, col));
            assert!(!a.dirty);
        }
        a.leave_edit();
        assert_eq!(a.note.text, "one\n");
    }

    #[test]
    fn altgr_letters_are_text_even_when_they_match_word_shortcuts() {
        let mut a = app("");
        a.enter_edit();
        for ch in ['b', 'f', 'd', '@'] {
            a.on_key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ));
        }
        a.leave_edit();
        assert_eq!(a.note.text, "bfd@");
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
