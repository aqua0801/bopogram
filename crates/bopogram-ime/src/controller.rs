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

use std::time::{Duration, Instant};

use crate::protocol::{Engine, Failure, Op, Response};

/// What the user chose in the settings window (`keys.json`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// Candidates per page, 5 to 10.
    pub page_size: usize,
    /// Pick with A S D F ... instead of 1 2 3 ...
    pub home_row: bool,
    /// The mode a new application starts in.
    pub start_chinese: bool,
    /// Shift pressed and released on its own switches Chinese/English.
    pub shift_toggles: bool,
    /// Opens the phrase menu; `None` when the user turned it off.
    pub phrase_key: Option<Hotkey>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            page_size: 9,
            home_row: false,
            start_chinese: true,
            shift_toggles: true,
            phrase_key: Some(Hotkey::PHRASES),
        }
    }
}

/// A key combination that works wherever the caret is (registered with TSF
/// as a preserved key, so the application never sees it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub vk: u32,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Hotkey {
    /// Alt+`, the default for the phrase menu.
    pub const PHRASES: Hotkey = Hotkey { vk: vk::OEM_3, ctrl: false, alt: true, shift: false };

    /// `{"vk": 192, "ctrl": false, "alt": true, "shift": false}`. A key
    /// without Ctrl or Alt would take a key away from typing: refused.
    fn parse(value: &serde_json::Value) -> Option<Hotkey> {
        let vk = u32::try_from(value.get("vk")?.as_u64()?).ok()?;
        let flag = |name: &str| value.get(name).and_then(serde_json::Value::as_bool).unwrap_or(false);
        let key = Hotkey { vk, ctrl: flag("ctrl"), alt: flag("alt"), shift: flag("shift") };
        ((1..0xFF).contains(&vk) && !is_modifier(vk) && (key.ctrl || key.alt)).then_some(key)
    }
}

/// A text the user keeps for the phrase menu (`phrases.json`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Phrase {
    /// Shown in the menu; the first line of the text when empty.
    pub name: String,
    /// What is inserted. Lines are separated by `\n`.
    pub text: String,
}

/// Longest label in the phrase menu, in characters.
const PHRASE_LABEL_CHARS: usize = 20;

impl Phrase {
    /// The phrase as the menu shows it: one short line.
    pub fn label(&self) -> String {
        let name = self.name.trim();
        let (source, more) = if name.is_empty() {
            let text = self.text.trim();
            (text.lines().next().unwrap_or(""), text.lines().nth(1).is_some())
        } else {
            (name, false)
        };
        let mut label: String = source.chars().take(PHRASE_LABEL_CHARS).collect();
        if more || source.chars().count() > PHRASE_LABEL_CHARS {
            label.push('…');
        }
        label
    }
}

/// Read `phrases.json`: `{"phrases": [{"name": "...", "text": "..."}, ...]}`.
/// Entries without text are skipped.
pub fn parse_phrases(text: &str) -> Vec<Phrase> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else {
        return Vec::new();
    };
    let Some(list) = value.get("phrases").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let field = |item: &serde_json::Value, name: &str| {
        item.get(name).and_then(serde_json::Value::as_str).unwrap_or("").replace("\r\n", "\n")
    };
    list.iter()
        .map(|item| Phrase { name: field(item, "name"), text: field(item, "text") })
        .filter(|phrase| !phrase.text.is_empty())
        .collect()
}

/// How the document takes a phrase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Insertion {
    /// What a line break becomes (`\r\n` for most Windows controls).
    pub newline: &'static str,
    /// False where a line break may act as Enter (terminals): phrases with
    /// more than one line are refused there.
    pub multiline: bool,
    /// True where text with line breaks must be typed rather than inserted
    /// (browsers and Electron: an inserted line break left the last line
    /// stuck as an open composition, sometimes doubled). The text then goes
    /// in as keystrokes, each line break as CR with Shift held.
    pub keystrokes: bool,
}

/// Said when a phrase with several lines is refused.
pub const MULTILINE_REFUSED: &str = "這個程式可能把換行當成 Enter，多行片語未送出";

/// The phrase menu while it is open.
struct Menu {
    phrases: Vec<Phrase>,
    labels: Vec<String>,
    selected: usize,
    insertion: Insertion,
}

impl Options {
    const DIGITS: &'static str = "1234567890";
    const HOME_ROW: &'static str = "asdfghjkl;";

    /// Read `keys.json`; anything missing or out of range keeps its default.
    pub fn parse(text: &str) -> Self {
        let mut options = Options::default();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else {
            return options;
        };
        if let Some(size) = value.get("page_size").and_then(serde_json::Value::as_u64) {
            if (5..=10).contains(&size) {
                options.page_size = size as usize;
            }
        }
        match value.get("select_keys").and_then(serde_json::Value::as_str) {
            Some("home_row") => options.home_row = true,
            Some("digits") => options.home_row = false,
            _ => {}
        }
        match value.get("start_mode").and_then(serde_json::Value::as_str) {
            Some("english") => options.start_chinese = false,
            Some("chinese") => options.start_chinese = true,
            _ => {}
        }
        if let Some(on) = value.get("shift_toggle").and_then(serde_json::Value::as_bool) {
            options.shift_toggles = on;
        }
        match value.get("phrase_key") {
            Some(serde_json::Value::Null) => options.phrase_key = None,
            Some(key) => options.phrase_key = Hotkey::parse(key).or(options.phrase_key),
            None => {}
        }
        options
    }

    /// The keys that pick from a page, in order.
    pub fn labels(&self) -> Vec<char> {
        let keys = if self.home_row { Self::HOME_ROW } else { Self::DIGITS };
        keys.chars().take(self.page_size).collect()
    }
}

/// What a key types with Caps Lock taken into account: it swaps the case
/// of letters, and only of letters.
fn typed_with_caps(key: Key) -> Option<char> {
    ascii(key.vk, key.shift).map(|c| {
        if key.caps && c.is_ascii_alphabetic() {
            if key.shift { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() }
        } else {
            c
        }
    })
}

/// The full-width form of a printable ASCII character.
fn full_width(c: char) -> char {
    match c {
        ' ' => '\u{3000}',
        '!'..='~' => char::from_u32(c as u32 + 0xFEE0).unwrap_or(c),
        c => c,
    }
}

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
    /// The key switched between full and half width.
    pub width_changed: bool,
    /// Something to tell the user near the caret.
    pub notice: Option<&'static str>,
    /// Text to type as keystrokes (`\n` meaning Shift+Enter), after
    /// everything else.
    pub keystrokes: Option<String>,
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
    ToggleWidth,
    /// Text for the document, not for the engine (full-width characters
    /// typed with nothing composed).
    Type(char),
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

/// The bopomofo symbol (or tone mark) of a Dachen layout key, for showing
/// keys while the engine is starting. The first tone (Space) has no mark.
fn dachen_symbol(key: char) -> Option<char> {
    const KEYS: &str = "1qaz2wsxedcrfv5tgbyhnujm8ik,9ol.0p;/-6347";
    const SYMBOLS: &str = "ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙㄧㄨㄩㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦˊˇˋ˙";
    KEYS.chars().position(|k| k == key).and_then(|i| SYMBOLS.chars().nth(i))
}

/// How long keys are kept for an engine that is starting, before the
/// composition is given up (and shown text committed as it is).
const STARTING_LIMIT: Duration = Duration::from_secs(20);
/// After giving up on a starting engine, how long "starting" counts as
/// "unreachable", so that keys reach the application.
const STARTING_COOLDOWN: Duration = Duration::from_secs(30);

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
    /// Full-width mode (Shift+Space, Chinese mode only): Latin letters,
    /// digits, symbols and spaces typed outside a composition come out full
    /// width.
    full_width: bool,
    options: Options,
    /// A request failed, so the engine may still hold a composition that
    /// the front end has given up on. Discard it before the next request.
    stale: bool,
    /// Names of the requests made since `take_sent` was last called (for
    /// the timing log).
    sent: Vec<&'static str>,
    /// Requests kept while the engine starts, sent in order once it answers.
    /// While there are any, the preedit is drawn here from the keys.
    queued: Vec<Op>,
    /// When the first of them was kept.
    queued_since: Option<Instant>,
    /// Until then, an engine that says it is starting is treated as absent.
    starting_refused_until: Option<Instant>,
    starting_limit: Duration,
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
    /// The phrase menu, shown in the candidate window instead of candidates.
    menu: Option<Menu>,
}

impl<E: Engine> Controller<E> {
    pub fn new(engine: E) -> Self {
        Controller {
            engine,
            chinese: true,
            enabled: true,
            shift_alone: false,
            prefix: false,
            full_width: false,
            options: Options::default(),
            stale: false,
            behind: 0,
            queued: Vec::new(),
            queued_since: None,
            starting_refused_until: None,
            starting_limit: STARTING_LIMIT,
            sent: Vec::new(),
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
            menu: None,
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

    /// False in a password field.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Full width is in effect (it applies in Chinese mode only).
    pub fn full_width(&self) -> bool {
        self.full_width && self.chinese
    }

    pub fn set_options(&mut self, options: Options) {
        self.options = options;
    }

    /// The keys that pick from a page, shown beside the candidates.
    pub fn labels(&self) -> Vec<char> {
        self.options.labels()
    }

    /// Where the page with the highlighted candidate starts.
    pub fn page_start(&self) -> usize {
        self.selected() / self.options.page_size * self.options.page_size
    }

    /// What the candidate window lists: the phrase menu, or the candidates.
    fn list(&self) -> Option<(&[String], usize)> {
        match &self.menu {
            Some(menu) => Some((&menu.labels[..], menu.selected)),
            None => self.window_open.then_some((&self.candidates[..], self.selected)),
        }
    }

    fn list_len(&self) -> usize {
        self.list().map_or(0, |(items, _)| items.len())
    }

    fn selected(&self) -> usize {
        self.list().map_or(0, |(_, selected)| selected)
    }

    fn set_selected(&mut self, index: usize) {
        match &mut self.menu {
            Some(menu) => menu.selected = index,
            None => self.selected = index,
        }
    }

    /// The phrase menu is open.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// Also true while the engine has not answered: it may be composing
    /// something this side has not seen yet.
    pub fn composing(&self) -> bool {
        !self.preedit.is_empty() || self.behind > 0 || !self.queued.is_empty()
    }

    /// Keys are waiting for the engine to finish starting: call `flush`
    /// again shortly.
    pub fn waiting(&self) -> bool {
        !self.queued.is_empty()
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
        let (items, selected) = self.list()?;
        let size = self.options.page_size;
        let page = selected / size;
        let start = page * size;
        let end = (start + size).min(items.len());
        Some((&items[start..end], selected - start, page, self.pages()))
    }

    fn pages(&self) -> usize {
        self.list_len().div_ceil(self.options.page_size).max(1)
    }

    /// The whole candidate list and the highlighted index, while the
    /// window is open (for applications that draw the list themselves).
    pub fn candidate_list(&self) -> Option<(&[String], usize)> {
        self.list()
    }

    /// Where each page starts in the candidate list.
    pub fn page_starts(&self) -> Vec<usize> {
        (0..self.pages()).map(|page| page * self.options.page_size).collect()
    }

    /// Highlight candidate `index` of the whole list. Out of range is ignored.
    pub fn highlight(&mut self, index: usize) {
        if index < self.list_len() {
            self.set_selected(index);
        }
    }

    /// Show the next (1) or previous (-1) page, wrapping around.
    pub fn turn_page(&mut self, delta: isize) {
        if self.list().is_some() {
            let pages = self.pages();
            let size = self.options.page_size;
            let page = (self.selected() / size + pages).wrapping_add_signed(delta) % pages;
            self.set_selected(page * size);
        }
    }

    /// Close the candidate window (or the phrase menu) without choosing anything.
    pub fn close_candidates(&mut self) {
        self.window_open = false;
        self.menu = None;
    }

    /// Choose candidate `index` of the whole list, as its digit key would
    /// (the mouse, or an application that draws the list itself).
    pub fn pick(&mut self, index: usize) -> Outcome {
        if self.menu.is_some() {
            return self.choose_phrase(index);
        }
        if !self.window_open || index >= self.candidates.len() {
            return Outcome::default();
        }
        self.window_open = false;
        match self.exchange(&Op::Select(index)) {
            Ok(_) | Err(Failure::Rejected | Failure::Starting) => {}
            Err(Failure::TimedOut) => {
                self.catch_up();
            }
            Err(Failure::Unreachable) => self.lost(),
        }
        Outcome { eaten: true, commit: self.take_committed(), ..Outcome::default() }
    }

    // -- the phrase menu ---------------------------------------------------

    /// Open the phrase menu. A composition in progress is committed first
    /// (its text is in the outcome). Nothing happens in a password field.
    pub fn open_menu(&mut self, phrases: Vec<Phrase>, insertion: Insertion) -> Outcome {
        if !self.enabled || phrases.is_empty() {
            return Outcome::default();
        }
        let commit = self.finish();
        let labels = phrases.iter().map(Phrase::label).collect();
        self.menu = Some(Menu { phrases, labels, selected: 0, insertion });
        Outcome { eaten: true, commit, ..Outcome::default() }
    }

    /// Close the phrase menu. True if it was open.
    pub fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    /// A key the menu does not take closes it (the key itself then goes
    /// wherever it would have gone). True if the menu was closed.
    pub fn dismiss_menu(&mut self, key: Key) -> bool {
        self.menu.is_some() && self.menu_action(key).is_none() && self.close_menu()
    }

    /// What a key does while the phrase menu is open: `None` for a key that
    /// closes it, `Some(Action::Pass)` for a modifier, which leaves it be.
    fn menu_action(&self, key: Key) -> Option<Action> {
        let menu = self.menu.as_ref()?;
        if is_modifier(key.vk) {
            return Some(Action::Pass);
        }
        if key.ctrl || key.alt {
            return None;
        }
        if !key.shift {
            if let Some(position) = self.selection_key(key) {
                let index = self.page_start() + position;
                return Some(if index < menu.phrases.len() {
                    Action::SelectIndex(index)
                } else {
                    Action::Swallow
                });
            }
        }
        match key.vk {
            vk::DOWN => Some(Action::MoveSelection(1)),
            vk::UP => Some(Action::MoveSelection(-1)),
            vk::NEXT | vk::SPACE => Some(Action::NextPage),
            vk::PRIOR => Some(Action::PrevPage),
            vk::RETURN => Some(Action::SelectIndex(menu.selected)),
            vk::ESCAPE => Some(Action::CloseWindow),
            _ => None,
        }
    }

    fn menu_key(&mut self, action: Action) -> Outcome {
        let count = self.list_len().max(1);
        match action {
            Action::SelectIndex(index) => return self.choose_phrase(index),
            Action::MoveSelection(step) => {
                let next = (self.selected() + count).wrapping_add_signed(step) % count;
                self.set_selected(next);
            }
            Action::NextPage => self.turn_page(1),
            Action::PrevPage => self.turn_page(-1),
            Action::CloseWindow => self.menu = None,
            _ => {}
        }
        Outcome { eaten: true, ..Outcome::default() }
    }

    /// Insert phrase `index` and close the menu.
    fn choose_phrase(&mut self, index: usize) -> Outcome {
        let mut outcome = Outcome { eaten: true, ..Outcome::default() };
        let Some(menu) = self.menu.take() else {
            return outcome;
        };
        let Some(phrase) = menu.phrases.get(index) else {
            return outcome;
        };
        if phrase.text.contains('\n') && !menu.insertion.multiline {
            outcome.notice = Some(MULTILINE_REFUSED);
            return outcome;
        }
        if phrase.text.contains('\n') && menu.insertion.keystrokes {
            outcome.keystrokes = Some(phrase.text.clone());
            return outcome;
        }
        outcome.commit = Some(phrase.text.replace('\n', menu.insertion.newline)).filter(|t| !t.is_empty());
        outcome
    }

    // -- deciding ----------------------------------------------------------

    /// `c` as typed in the current width.
    fn width(&self, c: char) -> char {
        if self.full_width { full_width(c) } else { c }
    }

    /// A key with nothing composed: the application's, unless full width
    /// changes what it types.
    fn idle_key(&self, typed: Option<char>) -> Action {
        match typed {
            Some(c) if self.full_width => Action::Type(full_width(c)),
            _ => Action::Pass,
        }
    }

    /// The position on the page that this key picks, if it is a selection key.
    fn selection_key(&self, key: Key) -> Option<usize> {
        let c = ascii(key.vk, false)?;
        self.options.labels().iter().position(|&label| label == c)
    }

    fn classify(&self, key: Key) -> Action {
        // English mode is the keyboard as it is: Shift+Space included,
        // which is easily pressed while typing capitals.
        if !self.enabled || !self.chinese || is_modifier(key.vk) || key.alt {
            return Action::Pass;
        }
        if key.vk == vk::SPACE && key.shift && !key.ctrl {
            return Action::ToggleWidth;
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
        let typed = ascii(key.vk, key.shift);
        // Caps Lock means the user wants Latin letters.
        if key.caps && !busy {
            return self.idle_key(typed_with_caps(key));
        }
        let idle = if busy { Action::Swallow } else { Action::Pass };

        if self.prefix {
            if let Some(c) = typed.filter(char::is_ascii_punctuation) {
                return Action::Send(Op::Symbol(c));
            }
            // Anything else cancels the prefix and is handled as usual.
        }

        if self.window_open && !key.shift {
            if let Some(position) = self.selection_key(key) {
                let index = self.page_start() + position;
                return if index < self.candidates.len() {
                    Action::SelectIndex(index)
                } else {
                    Action::Swallow
                };
            }
            match key.vk {
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
                    Action::Send(Op::Text(self.width(c).to_string()))
                } else if !busy {
                    self.idle_key(Some(c))
                } else {
                    idle
                };
            }
        }
        if key.shift {
            return match typed.filter(char::is_ascii_punctuation) {
                Some(c) if composing && self.pending.is_empty() => {
                    Action::Send(Op::Text(self.width(c).to_string()))
                }
                Some(c) if !busy => self.idle_key(Some(c)),
                _ => idle,
            };
        }

        if let Some(c) = layout_char(key.vk) {
            let has_pending = !self.pending.is_empty();
            return match c {
                // First tone, or a space inside the composition.
                ' ' if composing => Action::Send(Op::Key(' ')),
                ' ' if !busy => self.idle_key(Some(' ')),
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
        let numpad = (vk::NUMPAD0..=vk::NUMPAD9)
            .contains(&key.vk)
            .then(|| char::from_u32(key.vk - vk::NUMPAD0 + 0x30).unwrap_or('0'));
        if !composing {
            return if busy { idle } else { self.idle_key(numpad) };
        }
        let settled = self.pending.is_empty();
        match key.vk {
            vk::NUMPAD0..=vk::NUMPAD9 if settled => {
                Action::Send(Op::Text(self.width(numpad.unwrap_or('0')).to_string()))
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
        if let Some(action) = self.menu_action(key) {
            return action != Action::Pass;
        }
        self.classify(key) != Action::Pass
    }

    /// True if this key is about to start a new composition, which is the
    /// moment to report the text before the cursor with `set_context`.
    pub fn starts_composition(&self, key: Key) -> bool {
        self.menu_action(key).is_none()
            && !self.composing()
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

    fn ask(&mut self, op: &Op) -> Result<Response, Failure> {
        self.sent.push(op.name());
        self.engine.request(op)
    }

    /// The requests made since the last call, e.g. `["key"]` or `["state", "tone"]`.
    pub fn take_sent(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.sent)
    }

    /// One request, with the bookkeeping that keeps both sides in step.
    fn exchange(&mut self, op: &Op) -> Result<Response, Failure> {
        let mut result = self.replay();
        if result.is_ok() {
            result = self.exchange_inner(op).map(Some);
        }
        if result == Err(Failure::Starting) {
            if self.starting_refused_until.is_some_and(|until| Instant::now() < until) {
                // We gave up waiting for this engine a moment ago.
                result = Err(Failure::Unreachable);
            } else if !self.keep(op) {
                // Waited too long: what is shown goes to the document.
                self.lost();
                self.starting_refused_until = Some(Instant::now() + STARTING_COOLDOWN);
            }
        }
        match &result {
            // Every response carries the whole state: back in step.
            Ok(_) => self.behind = 0,
            Err(Failure::TimedOut) => self.behind += 1,
            Err(_) => {}
        }
        result.map(|response| response.unwrap_or_default())
    }

    /// Send what was kept while the engine was starting. `Ok(None)` when
    /// there is nothing (left) to send.
    fn replay(&mut self) -> Result<Option<Response>, Failure> {
        while let Some(op) = self.queued.first().cloned() {
            match self.exchange_inner(&op) {
                Ok(_) => {
                    self.queued.remove(0);
                }
                Err(Failure::Starting) => return Err(Failure::Starting),
                // The engine answered, but not this; the rest still counts.
                Err(Failure::Rejected) => {
                    self.queued.remove(0);
                }
                Err(failure) => {
                    self.queued.clear();
                    self.queued_since = None;
                    return Err(failure);
                }
            }
        }
        self.queued_since = None;
        Ok(None)
    }

    /// Keep a request for when the engine is ready, and show its effect
    /// meanwhile. False if the engine has kept us waiting too long.
    fn keep(&mut self, op: &Op) -> bool {
        let since = *self.queued_since.get_or_insert_with(Instant::now);
        if since.elapsed() > self.starting_limit {
            return false;
        }
        match op {
            Op::Key(c) => {
                if is_tone(*c) && !self.pending.is_empty() {
                    self.pending.clear();
                    self.preedit.extend(dachen_symbol(*c));
                } else if *c == ' ' {
                    self.preedit.push(' ');
                } else if let Some(symbol) = dachen_symbol(*c) {
                    self.pending.push(*c);
                    self.preedit.push(symbol);
                }
            }
            Op::Symbol(c) => self.preedit.push(*c),
            Op::Text(text) => self.preedit.push_str(text),
            Op::Backspace => {
                self.pending.pop();
                self.preedit.pop();
            }
            Op::Escape => {
                // Nothing kept is wanted any more.
                self.queued.clear();
                self.preedit.clear();
                self.pending.clear();
                self.queued_since = None;
                self.caret = 0;
                return true;
            }
            // The text before the cursor matters only before anything else.
            Op::Context(_) if self.queued.is_empty() => {}
            Op::Commit => {}
            // Moving, choosing and asking need the engine's state: dropped.
            _ => return true,
        }
        self.queued.push(op.clone());
        self.caret = self.preedit.chars().count();
        true
    }

    /// Try again to send what was kept while the engine was starting.
    /// Returns text the engine committed meanwhile.
    pub fn flush(&mut self) -> Option<String> {
        if self.queued.is_empty() {
            return None;
        }
        if self.exchange(&Op::State).is_err() && self.queued_since.is_some_and(|t| t.elapsed() > self.starting_limit) {
            self.lost();
            self.starting_refused_until = Some(Instant::now() + STARTING_COOLDOWN);
        }
        self.take_committed()
    }

    fn exchange_inner(&mut self, op: &Op) -> Result<Response, Failure> {
        if self.stale {
            // The first Escape only drops an unfinished syllable.
            self.ask(&Op::Escape)?;
            self.ask(&Op::Escape)?;
            self.stale = false;
        }
        if self.learning_unsent {
            self.ask(&Op::Learning(self.enabled))?;
            self.learning_unsent = false;
        }
        if let Some(wanted) = self.wanted_style.clone().filter(|w| self.style.as_ref() != Some(w)) {
            let response = self.ask(&Op::Style(wanted.clone()))?;
            self.absorb(&response);
            // An engine that does not report its style is not asked again.
            self.style.get_or_insert(wanted);
        }
        let response = self.ask(op)?;
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
        match self.ask(&Op::State) {
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
            // It answers (or restarts), so nothing is outstanding any more.
            Err(Failure::Rejected | Failure::Starting) => self.behind = 0,
        }
        false
    }

    fn clear(&mut self) {
        self.queued.clear();
        self.queued_since = None;
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
        self.menu = None;
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
        self.menu = None;
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
        match self.menu_action(key) {
            Some(Action::Pass) => return Outcome::default(),
            Some(action) => return self.menu_key(action),
            // Any other key closes the menu and does what it always does.
            None => self.menu = None,
        }
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
                | Action::ToggleWidth
        ) {
            self.window_open = false;
        }
        let mut outcome = Outcome { eaten: action != Action::Pass, ..Outcome::default() };
        let count = self.candidates.len().max(1);
        let sent = match action {
            Action::Pass | Action::Swallow | Action::CloseWindow => Ok(()),
            Action::ToggleWidth => {
                self.full_width = !self.full_width;
                outcome.width_changed = true;
                Ok(())
            }
            Action::Type(c) => {
                self.committed.push(c);
                Ok(())
            }
            Action::StartPrefix => {
                self.prefix = true;
                Ok(())
            }
            Action::MoveSelection(step) => {
                self.selected = (self.selected + count).wrapping_add_signed(step) % count;
                Ok(())
            }
            Action::NextPage => {
                self.turn_page(1);
                Ok(())
            }
            Action::PrevPage => {
                self.turn_page(-1);
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
            // Kept for when the engine is ready (or given up, with the
            // shown text committed). Either way the key was ours.
            Err(Failure::Starting) => outcome.eaten = was_busy || self.composing() || outcome.eaten,
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
        let alone = std::mem::take(&mut self.shift_alone);
        if is_shift(key.vk) && alone && self.enabled && self.options.shift_toggles {
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

    /// Take on a mode chosen elsewhere (another application, when the mode
    /// is shared). Like a switch, it ends a composition in progress.
    pub fn set_chinese(&mut self, chinese: bool) -> Option<String> {
        if chinese == self.chinese {
            return None;
        }
        self.chinese = chinese;
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
        /// The host answers that the engine is still loading.
        starting: bool,
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
            if self.starting {
                return Err(Failure::Starting);
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
        Outcome { eaten: true, commit: commit.map(str::to_owned), ..Outcome::default() }
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
    fn mouse_and_application_choices() {
        let mut c = typed("ji3");
        assert_eq!(c.pick(0), Outcome::default(), "no window, nothing to pick");
        c.key_down(code(vk::DOWN));
        assert_eq!(c.page_starts(), [0, 9]);
        c.turn_page(1);
        assert_eq!(c.candidate_list().unwrap().1, 9);
        c.turn_page(1);
        assert_eq!(c.candidate_list().unwrap().1, 0, "wraps");
        c.highlight(11);
        c.highlight(99);
        assert_eq!(c.candidate_list().unwrap().1, 11);
        assert_eq!(c.pick(11), eaten(None));
        assert!(c.candidate_list().is_none());
        assert_eq!(c.engine_mut().log.last(), Some(&Op::Select(11)));
        c.key_down(code(vk::DOWN));
        c.close_candidates();
        assert!(c.candidate_page().is_none());
        assert!(c.composing());
    }

    #[test]
    fn mode_from_elsewhere() {
        let mut c = typed("ji3");
        assert_eq!(c.set_chinese(true), None);
        assert_eq!(c.set_chinese(false).as_deref(), Some("字"));
        assert!(!c.chinese());
        assert!(!c.test_key(key('j')));
        assert_eq!(c.set_chinese(true), None);
        assert!(c.test_key(key('j')));
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
        assert_eq!(outcome, Outcome { eaten: true, style_changed: true, ..Outcome::default() });
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
    fn keys_wait_for_a_starting_engine() {
        let mut c = Controller::new(Fake { starting: true, ..Fake::default() });
        assert!(c.key_down(key('j')).eaten, "kept, not given to the application");
        c.key_down(key('i'));
        c.key_down(key('3'));
        c.key_down(key('j'));
        assert_eq!(c.preedit(), "ㄨㄛˇㄨ");
        c.key_down(code(vk::BACK));
        assert_eq!(c.preedit(), "ㄨㄛˇ");
        assert!(c.waiting());
        assert!(c.engine_mut().log.is_empty(), "nothing reached the engine");

        // Ready: everything is sent in order, and the engine's state shows.
        c.engine_mut().starting = false;
        assert_eq!(c.flush(), None);
        assert!(!c.waiting());
        assert_eq!(c.preedit(), "字");
        assert_eq!(
            c.engine_mut().log,
            [Op::Key('j'), Op::Key('i'), Op::Key('3'), Op::Key('j'), Op::Backspace, Op::State]
        );

        // Enter while starting: the commit comes once the engine is ready.
        let mut c = Controller::new(Fake { starting: true, ..Fake::default() });
        for k in "ji3".chars() {
            c.key_down(key(k));
        }
        assert_eq!(c.key_down(code(vk::RETURN)), eaten(None));
        c.engine_mut().starting = false;
        assert_eq!(c.flush().as_deref(), Some("字"));
        assert!(!c.composing());
    }

    #[test]
    fn a_starting_engine_is_not_waited_for_forever() {
        let mut c = Controller::new(Fake { starting: true, ..Fake::default() });
        c.starting_limit = Duration::from_millis(2);
        c.key_down(key('j'));
        std::thread::sleep(Duration::from_millis(5));
        // Shown text goes to the document; later keys go to the application.
        assert_eq!(c.key_down(key('i')), eaten(Some("ㄨ")));
        assert!(!c.composing());
        assert_eq!(c.key_down(key('j')), Outcome::default());
    }

    #[test]
    fn options_from_the_settings_window() {
        assert_eq!(Options::parse(""), Options::default());
        let options = Options::parse(
            r#"{"page_size": 6, "select_keys": "home_row", "start_mode": "english", "shift_toggle": false}"#,
        );
        assert_eq!(
            options,
            Options {
                page_size: 6,
                home_row: true,
                start_chinese: false,
                shift_toggles: false,
                phrase_key: Some(Hotkey::PHRASES),
            }
        );
        // The phrase key: another combination, turned off, or refused without Ctrl or Alt.
        let ctrl_q = Options::parse(r#"{"phrase_key": {"vk": 81, "ctrl": true}}"#).phrase_key;
        assert_eq!(ctrl_q, Some(Hotkey { vk: 0x51, ctrl: true, alt: false, shift: false }));
        assert_eq!(Options::parse(r#"{"phrase_key": null}"#).phrase_key, None);
        let bare = Options::parse(r#"{"phrase_key": {"vk": 81, "shift": true}}"#).phrase_key;
        assert_eq!(bare, Some(Hotkey::PHRASES), "falls back to the default");
        assert_eq!(options.labels(), ['a', 's', 'd', 'f', 'g', 'h']);
        assert_eq!(Options::parse(r#"{"page_size": 40}"#).page_size, 9, "out of range");
        assert_eq!(Options { page_size: 10, ..Options::default() }.labels().last(), Some(&'0'));
    }

    #[test]
    fn selection_keys_and_page_size() {
        let mut c = Controller::new(Fake::default());
        c.set_options(Options { page_size: 5, home_row: true, ..Options::default() });
        for k in "ji3".chars() {
            c.key_down(key(k));
        }
        c.key_down(code(vk::DOWN));
        let (items, _, page, pages) = c.candidate_page().unwrap();
        assert_eq!((items.len(), page, pages), (5, 0, 3));
        assert_eq!(c.labels(), ['a', 's', 'd', 'f', 'g']);
        c.key_down(key(' '));
        assert_eq!(c.page_start(), 5);
        // D picks the third on the second page; a digit is not a selection key.
        assert!(c.test_key(key('d')));
        c.key_down(key('d'));
        assert_eq!(c.engine_mut().log.last(), Some(&Op::Select(7)));
    }

    #[test]
    fn shift_alone_can_be_turned_off() {
        let mut c = Controller::new(Fake::default());
        c.set_options(Options { shift_toggles: false, ..Options::default() });
        c.key_down(code(vk::SHIFT));
        c.key_up(code(vk::SHIFT));
        assert!(c.chinese());
    }

    #[test]
    fn full_width() {
        let mut c = Controller::new(Fake::default());
        let shift_space = Key { shift: true, ..key(' ') };
        assert!(c.test_key(shift_space));
        let outcome = c.key_down(shift_space);
        assert!(outcome.eaten && outcome.width_changed);
        assert!(c.full_width());

        // Chinese mode, nothing composed: space, Shift letters and symbols, Caps Lock.
        assert_eq!(c.key_down(key(' ')), eaten(Some("\u{3000}")));
        assert_eq!(c.key_down(Key { shift: true, ..key('a') }), eaten(Some("Ａ")));
        assert_eq!(c.key_down(Key { shift: true, ..key('1') }), eaten(Some("！")));
        assert_eq!(c.key_down(Key { caps: true, ..key('b') }), eaten(Some("Ｂ")));
        assert_eq!(c.key_down(code(vk::NUMPAD0 + 7)), eaten(Some("７")));
        // Bopomofo keys still compose, and Ctrl shortcuts stay the application's.
        assert_eq!(c.key_down(key('j')), eaten(None));
        assert_eq!(c.preedit(), "j");
        c.key_down(code(vk::ESCAPE));
        assert!(!c.test_key(Key { ctrl: true, ..key('c') }));

        // English mode is untouched, Shift+Space included.
        c.toggle_chinese();
        assert!(!c.full_width());
        assert!(!c.test_key(key('j')));
        assert!(!c.test_key(shift_space));
        c.toggle_chinese();
        assert!(c.full_width(), "kept for when Chinese mode returns");

        // And back to half width.
        c.key_down(shift_space);
        assert!(!c.full_width());
        assert!(!c.test_key(key(' ')));
        assert!(c.engine_mut().log.iter().all(|op| !matches!(op, Op::Text(_))), "nothing typed went to the engine");
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

    fn phrases() -> Vec<Phrase> {
        parse_phrases(
            r#"{"phrases": [
                {"name": "", "text": "地址：台北市"},
                {"name": "簽名", "text": "祝好\r\n小明"},
                {"name": "", "text": "第一行\n第二行"},
                {"name": "空的", "text": ""}
            ]}"#,
        )
    }

    const EVERYWHERE: Insertion = Insertion { newline: "\r\n", multiline: true, keystrokes: false };

    #[test]
    fn phrase_file_and_labels() {
        let list = phrases();
        assert_eq!(list.len(), 3, "entries without text are skipped");
        assert_eq!(list[1].text, "祝好\n小明", "line breaks are kept as \\n");
        let labels: Vec<String> = list.iter().map(Phrase::label).collect();
        assert_eq!(labels, ["地址：台北市", "簽名", "第一行…"]);
        let long = Phrase { name: String::new(), text: "一二三四五六七八九十一二三四五六七八九十多".into() };
        assert_eq!(long.label().chars().count(), PHRASE_LABEL_CHARS + 1);
        assert!(parse_phrases("not json").is_empty());
    }

    #[test]
    fn phrase_menu() {
        let mut c = Controller::new(Fake::default());
        assert!(c.open_menu(phrases(), EVERYWHERE).eaten);
        assert!(c.menu_open());
        let (items, selected) = c.candidate_list().unwrap();
        assert_eq!((items.len(), selected), (3, 0));
        // Arrows move, Shift is left alone, 2 picks the second phrase.
        assert!(c.key_down(code(vk::DOWN)).eaten);
        assert!(!c.test_key(code(vk::SHIFT)));
        assert!(c.menu_open());
        assert_eq!(c.key_down(key('2')), eaten(Some("祝好\r\n小明")));
        assert!(!c.menu_open());
        assert!(c.engine_mut().log.is_empty(), "phrases never reach the engine");

        // Enter picks the highlighted one; Esc closes without anything.
        c.open_menu(phrases(), EVERYWHERE);
        c.key_down(code(vk::DOWN));
        c.key_down(code(vk::DOWN));
        assert_eq!(c.key_down(code(vk::RETURN)), eaten(Some("第一行\r\n第二行")));
        c.open_menu(phrases(), EVERYWHERE);
        assert_eq!(c.key_down(code(vk::ESCAPE)), eaten(None));
        assert!(!c.menu_open());

        // A key the menu does not use closes it and does what it always does.
        c.open_menu(phrases(), EVERYWHERE);
        assert!(c.test_key(key('j')), "a bopomofo key: the menu closes, the key composes");
        c.key_down(key('j'));
        assert!(!c.menu_open());
        assert_eq!(c.preedit(), "j");

        // Opening it while composing commits the composition first.
        c.key_down(key('i'));
        c.key_down(key('3'));
        let outcome = c.open_menu(phrases(), EVERYWHERE);
        assert_eq!(outcome.commit.as_deref(), Some("字"));
        assert!(!c.composing() && c.menu_open());

        // A key the application gets closes the menu before it goes.
        assert!(!c.test_key(Key { ctrl: true, ..key('c') }));
        assert!(c.dismiss_menu(Key { ctrl: true, ..key('c') }));
        assert!(!c.menu_open());

        // The mouse picks through the same list.
        c.open_menu(phrases(), EVERYWHERE);
        assert_eq!(c.pick(0), eaten(Some("地址：台北市")));
    }

    #[test]
    fn phrase_menu_in_english_mode_and_terminals() {
        let mut c = Controller::new(Fake::default());
        c.toggle_chinese();
        let terminal = Insertion { newline: "\r\n", multiline: false, keystrokes: false };
        c.open_menu(phrases(), terminal);
        assert!(c.test_key(key('1')), "the menu works in English mode too");
        let refused = c.key_down(key('3'));
        assert!(refused.eaten && refused.commit.is_none());
        assert_eq!(refused.notice, Some(MULTILINE_REFUSED));
        c.open_menu(phrases(), terminal);
        assert_eq!(c.key_down(key('1')), eaten(Some("地址：台北市")), "one line is fine");

        // Browsers: several lines are typed, one line is inserted.
        let browser = Insertion { newline: "\n", multiline: true, keystrokes: true };
        c.open_menu(phrases(), browser);
        let typed = c.key_down(key('3'));
        assert!(typed.eaten && typed.commit.is_none());
        assert_eq!(typed.keystrokes.as_deref(), Some("第一行\n第二行"));
        c.open_menu(phrases(), browser);
        assert_eq!(c.key_down(key('1')), eaten(Some("地址：台北市")));

        // Not in a password field, and not with nothing to show.
        c.set_enabled(false);
        assert!(!c.open_menu(phrases(), EVERYWHERE).eaten);
        c.set_enabled(true);
        assert!(!c.open_menu(Vec::new(), EVERYWHERE).eaten);
        assert!(!c.menu_open());
    }
}
