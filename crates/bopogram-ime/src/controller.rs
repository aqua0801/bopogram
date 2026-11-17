//! Which key does what.
//!
//! The key rules follow the behaviour Microsoft documents for its Bopomofo
//! IME (see the table in `prototype/docs/frontend-handoff.md`), so that its
//! users need not relearn anything.
//!
//! The controller keeps only what it needs to decide, without asking the
//! engine, whether a key belongs to the input method (TSF asks that before
//! it delivers the key). Everything about choosing characters stays in the
//! engine; after every request the controller copies the state it was sent.
//!
//! No Windows calls here, so this is tested with plain `cargo test`.

use crate::protocol::{Engine, Failure, Op, Response};

/// Candidates per page; the digit keys 1-9 pick from the page.
pub const PAGE_SIZE: usize = 9;

pub const CASUAL: &str = "casual";
pub const FORMAL: &str = "formal";

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
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
    pub const DELETE: u32 = 0x2E;
    pub const LWIN: u32 = 0x5B;
    pub const RWIN: u32 = 0x5C;
    pub const NUMPAD0: u32 = 0x60;
    pub const NUMPAD9: u32 = 0x69;
    pub const LSHIFT: u32 = 0xA0;
    pub const RMENU: u32 = 0xA5;
    pub const OEM_1: u32 = 0xBA; // ;
    pub const OEM_PLUS: u32 = 0xBB; // =
    pub const OEM_COMMA: u32 = 0xBC;
    pub const OEM_MINUS: u32 = 0xBD;
    pub const OEM_PERIOD: u32 = 0xBE;
    pub const OEM_2: u32 = 0xBF; // /
    pub const OEM_3: u32 = 0xC0; // `
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
    /// Text to put into the document before the preedit is shown again.
    /// The composition may continue after it (see `Controller::preedit`).
    pub commit: Option<String>,
    /// The key switched between the casual and the formal style.
    pub style_changed: bool,
}

#[derive(Debug, PartialEq)]
enum Action {
    Pass,
    Swallow,
    Send(Op),
    /// The ` key: the next symbol key gives its full-width form.
    StartPrefix,
    OpenCandidates,
    /// Index into the whole candidate list.
    SelectIndex(usize),
    MoveSelection(isize),
    NextPage,
    PrevPage,
    CloseWindow,
    ToggleStyle,
}

fn is_shift(code: u32) -> bool {
    code == vk::SHIFT || code == vk::LSHIFT || code == vk::LSHIFT + 1
}

fn is_modifier(code: u32) -> bool {
    matches!(code, vk::SHIFT | vk::CONTROL | vk::MENU | vk::CAPITAL | vk::LWIN | vk::RWIN)
        || (vk::LSHIFT..=vk::RMENU).contains(&code)
}

/// The ASCII character a key types on a US keyboard, Caps Lock aside.
fn ascii(code: u32, shift: bool) -> Option<char> {
    let pair = |plain: char, shifted: char| Some(if shift { shifted } else { plain });
    match code {
        0x30..=0x39 if shift => ")!@#$%^&*(".chars().nth((code - 0x30) as usize),
        0x30..=0x39 => char::from_u32(code),
        0x41..=0x5A => char::from_u32(if shift { code } else { code + 0x20 }),
        vk::SPACE => Some(' '),
        vk::OEM_1 => pair(';', ':'),
        vk::OEM_PLUS => pair('=', '+'),
        vk::OEM_COMMA => pair(',', '<'),
        vk::OEM_MINUS => pair('-', '_'),
        vk::OEM_PERIOD => pair('.', '>'),
        vk::OEM_2 => pair('/', '?'),
        vk::OEM_3 => pair('`', '~'),
        vk::OEM_4 => pair('[', '{'),
        vk::OEM_5 => pair('\\', '|'),
        vk::OEM_6 => pair(']', '}'),
        vk::OEM_7 => pair('\'', '"'),
        _ => None,
    }
}

/// The character the Dachen layout knows this key by.
fn layout_char(code: u32) -> Option<char> {
    match code {
        0x30..=0x39 | 0x41..=0x5A | vk::SPACE => ascii(code, false),
        vk::OEM_1 | vk::OEM_COMMA | vk::OEM_PERIOD | vk::OEM_2 | vk::OEM_MINUS => {
            ascii(code, false)
        }
        _ => None,
    }
}

fn is_tone(key: char) -> bool {
    matches!(key, ' ' | '3' | '4' | '6' | '7')
}

pub struct Controller<E: Engine> {
    engine: E,
    /// Chinese mode. In English mode every key goes to the application.
    chinese: bool,
    /// False in password fields: no key is handled, nothing is learned.
    enabled: bool,
    /// Shift went down and nothing else has been pressed since.
    shift_alone: bool,
    /// ` was pressed: the next symbol key gives its full-width form.
    prefix: bool,
    /// A request failed, so the engine may still hold a composition that
    /// the front end has given up on. Discard it before the next request.
    stale: bool,
    /// Requests in a row that were sent but not answered in time. While
    /// this is not zero the engine's session is ahead of the state below.
    behind: u32,
    /// The engine has not been told the current `enabled` state yet.
    learning_unsent: bool,
    /// The style the user chose, if known. The engine is told whenever it
    /// reports another one (a new connection starts with its default).
    wanted_style: Option<String>,
    /// The style the engine last reported.
    style: Option<String>,
    preedit: String,
    pending: String,
    /// Insertion point, in characters from the start of the preedit.
    caret: usize,
    /// Text the engine handed over for the document, not yet passed on.
    committed: String,
    candidates: Vec<String>,
    /// Highlighted candidate: an index into the whole list.
    selected: usize,
    window_open: bool,
}

impl<E: Engine> Controller<E> {
    pub fn new(engine: E) -> Self {
        Controller {
            engine,
            chinese: true,
            enabled: true,
            shift_alone: false,
            prefix: false,
            stale: false,
            behind: 0,
            learning_unsent: false,
            wanted_style: None,
            style: None,
            preedit: String::new(),
            pending: String::new(),
            caret: 0,
            committed: String::new(),
            candidates: Vec::new(),
            selected: 0,
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

    /// Where the caret belongs, in UTF-16 units from the start of the preedit.
    pub fn caret_units(&self) -> usize {
        self.preedit.chars().take(self.caret).map(char::len_utf16).sum()
    }

    pub fn chinese(&self) -> bool {
        self.chinese
    }

    /// Also true while the engine has not answered: it may be composing
    /// something this side has not seen yet.
    pub fn composing(&self) -> bool {
        !self.preedit.is_empty() || self.behind > 0
    }

    /// "casual" or "formal".
    pub fn style(&self) -> &str {
        self.wanted_style.as_deref().or(self.style.as_deref()).unwrap_or(CASUAL)
    }

    /// The style the user chose earlier (in any application). The engine is
    /// told with the next request.
    pub fn want_style(&mut self, style: &str) {
        if style == CASUAL || style == FORMAL {
            self.wanted_style = Some(style.to_owned());
        }
    }

    /// The candidates to show: the page's items, the highlighted item on
    /// the page, the page number and the page count.
    pub fn candidate_page(&self) -> Option<(&[String], usize, usize, usize)> {
        if !self.window_open {
            return None;
        }
        let page = self.selected / PAGE_SIZE;
        let start = page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.candidates.len());
        Some((&self.candidates[start..end], self.selected - start, page, self.pages()))
    }

    fn pages(&self) -> usize {
        self.candidates.len().div_ceil(PAGE_SIZE).max(1)
    }

    // -- deciding ----------------------------------------------------------

    fn classify(&self, key: Key) -> Action {
        if !self.enabled || !self.chinese || is_modifier(key.vk) || key.alt {
            return Action::Pass;
        }
        if key.ctrl {
            // Ctrl with these keys types full-width punctuation; every
            // other shortcut belongs to the application.
            let punctuation = if key.shift {
                // ： ？ ！
                matches!(key.vk, vk::OEM_1 | vk::OEM_2 | 0x31)
            } else {
                // ， 。 、 ； 【 】
                matches!(
                    key.vk,
                    vk::OEM_COMMA | vk::OEM_PERIOD | vk::OEM_7 | vk::OEM_1 | vk::OEM_4 | vk::OEM_6
                )
            };
            return match ascii(key.vk, key.shift) {
                Some(c) if punctuation => Action::Send(Op::Symbol(c)),
                _ if key.vk == vk::OEM_3 && !key.shift => Action::ToggleStyle,
                _ => Action::Pass,
            };
        }
        let composing = self.composing();
        let busy = composing || self.prefix;
        // Caps Lock means the user wants Latin letters.
        if key.caps && !busy {
            return Action::Pass;
        }
        let idle = if busy { Action::Swallow } else { Action::Pass };
        let typed = ascii(key.vk, key.shift);

        if self.prefix {
            if let Some(c) = typed.filter(char::is_ascii_punctuation) {
                return Action::Send(Op::Symbol(c));
            }
            // Anything else cancels the prefix and is handled as usual.
        }

        if self.window_open && !key.shift {
            let page_start = self.selected / PAGE_SIZE * PAGE_SIZE;
            match key.vk {
                0x31..=0x39 => {
                    let index = page_start + (key.vk - 0x31) as usize;
                    return if index < self.candidates.len() {
                        Action::SelectIndex(index)
                    } else {
                        Action::Swallow
                    };
                }
                vk::DOWN => return Action::MoveSelection(1),
                vk::UP => return Action::MoveSelection(-1),
                vk::NEXT | vk::SPACE => return Action::NextPage,
                vk::PRIOR => return Action::PrevPage,
                vk::RETURN => return Action::SelectIndex(self.selected),
                vk::ESCAPE => return Action::CloseWindow,
                _ => {}
            }
        }

        // Latin letters (Shift or Caps Lock) and the Shift symbols (: " < > ? ! ...)
        // go into the composition as they are, half-width; with nothing
        // composed they are the application's.
        if key.shift || key.caps {
            if let Some(c) = typed.filter(char::is_ascii_alphabetic) {
                let c = if key.shift ^ key.caps {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                };
                return if composing && self.pending.is_empty() {
                    Action::Send(Op::Text(c.to_string()))
                } else {
                    idle
                };
            }
        }
        if key.shift {
            return match typed.filter(char::is_ascii_punctuation) {
                Some(c) if composing && self.pending.is_empty() => {
                    Action::Send(Op::Text(c.to_string()))
                }
                _ => idle, // Shift+Space (full/half width) is not implemented
            };
        }

        if let Some(c) = layout_char(key.vk) {
            let has_pending = !self.pending.is_empty();
            return match c {
                // First tone, or a space inside the composition.
                ' ' if composing => Action::Send(Op::Key(' ')),
                ' ' => idle,
                // A tone with no syllable to attach to.
                c if is_tone(c) && !has_pending => idle,
                c => Action::Send(Op::Key(c)),
            };
        }
        match key.vk {
            vk::OEM_3 => return Action::StartPrefix,
            // The symbol keys that are not part of the layout.
            vk::OEM_PLUS | vk::OEM_4 | vk::OEM_5 | vk::OEM_6 | vk::OEM_7 => {
                if let Some(c) = typed {
                    return Action::Send(Op::Symbol(c));
                }
            }
            _ => {}
        }
        if !composing {
            return idle;
        }
        let settled = self.pending.is_empty();
        match key.vk {
            vk::NUMPAD0..=vk::NUMPAD9 if settled => {
                let digit = char::from_u32(key.vk - vk::NUMPAD0 + 0x30).unwrap_or('0');
                Action::Send(Op::Text(digit.to_string()))
            }
            vk::BACK => Action::Send(Op::Backspace),
            vk::DELETE => Action::Send(Op::Delete),
            vk::LEFT => Action::Send(Op::Left),
            vk::RIGHT => Action::Send(Op::Right),
            vk::HOME => Action::Send(Op::Home),
            vk::END => Action::Send(Op::End),
            vk::DOWN if settled => Action::OpenCandidates,
            vk::UP if settled => Action::Send(Op::Cycle(1)),
            vk::RETURN => Action::Send(Op::Commit),
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
        !self.composing()
            && matches!(self.classify(key), Action::Send(Op::Key(_) | Op::Symbol(_)))
    }

    // -- talking to the engine ---------------------------------------------

    /// Copy the state the engine sent, and keep the text it wants committed.
    fn absorb(&mut self, response: &Response) {
        self.preedit.clone_from(&response.preedit);
        self.pending.clone_from(&response.pending);
        self.caret = response.caret;
        if response.style.is_some() {
            self.style.clone_from(&response.style);
        }
        if let Some(text) = &response.commit {
            self.committed.push_str(text);
        }
    }

    /// One request, with the bookkeeping that keeps both sides in step.
    fn exchange(&mut self, op: &Op) -> Result<Response, Failure> {
        let result = self.exchange_inner(op);
        match result {
            // Every response carries the whole state: back in step.
            Ok(_) => self.behind = 0,
            Err(Failure::TimedOut) => self.behind += 1,
            Err(_) => {}
        }
        result
    }

    fn exchange_inner(&mut self, op: &Op) -> Result<Response, Failure> {
        if self.stale {
            // The first Escape only drops an unfinished syllable.
            self.engine.request(&Op::Escape)?;
            self.engine.request(&Op::Escape)?;
            self.stale = false;
        }
        if self.learning_unsent {
            self.engine.request(&Op::Learning(self.enabled))?;
            self.learning_unsent = false;
        }
        if let Some(wanted) = self.wanted_style.clone().filter(|w| self.style.as_ref() != Some(w)) {
            let response = self.engine.request(&Op::Style(wanted.clone()))?;
            self.absorb(&response);
            // An engine that does not report its style is not asked again.
            self.style.get_or_insert(wanted);
        }
        let response = self.engine.request(op)?;
        self.absorb(&response);
        Ok(response)
    }

    /// Catch up after a request that was not answered in time: the engine
    /// has handled it by now, or will before it answers this one. Without
    /// this the composition would show (and the next key would be judged
    /// by) a state the engine has already left.
    /// True if the engine turned out to be gone and the composition was
    /// given up.
    fn catch_up(&mut self) -> bool {
        if self.behind == 0 {
            return false;
        }
        match self.engine.request(&Op::State) {
            Ok(response) => {
                self.absorb(&response);
                self.behind = 0;
            }
            // Late is not gone: the composition stays for as long as the
            // pipe does. (The host breaks it off if the engine never answers.)
            Err(Failure::TimedOut) => self.behind += 1,
            Err(Failure::Unreachable) => {
                self.lost();
                return true;
            }
            // It answers, so nothing is outstanding any more.
            Err(Failure::Rejected) => self.behind = 0,
        }
        false
    }

    fn clear(&mut self) {
        self.preedit.clear();
        self.pending.clear();
        self.caret = 0;
        self.window_open = false;
        self.prefix = false;
    }

    /// The engine is gone (or the text is needed now and it does not
    /// answer). Give up on the composition, keeping what is on screen
    /// exactly as it is, so that nothing the user typed is lost.
    fn lost(&mut self) {
        self.stale = true;
        self.behind = 0;
        // Whatever the old session still commits must not arrive later,
        // on top of the text committed here.
        self.engine.abandon();
        let shown = std::mem::take(&mut self.preedit);
        self.committed.push_str(&shown);
        self.clear();
    }

    fn take_committed(&mut self) -> Option<String> {
        Some(std::mem::take(&mut self.committed)).filter(|text| !text.is_empty())
    }

    /// End the composition now and return its text, if there is one.
    /// Used when focus moves away and when the mode changes.
    pub fn finish(&mut self) -> Option<String> {
        self.prefix = false;
        if !self.composing() {
            return None;
        }
        self.window_open = false;
        self.catch_up();
        // The text is needed now, so there is no waiting for a late answer.
        if self.composing() && self.exchange(&Op::Commit).is_err() {
            self.lost();
        }
        self.take_committed()
    }

    /// Report the text before the cursor. Ignored while composing.
    pub fn set_context(&mut self, text: &str) {
        if !self.composing() {
            // Only a hint: a failure here costs nothing.
            let _ = self.exchange(&Op::Context(text.to_owned()));
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
        }
        if std::mem::take(&mut self.behind) > 0 {
            // A late answer must not put text into the password field.
            self.engine.abandon();
        }
        self.clear();
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
        // Judged before catching up: if that ends the composition, the key
        // was still meant for it and must not reach the application.
        let was_busy = self.composing() || self.prefix;
        let gave_up = self.catch_up();
        let action = if gave_up { Action::Swallow } else { self.classify(key) };
        if action != Action::Pass {
            self.prefix = false;
        }
        if !matches!(
            action,
            Action::Pass
                | Action::Swallow
                | Action::MoveSelection(_)
                | Action::NextPage
                | Action::PrevPage
        ) {
            self.window_open = false;
        }
        let mut outcome = Outcome { eaten: action != Action::Pass, ..Outcome::default() };
        let count = self.candidates.len().max(1);
        let page = self.selected / PAGE_SIZE;
        let sent = match action {
            Action::Pass | Action::Swallow | Action::CloseWindow => Ok(()),
            Action::StartPrefix => {
                self.prefix = true;
                Ok(())
            }
            Action::MoveSelection(step) => {
                self.selected = (self.selected + count).wrapping_add_signed(step) % count;
                Ok(())
            }
            Action::NextPage => {
                self.selected = (page + 1) % self.pages() * PAGE_SIZE;
                Ok(())
            }
            Action::PrevPage => {
                self.selected = (page + self.pages() - 1) % self.pages() * PAGE_SIZE;
                Ok(())
            }
            Action::Send(op) => self.exchange(&op).map(drop),
            Action::OpenCandidates => self.exchange(&Op::Candidates).map(|response| {
                if !response.candidates.is_empty() {
                    self.candidates = response.candidates;
                    self.selected = 0;
                    self.window_open = true;
                }
            }),
            Action::SelectIndex(index) => self.exchange(&Op::Select(index)).map(drop),
            Action::ToggleStyle => {
                let before = self.wanted_style.clone();
                let next = if self.style() == FORMAL { CASUAL } else { FORMAL };
                self.wanted_style = Some(next.to_owned());
                let done = self.exchange(&Op::State).map(drop);
                if done.is_ok() {
                    outcome.style_changed = true;
                } else {
                    self.wanted_style = before;
                }
                done
            }
        };
        match sent {
            Ok(()) => {}
            // The engine has the key and is merely slow: the composition
            // stays. Wait once more for its answer, so that the screen
            // shows what this key did.
            Err(Failure::TimedOut) => {
                self.catch_up();
            }
            // The engine refused this one key; the composition is intact.
            Err(Failure::Rejected) => {}
            Err(Failure::Unreachable) => {
                // With nothing composed the key simply goes to the
                // application; otherwise it is dropped, because it was
                // meant for a composition that no longer exists.
                outcome.eaten = was_busy;
                self.lost();
            }
        }
        outcome.commit = self.take_committed();
        outcome
    }

    /// Shift pressed and released on its own switches Chinese/English.
    pub fn key_up(&mut self, key: Key) -> Outcome {
        let mut outcome = Outcome::default();
        if is_shift(key.vk) && std::mem::take(&mut self.shift_alone) && self.enabled {
            outcome.commit = self.toggle_chinese();
        }
        outcome
    }

    /// Switch Chinese/English (the tray button does this too). Returns the
    /// text of the composition that the switch ended, if there was one.
    pub fn toggle_chinese(&mut self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        self.chinese = !self.chinese;
        self.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for the engine: every finished syllable becomes 字, and
    /// symbols and text are kept as they are sent. The cursor stays at the end.
    #[derive(Default)]
    struct Fake {
        down: bool,
        text: String,
        pending: String,
        style: String,
        /// Committed automatically with the next response, as a full buffer would be.
        flush: Option<String>,
        /// This many requests are handled but answered too late.
        slow: usize,
        /// Requests fail with an internal error and change nothing.
        broken: bool,
        /// Text committed by responses nobody waited for.
        late: String,
        abandoned: bool,
        log: Vec<Op>,
    }

    impl Engine for Fake {
        fn request(&mut self, op: &Op) -> Result<Response, Failure> {
            if self.down {
                return Err(Failure::Unreachable);
            }
            if self.broken {
                return Err(Failure::Rejected);
            }
            let mut response = self.handle(op);
            if self.slow > 0 {
                self.slow -= 1;
                self.late.push_str(response.commit.as_deref().unwrap_or(""));
                return Err(Failure::TimedOut);
            }
            if !self.late.is_empty() {
                let late = std::mem::take(&mut self.late);
                response.commit = Some(late + response.commit.as_deref().unwrap_or(""));
            }
            Ok(response)
        }

        fn abandon(&mut self) {
            self.abandoned = true;
            self.late.clear();
        }
    }

    impl Fake {
        fn handle(&mut self, op: &Op) -> Response {
            self.log.push(op.clone());
            let mut response = Response { ok: true, ..Response::default() };
            match op {
                Op::Key(' ') if self.pending.is_empty() => self.text.push(' '),
                Op::Key(c) if is_tone(*c) => {
                    self.pending.clear();
                    self.text.push('字');
                }
                Op::Key(c) => self.pending.push(*c),
                Op::Symbol(c) => self.text.push(*c),
                Op::Text(text) => self.text.push_str(text),
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
                    if self.pending.is_empty() {
                        self.text.clear();
                    }
                    self.pending.clear();
                }
                Op::Style(style) => self.style.clone_from(style),
                _ => {}
            }
            if let Some(flushed) = self.flush.take() {
                response.commit = Some(flushed + response.commit.as_deref().unwrap_or(""));
            }
            response.preedit = format!("{}{}", self.text, self.pending);
            response.pending = self.pending.clone();
            response.segments = self.text.chars().map(String::from).collect();
            response.cursor = self.text.chars().count();
            response.caret = response.preedit.chars().count();
            response.style =
                Some(if self.style.is_empty() { CASUAL.to_owned() } else { self.style.clone() });
            response
        }
    }

    fn key(c: char) -> Key {
        let vk = match c {
            ' ' => vk::SPACE,
            ',' => vk::OEM_COMMA,
            '`' => vk::OEM_3,
            '[' => vk::OEM_4,
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

    fn eaten(commit: Option<&str>) -> Outcome {
        Outcome { eaten: true, commit: commit.map(str::to_owned), style_changed: false }
    }

    #[test]
    fn compose_and_commit() {
        let mut c = typed("ji3");
        assert_eq!(c.preedit(), "字");
        assert_eq!(c.key_down(code(vk::RETURN)), eaten(Some("字")));
        assert!(!c.composing());
    }

    #[test]
    fn idle_keys_go_to_the_application() {
        let c = Controller::new(Fake::default());
        for vk in [
            vk::RETURN,
            vk::SPACE,
            vk::BACK,
            vk::DELETE,
            vk::LEFT,
            vk::HOME,
            vk::UP,
            vk::DOWN,
            vk::ESCAPE,
            vk::NUMPAD0,
            0x33,
        ] {
            assert!(!c.test_key(code(vk)), "vk {vk:#x}");
        }
        assert!(c.test_key(key('j')));
        assert!(c.starts_composition(key('j')));
    }

    #[test]
    fn keys_during_composition_stay_in_it() {
        let mut c = typed("ji3");
        assert!(c.test_key(code(0x09))); // Tab is swallowed
        assert!(c.test_key(code(0x33))); // a tone with no pending syllable
        assert!(!c.starts_composition(key('j')));
        // Shortcuts still reach the application.
        assert!(!c.test_key(Key { ctrl: true, ..key('c') }));

        for (vk, op) in [
            (vk::DELETE, Op::Delete),
            (vk::LEFT, Op::Left),
            (vk::RIGHT, Op::Right),
            (vk::HOME, Op::Home),
            (vk::END, Op::End),
            (vk::UP, Op::Cycle(1)),
            (vk::NUMPAD0 + 5, Op::Text("5".into())),
        ] {
            assert!(c.key_down(code(vk)).eaten);
            assert_eq!(c.engine_mut().log.last(), Some(&op), "vk {vk:#x}");
        }
    }

    #[test]
    fn space_is_first_tone_or_a_space() {
        let mut c = typed("ji ");
        assert_eq!(c.preedit(), "字");
        c.key_down(key(' '));
        assert_eq!(c.preedit(), "字 ");
        assert!(c.candidate_page().is_none());
    }

    #[test]
    fn candidate_window() {
        let mut c = typed("ji3");
        c.key_down(code(vk::DOWN));
        let (items, selected, page, pages) = c.candidate_page().unwrap();
        assert_eq!((items.len(), selected, page, pages), (9, 0, 0, 2));

        c.key_down(code(vk::DOWN));
        c.key_down(code(vk::DOWN));
        c.key_down(code(vk::UP));
        assert_eq!(c.candidate_page().unwrap().1, 1);
        c.key_down(code(vk::UP));
        c.key_down(code(vk::UP)); // wraps to the last candidate
        assert_eq!(c.candidate_page().unwrap().0, ["c9", "c10", "c11"]);
        assert_eq!(c.candidate_page().unwrap().1, 2);

        c.key_down(key(' ')); // next page wraps to the first
        assert_eq!(c.candidate_page().unwrap().2, 0);
        c.key_down(code(vk::PRIOR));
        assert_eq!(c.candidate_page().unwrap().0, ["c9", "c10", "c11"]);
        assert!(c.key_down(key('5')).eaten); // no fifth item on this page
        assert!(c.candidate_page().is_some());
        c.key_down(key('2'));
        assert!(c.candidate_page().is_none());
        assert_eq!(c.engine_mut().log.last(), Some(&Op::Select(10)));

        // Enter picks the highlighted candidate; Escape only closes the window.
        c.key_down(code(vk::DOWN));
        c.key_down(code(vk::DOWN));
        c.key_down(code(vk::RETURN));
        assert_eq!(c.engine_mut().log.last(), Some(&Op::Select(1)));
        assert!(c.composing());
        c.key_down(code(vk::DOWN));
        c.key_down(code(vk::ESCAPE));
        assert!(c.candidate_page().is_none());
        assert_eq!(c.preedit(), "字");
    }

    #[test]
    fn shift_alone_toggles_and_commits() {
        let mut c = typed("ji3");
        c.key_down(code(vk::SHIFT));
        let outcome = c.key_up(code(vk::SHIFT));
        assert_eq!(outcome, Outcome { commit: Some("字".into()), ..Outcome::default() });
        assert!(!c.chinese());
        assert!(!c.test_key(key('j')));
        c.key_down(code(vk::SHIFT));
        c.key_up(code(vk::SHIFT));
        assert!(c.chinese());
    }

    #[test]
    fn punctuation_goes_into_the_composition() {
        let mut c = typed("ji3");
        c.key_down(code(vk::SHIFT));
        assert_eq!(c.key_down(Key { shift: true, ..key(',') }), eaten(None));
        assert_eq!(c.engine_mut().log.last(), Some(&Op::Text("<".into())));
        c.key_up(code(vk::SHIFT));
        assert!(c.chinese(), "shift used with another key must not toggle");
        assert_eq!(c.key_down(Key { shift: true, ..key('1') }), eaten(None));
        assert_eq!(c.key_down(Key { ctrl: true, ..key(',') }), eaten(None));
        assert_eq!(c.key_down(key('[')), eaten(None));
        // Ctrl+Shift with ; / 1 are ： ？ ！; other Ctrl+Shift keys are shortcuts.
        let ctrl_shift = |vk| Key { vk, ctrl: true, shift: true, ..Key::default() };
        assert!(c.test_key(Key { vk: vk::OEM_1, ctrl: true, ..Key::default() }));
        assert!(!c.test_key(ctrl_shift(vk::OEM_COMMA)));
        assert!(!c.test_key(Key { vk: vk::OEM_2, ctrl: true, ..Key::default() }));
        for (vk, symbol) in [(vk::OEM_1, ':'), (vk::OEM_2, '?'), (0x31, '!')] {
            assert!(c.key_down(ctrl_shift(vk)).eaten);
            assert_eq!(c.engine_mut().log.last(), Some(&Op::Symbol(symbol)));
            c.key_down(code(vk::BACK));
        }
        assert_eq!(c.preedit(), "字<!,[");

        // Punctuation also starts a composition.
        let mut c = Controller::new(Fake::default());
        assert!(!c.test_key(Key { shift: true, ..key(',') }), "half-width, typed by the application");
        assert!(c.starts_composition(Key { ctrl: true, ..key(',') }));
        assert!(c.key_down(Key { ctrl: true, ..key(',') }).eaten);
        assert_eq!(c.preedit(), ",");
    }

    #[test]
    fn latin_letters() {
        // With nothing composed they are the application's.
        let c = Controller::new(Fake::default());
        assert!(!c.test_key(Key { shift: true, ..key('a') }));
        assert!(!c.test_key(Key { caps: true, ..key('a') }));

        let mut c = typed("ji3");
        c.key_down(Key { shift: true, ..key('a') });
        c.key_down(Key { caps: true, ..key('b') });
        c.key_down(Key { caps: true, shift: true, ..key('c') });
        assert_eq!(c.preedit(), "字ABc");
    }

    #[test]
    fn backtick_prefix() {
        let mut c = Controller::new(Fake::default());
        assert!(c.key_down(key('`')).eaten);
        assert!(!c.composing());
        assert!(c.key_down(key(',')).eaten);
        assert_eq!(c.engine_mut().log, [Op::Symbol(',')]);

        // Any other key cancels the prefix and is handled as usual.
        c.key_down(code(vk::RETURN));
        c.key_down(key('`'));
        c.key_down(key('j'));
        assert_eq!(c.engine_mut().log.last(), Some(&Op::Key('j')));
        c.key_down(code(vk::ESCAPE));
        c.key_down(key('`'));
        assert!(c.key_down(code(vk::RETURN)).eaten, "cancelling the prefix eats the key");
        assert!(!c.test_key(code(vk::RETURN)));
    }

    #[test]
    fn text_committed_by_the_engine_is_passed_on() {
        let mut c = typed("ji3");
        c.engine_mut().flush = Some("前面".into());
        assert_eq!(c.key_down(key('j')), eaten(Some("前面")));
        assert!(c.composing());
        c.engine_mut().flush = Some("前面".into());
        assert_eq!(c.key_down(code(vk::RETURN)), eaten(Some("前面字")));
    }

    #[test]
    fn style() {
        let mut c = Controller::new(Fake::default());
        assert_eq!(c.style(), CASUAL);
        let toggle = Key { ctrl: true, ..key('`') };
        assert!(c.test_key(toggle));
        let outcome = c.key_down(toggle);
        assert_eq!(outcome, Outcome { eaten: true, commit: None, style_changed: true });
        assert_eq!(c.style(), FORMAL);
        assert_eq!(c.engine_mut().log, [Op::Style(FORMAL.into()), Op::State]);

        // A new connection starts with the engine's default: tell it again.
        c.engine_mut().style.clear();
        c.engine_mut().log.clear();
        c.key_down(key('j'));
        c.key_down(key('i'));
        assert_eq!(
            c.engine_mut().log,
            [Op::Key('j'), Op::Style(FORMAL.into()), Op::Key('i')]
        );

        // Chosen in another application.
        c.want_style(CASUAL);
        c.key_down(key('3'));
        assert_eq!(c.engine_mut().style, CASUAL);

        // Engine down: the key is the application's, nothing changes.
        c.key_down(code(vk::ESCAPE));
        c.engine_mut().down = true;
        assert_eq!(c.key_down(toggle), Outcome::default());
        assert_eq!(c.style(), CASUAL);
    }

    #[test]
    fn engine_down_while_idle_passes_the_key() {
        let mut c = Controller::new(Fake { down: true, ..Fake::default() });
        assert_eq!(c.key_down(key('j')), Outcome::default());
    }

    #[test]
    fn engine_down_while_composing_keeps_the_text() {
        let mut c = typed("ji3j");
        c.engine_mut().down = true;
        assert_eq!(c.key_down(key('i')), eaten(Some("字j")));
        assert!(!c.composing());

        // Back up: the abandoned composition is discarded before anything else.
        c.engine_mut().down = false;
        c.engine_mut().log.clear();
        c.key_down(key('j'));
        assert_eq!(c.engine_mut().log, [Op::Escape, Op::Escape, Op::Key('j')]);
        assert_eq!(c.preedit(), "j");
    }

    #[test]
    fn slow_engine_keeps_the_composition() {
        // The tone key is answered late: nothing is committed, and the
        // second wait shows the finished syllable.
        let mut c = typed("ji3ji");
        c.engine_mut().slow = 1;
        assert_eq!(c.key_down(key('3')), eaten(None));
        assert_eq!(c.preedit(), "字字");
        assert_eq!(c.engine_mut().log.last(), Some(&Op::State));

        // Still no answer after the second wait: the screen is behind, but
        // nothing is lost and the next key catches up first.
        c.key_down(key('j'));
        c.key_down(key('i'));
        c.engine_mut().slow = 2;
        assert_eq!(c.key_down(key('3')), eaten(None));
        assert_eq!(c.preedit(), "字字ji");
        assert!(c.composing());
        assert_eq!(c.key_down(code(vk::DOWN)), eaten(None));
        assert!(c.candidate_page().is_some(), "judged by the state the engine is in");
        assert_eq!(c.preedit(), "字字字");
        assert!(!c.engine_mut().abandoned);
    }

    #[test]
    fn slow_first_key_is_not_typed_twice() {
        let mut c = Controller::new(Fake { slow: 2, ..Fake::default() });
        assert_eq!(c.key_down(key('j')), eaten(None), "the engine has it");
        assert!(c.test_key(code(vk::BACK)), "there may be a composition");
        c.key_down(key('i'));
        assert_eq!(c.preedit(), "ji");
    }

    #[test]
    fn slow_commit_arrives_once() {
        let mut c = typed("ji3");
        c.engine_mut().slow = 1;
        assert_eq!(c.key_down(code(vk::RETURN)), eaten(Some("字")));
        assert!(!c.composing());

        // The automatic commit of a long buffer, answered late.
        let mut c = typed("ji3");
        c.engine_mut().flush = Some("前面".into());
        c.engine_mut().slow = 2;
        assert_eq!(c.key_down(key('j')), eaten(None));
        assert_eq!(c.key_down(key('i')), eaten(Some("前面")));
        assert_eq!(c.preedit(), "字ji");

        // A commit answered late ends the composition; the key after it
        // starts a new one.
        let mut c = typed("ji3");
        c.engine_mut().slow = 2;
        assert_eq!(c.key_down(code(vk::RETURN)), eaten(None));
        assert_eq!(c.key_down(key('j')), eaten(Some("字")));
        assert_eq!(c.preedit(), "j");
    }

    #[test]
    fn only_a_broken_pipe_ends_the_composition() {
        let mut c = typed("ji3j");
        c.engine_mut().slow = usize::MAX;
        for _ in 0..10 {
            assert_eq!(c.key_down(key('i')), eaten(None));
        }
        assert_eq!(c.preedit(), "字j");
        assert!(!c.engine_mut().abandoned);
        // What is on screen goes into the document as it is.
        c.engine_mut().down = true;
        assert_eq!(c.key_down(key('i')), eaten(Some("字j")));
        assert!(!c.composing());

        // Focus moves away while the engine is slow: the text is needed
        // now, and the late answer must not deliver it a second time.
        let mut c = typed("ji3");
        c.engine_mut().slow = usize::MAX;
        assert_eq!(c.finish().as_deref(), Some("字"));
        assert!(c.engine_mut().abandoned);
    }

    #[test]
    fn engine_error_changes_nothing() {
        let mut c = typed("ji3j");
        c.engine_mut().broken = true;
        assert_eq!(c.key_down(key('i')), eaten(None));
        assert_eq!(c.preedit(), "字j");
        c.engine_mut().broken = false;
        c.key_down(key('i'));
        assert_eq!(c.preedit(), "字ji");
        assert_eq!(c.engine_mut().log.last(), Some(&Op::Key('i')), "no reset in between");
    }

    #[test]
    fn password_field() {
        let mut c = typed("ji3j");
        c.set_enabled(false);
        assert!(!c.composing());
        assert!(!c.test_key(key('j')));
        assert!(!c.test_key(Key { ctrl: true, ..key(',') }));
        let log = &c.engine_mut().log;
        assert_eq!(
            log[log.len() - 4..],
            [Op::Escape, Op::Escape, Op::Learning(false), Op::State]
        );
        c.key_down(code(vk::SHIFT));
        c.key_up(code(vk::SHIFT));
        assert!(c.chinese(), "no mode switch inside a password field");
        c.set_enabled(true);
        assert!(c.engine_mut().log.contains(&Op::Learning(true)));
        assert!(c.test_key(key('j')));
    }
}
