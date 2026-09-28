//! The text-editing core shared by every editable widget that is not bound to a
//! `Signal<String>` field (search, combo filters, inline rename, numeric entry, the
//! command palette, the multiline editor): grapheme-correct cursor movement, word jumps,
//! selection, field-local undo/redo, clipboard, and IME pre-edit that never reaches the
//! text before `Commit` (§21.7, §21.9).

use unicode_segmentation::UnicodeSegmentation;

use crate::clipboard::Clipboard;
use crate::geom::{Corners, Point, Rect};
use crate::input::{ImeEvent, KeyCode, KeyEvent};
use crate::render::Primitive;
use crate::style::{ColorRole, PairKind};
use crate::text::TextStyle;
use crate::widget::PaintCx;

const UNDO_LIMIT: usize = 200;

/// What a key did.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EditOutcome {
    /// Not an editing key: let it bubble.
    Ignored,
    /// The cursor or selection moved.
    Moved,
    /// The text changed.
    Changed,
    /// Enter (single line).
    Submit,
    /// Escape.
    Cancel,
}

impl EditOutcome {
    pub fn handled(self) -> bool {
        self != EditOutcome::Ignored
    }
}

/// An editable string with a cursor, a selection anchor, undo and an IME pre-edit.
#[derive(Clone, Debug, Default)]
pub struct LineEdit {
    pub text: String,
    /// Byte index of the caret.
    pub cursor: usize,
    /// Byte index of the selection's other end (== `cursor`: no selection).
    pub anchor: usize,
    pub preedit: Option<(String, Option<(usize, usize)>)>,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    /// Enter inserts a newline; Up/Down are left to the widget (it knows the layout).
    pub multiline: bool,
    /// A password: shown masked, never copied or cut.
    pub masked: bool,
}

impl LineEdit {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
            anchor: text.len(),
            ..Self::default()
        }
    }

    /// Replace the text (not undoable), caret at the end.
    pub fn set_text(&mut self, s: &str) {
        self.text = s.to_string();
        self.cursor = s.len();
        self.anchor = s.len();
        self.preedit = None;
        self.undo.clear();
        self.redo.clear();
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }
    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    fn fix(&self, i: usize) -> usize {
        let mut i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// Move the caret (and the anchor unless extending the selection).
    pub fn set_cursor(&mut self, i: usize, extend: bool) {
        self.cursor = self.fix(i);
        if !extend {
            self.anchor = self.cursor;
        }
    }

    fn prev_boundary(&self, i: usize) -> usize {
        self.text[..i]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(p, _)| p)
    }
    fn next_boundary(&self, i: usize) -> usize {
        self.text[i..]
            .graphemes(true)
            .next()
            .map_or(self.text.len(), |g| i + g.len())
    }
    fn prev_word(&self, i: usize) -> usize {
        let mut last = 0;
        for (p, w) in self.text[..i].split_word_bound_indices() {
            if !w.trim().is_empty() {
                last = p;
            }
        }
        last
    }
    fn next_word(&self, i: usize) -> usize {
        for (p, w) in self.text[i..].split_word_bound_indices() {
            let end = i + p + w.len();
            if !w.trim().is_empty() && end > i {
                return end;
            }
        }
        self.text.len()
    }

    fn push_undo(&mut self) {
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Replace the selection with `s` (one undo step).
    pub fn insert(&mut self, s: &str) {
        let s: String = if self.multiline {
            s.replace("\r\n", "\n")
        } else {
            s.chars().filter(|c| *c != '\n' && *c != '\r').collect()
        };
        self.cursor = self.fix(self.cursor);
        self.anchor = self.fix(self.anchor);
        self.push_undo();
        let (a, b) = self.selection();
        self.text.replace_range(a..b, &s);
        self.cursor = a + s.len();
        self.anchor = self.cursor;
    }

    fn delete(&mut self, forward: bool, word: bool) -> bool {
        let (mut a, mut b) = self.selection();
        if a == b {
            if forward {
                b = if word {
                    self.next_word(b)
                } else {
                    self.next_boundary(b)
                };
            } else {
                a = if word {
                    self.prev_word(a)
                } else {
                    self.prev_boundary(a)
                };
            }
        }
        if a == b {
            return false;
        }
        self.push_undo();
        self.text.replace_range(a..b, "");
        self.cursor = a;
        self.anchor = a;
        true
    }

    /// Handle a key press.
    pub fn key(&mut self, k: &KeyEvent, clip: &mut dyn Clipboard) -> EditOutcome {
        if self.preedit.is_some() {
            return EditOutcome::Moved; // the IME owns the keyboard while composing
        }
        self.cursor = self.fix(self.cursor);
        self.anchor = self.fix(self.anchor);
        let m = k.mods;
        let word = m.ctrl || m.alt;
        match &k.code {
            KeyCode::Char('a') if m.ctrl => {
                self.select_all();
                EditOutcome::Moved
            }
            KeyCode::Char('c') | KeyCode::Char('x') if m.ctrl => {
                let (a, b) = self.selection();
                if a == b || self.masked {
                    return EditOutcome::Moved;
                }
                clip.set_text(&self.text[a..b]);
                if k.code == KeyCode::Char('x') && self.delete(false, false) {
                    return EditOutcome::Changed;
                }
                EditOutcome::Moved
            }
            KeyCode::Char('v') if m.ctrl => match clip.get_text() {
                Some(s) => {
                    self.insert(&s);
                    EditOutcome::Changed
                }
                None => EditOutcome::Moved,
            },
            KeyCode::Char('z') if m.ctrl && !m.shift => match self.undo.pop() {
                Some((t, c)) => {
                    self.redo.push((self.text.clone(), self.cursor));
                    self.text = t;
                    self.set_cursor(c, false);
                    EditOutcome::Changed
                }
                // Nothing to undo here: the key bubbles to the global (project) undo.
                None => EditOutcome::Ignored,
            },
            KeyCode::Char('y') | KeyCode::Char('z') if m.ctrl => match self.redo.pop() {
                Some((t, c)) => {
                    self.undo.push((self.text.clone(), self.cursor));
                    self.text = t;
                    self.set_cursor(c, false);
                    EditOutcome::Changed
                }
                None => EditOutcome::Ignored,
            },
            KeyCode::Backspace => {
                if self.delete(false, word) {
                    EditOutcome::Changed
                } else {
                    EditOutcome::Moved
                }
            }
            KeyCode::Delete => {
                if self.delete(true, word) {
                    EditOutcome::Changed
                } else {
                    EditOutcome::Moved
                }
            }
            KeyCode::Left => {
                let (a, _) = self.selection();
                let to = if self.has_selection() && !m.shift {
                    a
                } else if word {
                    self.prev_word(self.cursor)
                } else {
                    self.prev_boundary(self.cursor)
                };
                self.set_cursor(to, m.shift);
                EditOutcome::Moved
            }
            KeyCode::Right => {
                let (_, b) = self.selection();
                let to = if self.has_selection() && !m.shift {
                    b
                } else if word {
                    self.next_word(self.cursor)
                } else {
                    self.next_boundary(self.cursor)
                };
                self.set_cursor(to, m.shift);
                EditOutcome::Moved
            }
            KeyCode::Home if !self.multiline || m.ctrl => {
                self.set_cursor(0, m.shift);
                EditOutcome::Moved
            }
            KeyCode::End if !self.multiline || m.ctrl => {
                let e = self.text.len();
                self.set_cursor(e, m.shift);
                EditOutcome::Moved
            }
            KeyCode::Home => {
                let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
                self.set_cursor(start, m.shift);
                EditOutcome::Moved
            }
            KeyCode::End => {
                let end = self.text[self.cursor..]
                    .find('\n')
                    .map_or(self.text.len(), |i| self.cursor + i);
                self.set_cursor(end, m.shift);
                EditOutcome::Moved
            }
            KeyCode::Enter if self.multiline && !m.ctrl => {
                self.insert("\n");
                EditOutcome::Changed
            }
            KeyCode::Enter => EditOutcome::Submit,
            KeyCode::Escape => EditOutcome::Cancel,
            _ => match &k.text {
                Some(t) if !m.ctrl && !m.alt && !t.chars().any(char::is_control) => {
                    let t = t.clone();
                    self.insert(&t);
                    EditOutcome::Changed
                }
                _ => EditOutcome::Ignored,
            },
        }
    }

    /// Handle an IME event: pre-edit is shown, never written; `Commit` inserts once.
    pub fn ime(&mut self, ev: &ImeEvent) -> EditOutcome {
        match ev {
            ImeEvent::Preedit { text, cursor } => {
                self.preedit = (!text.is_empty()).then(|| (text.clone(), *cursor));
                EditOutcome::Moved
            }
            ImeEvent::Commit(s) => {
                self.preedit = None;
                self.insert(s);
                EditOutcome::Changed
            }
            ImeEvent::Enabled | ImeEvent::Disabled => {
                self.preedit = None;
                EditOutcome::Moved
            }
        }
    }

    /// The text as drawn (masked, with the pre-edit spliced in) and where the pre-edit
    /// starts in it.
    pub fn display(&self) -> (String, usize) {
        let base = if self.masked {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        };
        let map = |i: usize| {
            if self.masked {
                self.text[..i.min(self.text.len())].chars().count() * '•'.len_utf8()
            } else {
                i
            }
        };
        match &self.preedit {
            Some((p, _)) => {
                let mut s = base;
                let at = map(self.cursor).min(s.len());
                s.insert_str(at, p);
                (s, at)
            }
            None => (base, map(self.cursor)),
        }
    }

    /// The caret's index in [`LineEdit::display`].
    pub fn display_caret(&self) -> usize {
        let (_, at) = self.display();
        match &self.preedit {
            Some((p, c)) => at + c.map_or(p.len(), |(_, e)| e),
            None => at,
        }
    }

    fn display_index(&self, i: usize) -> usize {
        if self.masked {
            self.text[..i.min(self.text.len())].chars().count() * '•'.len_utf8()
        } else {
            i
        }
    }

    /// Byte index in the text for a byte index in the (masked) display.
    pub fn text_index(&self, display_i: usize) -> usize {
        if !self.masked {
            return self.fix(display_i);
        }
        let n = display_i / '•'.len_utf8();
        self.text
            .char_indices()
            .nth(n)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Paint a single-line edit into `inner` (clipped): selection, text, pre-edit
    /// underline and caret. `bg` is the field's background token.
    #[allow(clippy::too_many_arguments)] // each is a distinct paint input; a struct would only rename them
    pub fn paint_line(
        &self,
        cx: &mut PaintCx,
        inner: Rect,
        style: &TextStyle,
        focused: bool,
        caret_on: bool,
        placeholder: &str,
        bg: ColorRole,
    ) {
        cx.primitive(Primitive::PushClip {
            rect: inner,
            radii: Corners::ZERO,
        });
        let (disp, pre_at) = self.display();
        let (run, size) = cx.shape(&disp, style);
        let y = inner.y + (inner.h - size.h) * 0.5;
        if disp.is_empty() {
            if !placeholder.is_empty() {
                cx.text(
                    placeholder,
                    style,
                    Point::new(inner.x, y),
                    ColorRole::FgMuted,
                );
            }
        } else {
            let (a, b) = self.selection();
            if focused && a != b && self.preedit.is_none() {
                let x0 = cx.text_system().caret_x(run, self.display_index(a));
                let x1 = cx.text_system().caret_x(run, self.display_index(b));
                let sel = cx.theme().color(ColorRole::Selection);
                cx.pair(ColorRole::FgPrimary, ColorRole::Selection, PairKind::Text);
                cx.primitive(Primitive::Quad {
                    rect: Rect::new(inner.x + x0, y, x1 - x0, size.h),
                    radii: Corners::ZERO,
                    fill: sel,
                    border: None,
                    shadow: None,
                });
            }
            cx.run(run, Point::new(inner.x, y), ColorRole::FgPrimary);
            if let Some((p, _)) = &self.preedit {
                let x0 = cx.text_system().caret_x(run, pre_at);
                let x1 = cx.text_system().caret_x(run, pre_at + p.len());
                cx.mark(
                    Rect::new(inner.x + x0, y + size.h - 2.0, (x1 - x0).max(1.0), 1.0),
                    ColorRole::FgPrimary,
                    bg,
                    0.0,
                );
            }
        }
        if focused && caret_on {
            let x = cx.text_system().caret_x(run, self.display_caret());
            cx.mark(
                Rect::new(inner.x + x, y, 1.5, size.h.max(style.size)),
                ColorRole::FgPrimary,
                bg,
                0.0,
            );
        }
        cx.primitive(Primitive::PopClip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::InProcessClipboard;
    use crate::input::Modifiers;

    fn key(
        e: &mut LineEdit,
        c: KeyCode,
        m: Modifiers,
        clip: &mut InProcessClipboard,
    ) -> EditOutcome {
        e.key(&KeyEvent::press(c, m), clip)
    }

    #[test]
    fn editing_words_undo_and_clipboard() {
        let mut clip = InProcessClipboard::default();
        let mut e = LineEdit::new("hello brave world");
        key(&mut e, KeyCode::Left, Modifiers::CTRL, &mut clip);
        assert_eq!(e.cursor, 12);
        key(&mut e, KeyCode::Backspace, Modifiers::CTRL, &mut clip);
        assert_eq!(e.text, "hello world");
        key(&mut e, KeyCode::Char('z'), Modifiers::CTRL, &mut clip);
        assert_eq!(e.text, "hello brave world");
        e.select_all();
        key(&mut e, KeyCode::Char('c'), Modifiers::CTRL, &mut clip);
        key(&mut e, KeyCode::End, Modifiers::NONE, &mut clip);
        key(&mut e, KeyCode::Char('v'), Modifiers::CTRL, &mut clip);
        assert_eq!(e.text, "hello brave worldhello brave world");
        assert_eq!(
            key(&mut e, KeyCode::Enter, Modifiers::NONE, &mut clip),
            EditOutcome::Submit
        );
    }

    #[test]
    fn preedit_never_reaches_the_text_and_masks_hide_it() {
        let mut e = LineEdit::new("");
        e.ime(&ImeEvent::Preedit {
            text: "にほん".into(),
            cursor: None,
        });
        assert_eq!(e.text, "");
        e.ime(&ImeEvent::Commit("日本".into()));
        assert_eq!(e.text, "日本");
        let mut p = LineEdit::new("secret");
        p.masked = true;
        assert_eq!(p.display().0, "••••••");
        let mut clip = InProcessClipboard::default();
        p.select_all();
        p.key(
            &KeyEvent::press(KeyCode::Char('c'), Modifiers::CTRL),
            &mut clip,
        );
        assert_eq!(clip.get_text(), None, "a password is never copied");
    }

    #[test]
    fn multiline_enter_and_line_home_end() {
        let mut clip = InProcessClipboard::default();
        let mut e = LineEdit::new("ab");
        e.multiline = true;
        key(&mut e, KeyCode::Enter, Modifiers::NONE, &mut clip);
        e.insert("cd");
        assert_eq!(e.text, "ab\ncd");
        key(&mut e, KeyCode::Home, Modifiers::NONE, &mut clip);
        assert_eq!(e.cursor, 3);
        key(&mut e, KeyCode::Home, Modifiers::CTRL, &mut clip);
        assert_eq!(e.cursor, 0);
    }
}
