//! Which key does what.
//!
//! The controller keeps only what it needs to decide, without asking the
//! engine, whether a key belongs to the input method (TSF asks that before
//! it delivers the key). Everything about choosing characters stays in the
//! engine; after every request the controller copies the state it was sent.
//!
//! No Windows calls here, so this is tested with plain `cargo test`.

use crate::protocol::{Engine, Op, Response};

/// Candidates per page; the digit keys 1-9 pick from the page.
pub const PAGE_SIZE: usize = 9;

// Virtual-key codes (US keyboard, which is what Taiwanese keyboards use).
pub mod vk {
    pub const BACK: u32 = 0x08;
    pub const RETURN: u32 = 0x0D;
    pub const SHIFT: u32 = 0x10;
    pub const CONTROL: u32 = 0x11;
    pub const MENU: u32 = 0x12;
    pub const CAPITAL: u32 = 0x14;
    pub const ESCAPE: u32 = 0x1B;
    pub const SPACE: u32 = 0x20;
    pub const PRIOR: u32 = 0x21;
    pub const NEXT: u32 = 0x22;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
    pub const LWIN: u32 = 0x5B;
    pub const RWIN: u32 = 0x5C;
    pub const LSHIFT: u32 = 0xA0;
    pub const RMENU: u32 = 0xA5;
    pub const OEM_1: u32 = 0xBA; // ;
    pub const OEM_COMMA: u32 = 0xBC;
    pub const OEM_MINUS: u32 = 0xBD;
    pub const OEM_PERIOD: u32 = 0xBE;
    pub const OEM_2: u32 = 0xBF; // /
    pub const OEM_4: u32 = 0xDB; // [
    pub const OEM_5: u32 = 0xDC; // \
    pub const OEM_6: u32 = 0xDD; // ]
    pub const OEM_7: u32 = 0xDE; // '
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Key {
    pub vk: u32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub caps: bool,
}

/// What the front end must do with a key.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// False: the application handles the key as if no input method existed.
    pub eaten: bool,
    /// Text to put into the document. The composition ends with it.
    pub commit: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Action {
    Pass,
    Swallow,
    Send(Op),
    OpenCandidates,
    SelectOnPage(usize),
    NextPage,
    PrevPage,
    CloseWindow,
    Commit,
    Punctuation(char),
}

fn is_shift(code: u32) -> bool {
    code == vk::SHIFT || code == vk::LSHIFT || code == vk::LSHIFT + 1
}

fn is_modifier(code: u32) -> bool {
    matches!(code, vk::SHIFT | vk::CONTROL | vk::MENU | vk::CAPITAL | vk::LWIN | vk::RWIN)
        || (vk::LSHIFT..=vk::RMENU).contains(&code)
}

/// The character the Dachen layout knows this key by.
fn layout_char(code: u32) -> Option<char> {
    match code {
        0x30..=0x39 => char::from_u32(code),
        0x41..=0x5A => char::from_u32(code + 0x20),
        vk::SPACE => Some(' '),
        vk::OEM_1 => Some(';'),
        vk::OEM_COMMA => Some(','),
        vk::OEM_PERIOD => Some('.'),
        vk::OEM_2 => Some('/'),
        vk::OEM_MINUS => Some('-'),
        _ => None,
    }
}

fn is_tone(key: char) -> bool {
    matches!(key, ' ' | '3' | '4' | '6' | '7')
}

/// Full-width punctuation, typed with Shift. The plain keys are bopomofo.
fn punctuation(code: u32) -> Option<char> {
    Some(match code {
        vk::OEM_COMMA => '，',
        vk::OEM_PERIOD => '。',
        vk::OEM_2 => '？',
        vk::OEM_1 => '：',
        vk::OEM_5 => '；',
        vk::OEM_7 => '、',
        vk::OEM_4 => '「',
        vk::OEM_6 => '」',
        0x31 => '！',
        0x39 => '（',
        0x30 => '）',
        _ => return None,
    })
}

pub struct Controller<E: Engine> {
    engine: E,
    /// Chinese mode. In English mode every key goes to the application.
    chinese: bool,
    /// False in password fields: no key is handled, nothing is learned.
    enabled: bool,
    /// Shift went down and nothing else has been pressed since.
    shift_alone: bool,
    /// A request failed, so the engine may still hold a composition that
    /// the front end has given up on. Discard it before the next request.
    stale: bool,
    /// The engine has not been told the current `enabled` state yet.
    learning_unsent: bool,
    preedit: String,
    pending: String,
    /// The chosen text without the unfinished syllable.
    text: String,
    cursor: usize,
    candidates: Vec<String>,
    page: usize,
    window_open: bool,
}

impl<E: Engine> Controller<E> {
    pub fn new(engine: E) -> Self {
        Controller {
            engine,
            chinese: true,
            enabled: true,
            shift_alone: false,
            stale: false,
            learning_unsent: false,
            preedit: String::new(),
            pending: String::new(),
            text: String::new(),
            cursor: 0,
            candidates: Vec::new(),
            page: 0,
            window_open: false,
        }
    }

    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    // -- what to show ------------------------------------------------------

    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    /// Candidate cursor, in characters from the start of the preedit.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Where the caret belongs, in UTF-16 units from the start of the
    /// preedit: at the candidate cursor, or at the very end (after the
    /// unfinished syllable) when the cursor is past the last character.
    pub fn caret_units(&self) -> usize {
        if self.cursor >= self.text.chars().count() {
            return self.preedit.encode_utf16().count();
        }
        self.preedit.chars().take(self.cursor).map(char::len_utf16).sum()
    }

    pub fn chinese(&self) -> bool {
        self.chinese
    }

    pub fn composing(&self) -> bool {
        !self.preedit.is_empty()
    }

    /// The candidates to show, with the page number and the page count.
    pub fn candidate_page(&self) -> Option<(&[String], usize, usize)> {
        if !self.window_open {
            return None;
        }
        let start = self.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.candidates.len());
        Some((&self.candidates[start..end], self.page, self.pages()))
    }

    fn pages(&self) -> usize {
        self.candidates.len().div_ceil(PAGE_SIZE)
    }

    // -- deciding ----------------------------------------------------------

    fn classify(&self, key: Key) -> Action {
        if !self.enabled || !self.chinese || is_modifier(key.vk) || key.ctrl || key.alt {
            return Action::Pass;
        }
        let composing = self.composing();
        // Caps Lock means the user wants Latin letters.
        if key.caps && !composing {
            return Action::Pass;
        }
        let idle = if composing { Action::Swallow } else { Action::Pass };

        if self.window_open && !key.shift {
            match key.vk {
                0x31..=0x39 => {
                    let n = (key.vk - 0x31) as usize;
                    let on_page = self.candidate_page().map_or(0, |(items, _, _)| items.len());
                    return if n < on_page { Action::SelectOnPage(n) } else { Action::Swallow };
                }
                vk::NEXT | vk::DOWN | vk::SPACE => return Action::NextPage,
                vk::PRIOR | vk::UP => return Action::PrevPage,
                vk::ESCAPE | vk::RETURN | vk::BACK => return Action::CloseWindow,
                _ => {}
            }
        }

        if key.shift {
            return punctuation(key.vk).map_or(idle, Action::Punctuation);
        }
        if let Some(c) = layout_char(key.vk) {
            let has_pending = !self.pending.is_empty();
            return match c {
                ' ' if has_pending => Action::Send(Op::Key(' ')),
                ' ' if composing => Action::OpenCandidates,
                // A tone with no syllable to attach to.
                c if is_tone(c) && !has_pending => idle,
                c => Action::Send(Op::Key(c)),
            };
        }
        if !composing {
            return Action::Pass;
        }
        match key.vk {
            vk::BACK => Action::Send(Op::Backspace),
            vk::LEFT => Action::Send(Op::Left),
            vk::RIGHT => Action::Send(Op::Right),
            vk::DOWN if self.pending.is_empty() => Action::OpenCandidates,
            vk::RETURN => Action::Commit,
            vk::ESCAPE => Action::Send(Op::Escape),
            // Anything else would reach the application in the middle of a composition.
            _ => Action::Swallow,
        }
    }

    /// Would `key_down` take this key? Makes no request.
    pub fn test_key(&self, key: Key) -> bool {
        self.classify(key) != Action::Pass
    }

    /// True if this key is about to start a new composition, which is the
    /// moment to report the text before the cursor with `set_context`.
    pub fn starts_composition(&self, key: Key) -> bool {
        !self.composing() && matches!(self.classify(key), Action::Send(Op::Key(_)))
    }

    // -- talking to the engine ---------------------------------------------

    /// One request, with the bookkeeping that keeps both sides in step.
    fn exchange(&mut self, op: &Op) -> Option<Response> {
        if self.stale {
            self.engine.request(&Op::Escape)?;
            self.stale = false;
        }
        if self.learning_unsent {
            self.engine.request(&Op::Learning(self.enabled))?;
            self.learning_unsent = false;
        }
        let response = self.engine.request(op)?;
        self.preedit = response.preedit.clone();
        self.pending = response.pending.clone();
        self.text = response.segments.concat();
        self.cursor = response.cursor;
        Some(response)
    }

    fn clear(&mut self) {
        self.preedit.clear();
        self.pending.clear();
        self.text.clear();
        self.cursor = 0;
        self.window_open = false;
    }

    /// The engine did not answer. Give up on the composition, keeping the
    /// text that was already on screen so the user does not lose it.
    fn lost(&mut self) -> Option<String> {
        self.stale = true;
        let text = std::mem::take(&mut self.text);
        self.clear();
        Some(text).filter(|t| !t.is_empty())
    }

    /// End the composition now and return its text, if there is one.
    /// Used when focus moves away, when the mode changes, and before punctuation.
    pub fn finish(&mut self) -> Option<String> {
        if !self.composing() {
            return None;
        }
        self.window_open = false;
        match self.exchange(&Op::Commit) {
            Some(response) => response.commit.filter(|t| !t.is_empty()),
            None => self.lost(),
        }
    }

    /// Report the text before the cursor. Ignored while composing.
    pub fn set_context(&mut self, text: &str) {
        if !self.composing() && self.exchange(&Op::Context(text.to_owned())).is_none() {
            self.stale = true;
        }
    }

    /// Turn the input method off for a password field, and back on after.
    /// A composition in progress is discarded, not committed.
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled == self.enabled {
            return;
        }
        if self.composing() {
            self.stale = true;
            self.clear();
        }
        self.enabled = enabled;
        self.learning_unsent = true;
        // Sent now if the engine is reachable, otherwise with the next request.
        let _ = self.exchange(&Op::State);
    }

    // -- keys --------------------------------------------------------------

    /// Record a key press without acting on it. TSF may ask about a key
    /// (`test_key`) and never deliver it; the Shift tracking still has to see it.
    pub fn note_key_down(&mut self, key: Key) {
        self.shift_alone = is_shift(key.vk) && !key.ctrl && !key.alt;
    }

    pub fn key_down(&mut self, key: Key) -> Outcome {
        self.note_key_down(key);
        let action = self.classify(key);
        if !matches!(action, Action::Pass | Action::Swallow | Action::NextPage | Action::PrevPage) {
            self.window_open = false;
        }
        let was_composing = self.composing();
        let mut outcome = Outcome { eaten: action != Action::Pass, commit: None };
        let sent = match action {
            Action::Pass | Action::Swallow | Action::CloseWindow => Some(()),
            Action::NextPage => {
                self.page = (self.page + 1) % self.pages().max(1);
                Some(())
            }
            Action::PrevPage => {
                self.page = (self.page + self.pages().max(1) - 1) % self.pages().max(1);
                Some(())
            }
            Action::Send(op) => self.exchange(&op).map(drop),
            Action::OpenCandidates => self.exchange(&Op::Candidates).map(|response| {
                if !response.candidates.is_empty() {
                    self.candidates = response.candidates;
                    self.page = 0;
                    self.window_open = true;
                }
            }),
            Action::SelectOnPage(n) => {
                self.exchange(&Op::Select(self.page * PAGE_SIZE + n)).map(drop)
            }
            Action::Commit => self.exchange(&Op::Commit).map(|response| {
                outcome.commit = response.commit.filter(|t| !t.is_empty());
            }),
            Action::Punctuation(mark) => {
                let mut text = self.finish().unwrap_or_default();
                text.push(mark);
                outcome.commit = Some(text);
                Some(())
            }
        };
        if sent.is_none() {
            // Engine unreachable. With nothing composed the key simply goes
            // to the application; otherwise it is dropped, because it was
            // meant for a composition that no longer exists.
            outcome.eaten = was_composing;
            outcome.commit = self.lost();
        }
        outcome
    }

    /// Shift pressed and released on its own switches Chinese/English.
    pub fn key_up(&mut self, key: Key) -> Outcome {
        let mut outcome = Outcome::default();
        if is_shift(key.vk) && std::mem::take(&mut self.shift_alone) && self.enabled {
            self.chinese = !self.chinese;
            outcome.commit = self.finish();
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for the engine: every finished syllable becomes 字.
    #[derive(Default)]
    struct Fake {
        down: bool,
        text: String,
        pending: String,
        log: Vec<Op>,
    }

    impl Engine for Fake {
        fn request(&mut self, op: &Op) -> Option<Response> {
            if self.down {
                return None;
            }
            self.log.push(op.clone());
            let mut response = Response { ok: true, ..Response::default() };
            match op {
                Op::Key(c) if is_tone(*c) => {
                    self.pending.clear();
                    self.text.push('字');
                }
                Op::Key(c) => self.pending.push(*c),
                Op::Backspace => {
                    if self.pending.pop().is_none() {
                        self.text.pop();
                    }
                }
                Op::Candidates => {
                    response.candidates = (0..12).map(|i| format!("c{i}")).collect();
                }
                Op::Commit => response.commit = Some(std::mem::take(&mut self.text)),
                Op::Escape => {
                    self.text.clear();
                    self.pending.clear();
                }
                _ => {}
            }
            response.preedit = format!("{}{}", self.text, self.pending);
            response.pending = self.pending.clone();
            response.segments = self.text.chars().map(String::from).collect();
            response.cursor = self.text.chars().count();
            Some(response)
        }
    }

    fn key(c: char) -> Key {
        let vk = match c {
            ' ' => vk::SPACE,
            ',' => vk::OEM_COMMA,
            c => c.to_ascii_uppercase() as u32,
        };
        Key { vk, ..Key::default() }
    }

    fn code(vk: u32) -> Key {
        Key { vk, ..Key::default() }
    }

    fn typed(keys: &str) -> Controller<Fake> {
        let mut c = Controller::new(Fake::default());
        for k in keys.chars() {
            assert!(c.key_down(key(k)).eaten);
        }
        c
    }

    #[test]
    fn compose_and_commit() {
        let mut c = typed("ji3");
        assert_eq!(c.preedit(), "字");
        let outcome = c.key_down(code(vk::RETURN));
        assert_eq!(outcome, Outcome { eaten: true, commit: Some("字".into()) });
        assert!(!c.composing());
    }

    #[test]
    fn idle_keys_go_to_the_application() {
        let c = Controller::new(Fake::default());
        for vk in [vk::RETURN, vk::SPACE, vk::BACK, vk::LEFT, vk::DOWN, vk::ESCAPE, 0x33] {
            assert!(!c.test_key(code(vk)), "vk {vk:#x}");
        }
        assert!(c.test_key(key('j')));
        assert!(c.starts_composition(key('j')));
    }

    #[test]
    fn keys_during_composition_stay_in_it() {
        let c = typed("ji3");
        assert!(c.test_key(code(0x09))); // Tab is swallowed
        assert!(c.test_key(code(0x33))); // a tone with no pending syllable
        assert!(!c.starts_composition(key('j')));
        // Shortcuts still reach the application.
        assert!(!c.test_key(Key { ctrl: true, ..key('c') }));
    }

    #[test]
    fn space_is_first_tone_then_candidates() {
        let mut c = typed("ji ");
        assert_eq!(c.preedit(), "字");
        assert!(c.candidate_page().is_none());
        c.key_down(key(' '));
        let (items, page, pages) = c.candidate_page().unwrap();
        assert_eq!((items.len(), page, pages), (9, 0, 2));

        c.key_down(key(' ')); // next page
        assert_eq!(c.candidate_page().unwrap().0, ["c9", "c10", "c11"]);
        assert!(c.key_down(key('5')).eaten); // no fifth item on this page
        assert!(c.candidate_page().is_some());
        c.key_down(key('2'));
        assert!(c.candidate_page().is_none());
        assert_eq!(c.engine_mut().log.last(), Some(&Op::Select(10)));
    }

    #[test]
    fn shift_alone_toggles_and_commits() {
        let mut c = typed("ji3");
        c.key_down(code(vk::SHIFT));
        let outcome = c.key_up(code(vk::SHIFT));
        assert_eq!(outcome, Outcome { eaten: false, commit: Some("字".into()) });
        assert!(!c.chinese());
        assert!(!c.test_key(key('j')));
        c.key_down(code(vk::SHIFT));
        c.key_up(code(vk::SHIFT));
        assert!(c.chinese());
    }

    #[test]
    fn shift_punctuation_commits_first() {
        let mut c = typed("ji3");
        c.key_down(code(vk::SHIFT));
        let outcome = c.key_down(Key { shift: true, ..key(',') });
        assert_eq!(outcome, Outcome { eaten: true, commit: Some("字，".into()) });
        c.key_up(code(vk::SHIFT));
        assert!(c.chinese(), "shift used with another key must not toggle");
        // Shift with a letter is left to the application when idle.
        assert!(!c.test_key(Key { shift: true, ..key('a') }));
    }

    #[test]
    fn engine_down_while_idle_passes_the_key() {
        let mut c = Controller::new(Fake { down: true, ..Fake::default() });
        assert_eq!(c.key_down(key('j')), Outcome { eaten: false, commit: None });
    }

    #[test]
    fn engine_down_while_composing_keeps_the_text() {
        let mut c = typed("ji3j");
        c.engine_mut().down = true;
        let outcome = c.key_down(key('i'));
        assert_eq!(outcome, Outcome { eaten: true, commit: Some("字".into()) });
        assert!(!c.composing());

        // Back up: the abandoned composition is discarded before anything else.
        c.engine_mut().down = false;
        c.engine_mut().log.clear();
        c.key_down(key('j'));
        assert_eq!(c.engine_mut().log, [Op::Escape, Op::Key('j')]);
        assert_eq!(c.preedit(), "j");
    }

    #[test]
    fn password_field() {
        let mut c = typed("ji3j");
        c.set_enabled(false);
        assert!(!c.composing());
        assert!(!c.test_key(key('j')));
        let log = &c.engine_mut().log;
        assert_eq!(log[log.len() - 3..], [Op::Escape, Op::Learning(false), Op::State]);
        c.key_down(code(vk::SHIFT));
        c.key_up(code(vk::SHIFT));
        assert!(c.chinese(), "no mode switch inside a password field");
        c.set_enabled(true);
        assert!(c.engine_mut().log.contains(&Op::Learning(true)));
        assert!(c.test_key(key('j')));
    }
}
