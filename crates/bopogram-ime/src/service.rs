//! The TSF text input processor.
//!
//! One instance per application thread that uses the input method. It
//! forwards keys to the controller and draws what the controller holds:
//! the composition in the document and the candidate window.
//!
//! TSF can call back into this object while one of its own calls is in
//! progress (an edit session runs inside `RequestEditSession`, for one).
//! So the state is only ever borrowed for short stretches that make no TSF
//! calls, and a borrow that cannot be had is skipped rather than panicking.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::core::{implement, IUnknownImpl, Interface, Ref, Result, BOOL, GUID};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, VK_CAPITAL, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, IEnumTfDisplayAttributeInfo, ITfCategoryMgr, ITfComposition,
    ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
    ITfDisplayAttributeInfo, ITfDisplayAttributeProvider, ITfDisplayAttributeProvider_Impl,
    ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfTextInputProcessor_Impl,
    ITfLangBarItemButton, ITfLangBarItemMgr, ITfLangBarItemSink, ITfSource, ITfThreadFocusSink,
    ITfThreadFocusSink_Impl, ITfThreadMgr, ITfUIElement,
    ITfUIElementMgr, TF_ES_ASYNCDONTCARE, TF_ES_READWRITE, TF_LBI_ICON, TF_LBI_TEXT,
    TF_MOD_ALT, TF_MOD_CONTROL, TF_MOD_SHIFT, TF_PRESERVEDKEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetGUIThreadInfo, GetWindowLongW, KillTimer, SetTimer,
    GUITHREADINFO, GWL_STYLE,
};

use crate::controller::{vk, Controller, Hotkey, Key, CASUAL, FORMAL};
use crate::display::{AttributeEnumerator, CompositionAttribute};
use crate::dll::{host_path, settings_path, GUID_DISPLAY_ATTRIBUTE, GUID_PHRASE_KEY};
use crate::langbar::ModeButton;
use crate::pipe::PipeEngine;
use crate::popup::{Event, Handler, Popup};
use crate::{candidates, edit, guard, settings, timing, wide};

/// Edit control style: the control shows dots instead of the text.
const ES_PASSWORD: i32 = 0x0020;

#[derive(Default)]
pub(crate) struct State {
    thread_mgr: Option<ITfThreadMgr>,
    client: u32,
    /// `None` when the engine can never be reached from this process
    /// (service accounts); every key then goes to the application.
    controller: Option<Controller<PipeEngine>>,
    composition: Option<ITfComposition>,
    /// Display attribute of the composition, as TSF's atom for its GUID. 0 if unavailable.
    attribute: u32,
    popup: Option<Popup>,
    /// Where the composition is on the screen, for placing the popup.
    anchor: Option<RECT>,
    /// The preedit and caret last written to the document.
    shown: String,
    shown_caret: usize,
    /// Last answer about the focused field being a password field.
    password: bool,
    /// The mode button in the input indicator, and whoever wants to hear
    /// that it changed.
    button: Option<ITfLangBarItemButton>,
    /// Our registration for hearing that this thread got the focus.
    focus_cookie: Option<u32>,
    button_sink: Option<ITfLangBarItemSink>,
    /// This text service as the owner of its compositions.
    sink: Option<ITfCompositionSink>,
    /// The candidate list announced to TSF while the window is open.
    candidate_ui: Option<CandidateUi>,
    /// What we committed lately, per document (most recent last), for
    /// documents whose text cannot be read: the engine gets it as context.
    recent: Vec<(usize, String)>,
    /// The phrase key as registered with TSF, if it is.
    preserved: Option<Hotkey>,
    /// When the phrase key last arrived (held keys repeat; see `phrase_key`).
    phrase_key_at: Option<Instant>,
    /// The phrase key had Alt in it: its release is eaten, or the
    /// application would take Alt pressed alone and open its menu bar.
    eat_alt_up: bool,
    /// Where the caret was when the phrase menu opened.
    menu_anchor: Option<RECT>,
}

/// Documents remembered in `State::recent`, and characters kept for each.
const RECENT_DOCUMENTS: usize = 8;
const RECENT_CHARS: usize = 40;

/// Identifies a document while it exists.
fn document_key(context: &ITfContext) -> usize {
    unsafe { context.GetDocumentMgr() }.map_or(0, |document| document.as_raw() as usize)
}

fn remember_commit(state: &Shared, context: &ITfContext, text: &str) {
    let key = document_key(context);
    with(state, |s| {
        let mut kept = s
            .recent
            .iter()
            .position(|(k, _)| *k == key)
            .map(|i| s.recent.remove(i).1)
            .unwrap_or_default();
        kept.push_str(text);
        let skip = kept.chars().count().saturating_sub(RECENT_CHARS);
        let kept: String = kept.chars().skip(skip).collect();
        s.recent.push((key, kept));
        if s.recent.len() > RECENT_DOCUMENTS {
            s.recent.remove(0);
        }
    });
}

fn recent_text(state: &Shared, context: &ITfContext) -> String {
    let key = document_key(context);
    with(state, |s| s.recent.iter().find(|(k, _)| *k == key).map(|(_, t)| t.clone()))
        .flatten()
        .unwrap_or_default()
}

thread_local! {
    /// Text services with keys waiting for a starting engine.
    static RETRY: RefCell<Vec<std::rc::Weak<RefCell<State>>>> = const { RefCell::new(Vec::new()) };
}

/// How often kept keys are offered to a starting engine.
const RETRY_MS: u32 = 250;

/// Offer the kept keys again shortly, if there are any.
fn retry_later(state: &Shared) {
    if !with_controller(state, |c| c.waiting()).unwrap_or(false) {
        return;
    }
    let first = RETRY.with(|retry| {
        let mut retry = retry.borrow_mut();
        let first = retry.is_empty();
        if !retry.iter().any(|w| w.ptr_eq(&Rc::downgrade(state))) {
            retry.push(Rc::downgrade(state));
        }
        first
    });
    if first {
        unsafe {
            SetTimer(None, 0, RETRY_MS, Some(retry_timer));
        }
    }
}

unsafe extern "system" fn retry_timer(_window: HWND, _message: u32, id: usize, _time: u32) {
    guard((), || unsafe {
        let _ = KillTimer(None, id);
        let waiting: Vec<_> = RETRY.with(|retry| retry.borrow_mut().drain(..).collect());
        for state in waiting.iter().filter_map(std::rc::Weak::upgrade) {
            let Some(commit) = with_controller(&state, |c| c.flush()) else { continue };
            if let Some(context) = document_context(&state) {
                refresh(&state, &context, commit, None);
            }
            retry_later(&state);
        }
    })
}

struct CandidateUi {
    /// `None` if TSF did not take the element; our window is then shown.
    id: Option<u32>,
    /// False while the application draws the candidates itself.
    shown: bool,
}

pub(crate) type Shared = Rc<RefCell<State>>;

// -- what the mode button needs ----------------------------------------------

pub(crate) fn is_full_width(state: &Shared) -> bool {
    with_controller(state, |c| c.full_width()).unwrap_or(false)
}

pub(crate) fn is_chinese(state: &Shared) -> bool {
    with_controller(state, |c| c.chinese()).unwrap_or(true)
}

/// For the taskbar asking what to show: the shared mode is taken on first,
/// so that it never shows a mode this application is about to leave. No
/// notification is sent from here (the taskbar is asking already).
pub(crate) fn current_mode(state: &Shared) -> bool {
    if settings::shared_mode_cached() && with_controller(state, |c| !c.composing()).unwrap_or(false) {
        if let Some(chinese) = settings::load_mode() {
            with_controller(state, |c| c.set_chinese(chinese));
        }
    }
    is_chinese(state)
}

thread_local! {
    /// Text services waiting for the delayed button update (see `button_changed_later`).
    static LATER: RefCell<Vec<std::rc::Weak<RefCell<State>>>> = const { RefCell::new(Vec::new()) };
}

/// Delay of the second button update after the focus arrives.
const LATER_MS: u32 = 150;

/// Announce the button again shortly. When the focus moves to this
/// application the taskbar may still be busy with the move and miss an
/// update sent at once.
fn button_changed_later(state: &Shared) {
    LATER.with(|later| later.borrow_mut().push(Rc::downgrade(state)));
    unsafe {
        SetTimer(None, 0, LATER_MS, Some(later_timer));
    }
}

unsafe extern "system" fn later_timer(_window: HWND, _message: u32, id: usize, _time: u32) {
    guard((), || unsafe {
        let _ = KillTimer(None, id);
        let waiting: Vec<_> = LATER.with(|later| later.borrow_mut().drain(..).collect());
        for state in waiting.iter().filter_map(std::rc::Weak::upgrade) {
            button_changed(&state);
        }
    })
}

pub(crate) fn is_formal(state: &Shared) -> bool {
    with_controller(state, |c| c.style() == FORMAL).unwrap_or(false)
}

pub(crate) fn has_button_sink(state: &Shared) -> bool {
    with(state, |s| s.button_sink.is_some()).unwrap_or(false)
}

pub(crate) fn set_button_sink(state: &Shared, sink: Option<ITfLangBarItemSink>) {
    with(state, |s| s.button_sink = sink);
}

/// Tell the input indicator that the button looks different now.
fn button_changed(state: &Shared) {
    if let Some(sink) = with(state, |s| s.button_sink.clone()).flatten() {
        unsafe {
            let _ = sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT);
        }
    }
}

/// Switch Chinese/English from the button. A composition in progress is
/// committed to the focused document.
pub(crate) fn toggle_mode(state: &Shared) {
    let Some((commit, chinese)) = with_controller(state, |c| (c.toggle_chinese(), c.chinese())) else {
        return;
    };
    mode_switched(chinese);
    if let Some(context) = document_context(state) {
        refresh(state, &context, commit, None);
    }
    button_changed(state);
}

/// The user switched Chinese/English here. When the mode is shared, the
/// other applications take it on when they next get the focus.
fn mode_switched(chinese: bool) {
    if settings::shared_mode() {
        settings::save_mode(chinese);
    }
}

/// The same, before a key is judged: a safety net in case the focus
/// notification did not come. Nothing is composed, so nothing is committed.
fn adopt_shared_mode_if_idle(state: &Shared) {
    if with_controller(state, |c| !c.composing()).unwrap_or(false) && settings::shared_mode_cached() {
        let before = with_controller(state, |c| c.chinese());
        adopt_shared_mode(state);
        if before != with_controller(state, |c| c.chinese()) {
            button_changed(state);
        }
    }
}

/// Take on the shared mode, if the mode is shared and differs from ours.
fn adopt_shared_mode(state: &Shared) {
    if !settings::shared_mode() {
        return;
    }
    let Some(chinese) = settings::load_mode() else {
        return;
    };
    let Some(commit) = with_controller(state, |c| c.set_chinese(chinese)) else {
        return;
    };
    if let Some(text) = commit {
        if let Some(context) = document_context(state) {
            refresh(state, &context, Some(text), None);
        }
    }
}

/// The document being composed into, or else the focused one.
fn document_context(state: &Shared) -> Option<ITfContext> {
    let (composition, thread_mgr) =
        with(state, |s| (s.composition.clone(), s.thread_mgr.clone()))?;
    unsafe {
        if let Some(context) = composition.and_then(|c| c.GetRange().and_then(|r| r.GetContext()).ok()) {
            return Some(context);
        }
        thread_mgr?.GetFocus().and_then(|document| document.GetTop()).ok()
    }
}

// -- the candidate list, for the mouse and for applications that draw it ---

pub(crate) fn candidate_list(state: &Shared) -> Option<(Vec<String>, usize)> {
    with_controller(state, |c| c.candidate_list().map(|(words, selected)| (words.to_vec(), selected)))
        .flatten()
}

/// Where each page of the candidate list starts.
pub(crate) fn page_starts(state: &Shared) -> Vec<usize> {
    with_controller(state, |c| c.page_starts()).unwrap_or_else(|| vec![0])
}

/// Where the page with the highlighted candidate starts.
pub(crate) fn page_start(state: &Shared) -> usize {
    with_controller(state, |c| c.page_start()).unwrap_or(0)
}

pub(crate) fn candidate_window_shown(state: &Shared) -> bool {
    with(state, |s| s.candidate_ui.as_ref().is_none_or(|ui| ui.shown)).unwrap_or(true)
}

/// The application decides whether our candidate window is shown.
pub(crate) fn show_candidate_window(state: &Shared, show: bool) {
    with(state, |s| {
        if let Some(ui) = s.candidate_ui.as_mut() {
            ui.shown = show;
        }
    });
    draw_candidates(state);
}

pub(crate) fn highlight_candidate(state: &Shared, index: usize) {
    with_controller(state, |c| c.highlight(index));
    draw_candidates(state);
}

/// Choose a candidate without a key: the composition changes, so the
/// document is written in an edit session of its own.
pub(crate) fn pick_candidate(state: &Shared, index: usize) {
    let Some(outcome) = with_controller(state, |c| c.pick(index)) else {
        return;
    };
    if let Some(context) = document_context(state) {
        refresh(state, &context, outcome.commit, None);
    }
    if let Some(notice) = outcome.notice {
        tip(state, &notice);
    }
}

pub(crate) fn close_candidates(state: &Shared) {
    with_controller(state, |c| c.close_candidates());
    refresh_candidates(state);
}

/// A click or the wheel in our candidate window.
fn mouse(state: &Shared, event: Event) {
    match event {
        Event::Pick(row) => {
            let start = with_controller(state, |c| c.candidate_list().is_some().then(|| c.page_start())).flatten();
            if let Some(start) = start {
                pick_candidate(state, start + row);
            }
        }
        Event::Page(delta) => {
            with_controller(state, |c| c.turn_page(delta));
            refresh_candidates(state);
        }
    }
}

/// The popup, created on first use. It reports the mouse back here.
fn ensure_popup(state: &Shared) {
    if with(state, |s| s.popup.is_some()).unwrap_or(true) {
        return;
    }
    let weak = Rc::downgrade(state);
    let handler: Handler = Rc::new(move |event| {
        if let Some(state) = weak.upgrade() {
            mouse(&state, event);
        }
    });
    let popup = Popup::new(Some(handler));
    with(state, |s| s.popup = popup);
}

/// Announce, update or withdraw the candidate list, then draw it.
fn refresh_candidates(state: &Shared) {
    let Some((open, thread_mgr, announced)) = with(state, |s| {
        let open = s.controller.as_ref().is_some_and(|c| c.candidate_list().is_some());
        (open, s.thread_mgr.clone(), s.candidate_ui.as_ref().map(|ui| ui.id))
    }) else {
        return;
    };
    let manager = thread_mgr.and_then(|t| t.cast::<ITfUIElementMgr>().ok());
    unsafe {
        match (open, announced) {
            (false, Some(id)) => {
                with(state, |s| s.candidate_ui = None);
                if let (Some(manager), Some(id)) = (&manager, id) {
                    let _ = manager.EndUIElement(id);
                }
            }
            (true, None) => {
                let document = document_context(state).and_then(|c| c.GetDocumentMgr().ok());
                let element: ITfUIElement = candidates::element(state, document);
                let mut show = BOOL::from(true);
                let mut id = 0u32;
                let begun = manager
                    .as_ref()
                    .is_some_and(|m| m.BeginUIElement(&element, &mut show, &mut id).is_ok());
                let ui = if begun {
                    CandidateUi { id: Some(id), shown: show.as_bool() }
                } else {
                    CandidateUi { id: None, shown: true }
                };
                with(state, |s| s.candidate_ui = Some(ui));
            }
            (true, Some(Some(id))) => {
                if let Some(manager) = &manager {
                    let _ = manager.UpdateUIElement(id);
                }
            }
            _ => {}
        }
    }
    draw_candidates(state);
}

/// Our window, if the candidates are open and the application lets us show it.
fn draw_candidates(state: &Shared) {
    ensure_popup(state);
    with(state, |s| {
        let shown = s.candidate_ui.as_ref().is_none_or(|ui| ui.shown);
        let labels = s.controller.as_ref().map(|c| c.labels()).unwrap_or_default();
        let page = s.controller.as_ref().and_then(|c| c.candidate_page());
        let menu = s.controller.as_ref().is_some_and(|c| c.menu_open());
        let anchor = if menu { s.menu_anchor.or(s.anchor) } else { s.anchor };
        let Some(popup) = &s.popup else { return };
        match page {
            Some((items, selected, page, pages)) if shown => popup.show(
                items,
                &labels,
                selected,
                page,
                pages,
                anchor.unwrap_or_else(fallback_anchor),
            ),
            _ => popup.hide(),
        }
    });
}

/// Bring the document and the candidate window in line with the controller.
fn refresh(
    state: &Shared,
    context: &ITfContext,
    commit: Option<String>,
    mut timing: Option<&mut timing::Key>,
) {
    let Some((client, sink, preedit, caret, unchanged)) = with(state, |s| {
        let controller = s.controller.as_ref()?;
        let preedit = controller.preedit().to_owned();
        let caret = controller.caret_units();
        let unchanged = commit.is_none() && preedit == s.shown && caret == s.shown_caret;
        s.shown.clone_from(&preedit);
        s.shown_caret = caret;
        Some((s.client, s.sink.clone(), preedit, caret, unchanged))
    })
    .flatten() else {
        return;
    };
    if let Some(text) = &commit {
        remember_commit(state, context, text);
    }
    if !unchanged {
        let shared = Rc::clone(state);
        let document = context.clone();
        edit::run(context, client, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE, move |cookie| {
            write(&shared, &document, cookie, sink.as_ref(), commit.as_deref(), &preedit, caret)
        });
        if let Some(timing) = timing.as_deref_mut() {
            timing.step("write");
        }
    }
    refresh_candidates(state);
    if let Some(timing) = timing {
        timing.step("popup");
    }
}

/// A short notice near the caret.
fn tip(state: &Shared, text: &str) {
    ensure_popup(state);
    with(state, |s| {
        if let Some(popup) = &s.popup {
            popup.show_tip(text, s.anchor.unwrap_or_else(fallback_anchor));
        }
    });
}

/// Choose the casual or the formal style from the menu.
pub(crate) fn choose_style(state: &Shared, formal: bool) {
    let style = if formal { FORMAL } else { CASUAL };
    with_controller(state, |c| c.want_style(style));
    settings::save_style(style);
}

/// Open the settings window, a separate program.
pub(crate) fn open_settings() {
    if let Some(exe) = settings_path().filter(|exe| exe.exists() && crate::pipe::may_launch()) {
        crate::pipe::spawn(&exe);
    }
}

/// The request names joined for the timing log, e.g. "state+tone". The few
/// combinations that occur are kept for the life of the process.
fn leak_join(names: &[&'static str]) -> &'static str {
    if let [one] = names {
        return one;
    }
    thread_local! {
        static JOINED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }
    let joined = names.join("+");
    JOINED.with(|known| {
        let mut known = known.borrow_mut();
        if let Some(found) = known.iter().find(|k| **k == joined) {
            return *found;
        }
        let leaked: &'static str = Box::leak(joined.into_boxed_str());
        known.push(leaked);
        leaked
    })
}

fn with<R>(state: &Shared, f: impl FnOnce(&mut State) -> R) -> Option<R> {
    state.try_borrow_mut().ok().map(|mut state| f(&mut state))
}

fn with_controller<R>(
    state: &Shared,
    f: impl FnOnce(&mut Controller<PipeEngine>) -> R,
) -> Option<R> {
    with(state, |state| state.controller.as_mut().map(f)).flatten()
}

#[implement(
    ITfTextInputProcessorEx,
    ITfTextInputProcessor,
    ITfKeyEventSink,
    ITfCompositionSink,
    ITfDisplayAttributeProvider,
    ITfThreadFocusSink
)]
pub struct TextService {
    state: Shared,
}

impl TextService {
    pub fn new() -> Self {
        TextService { state: Rc::default() }
    }
}

fn key_from(wparam: WPARAM) -> Key {
    let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    Key {
        vk: wparam.0 as u32,
        shift: down(VK_SHIFT.0),
        ctrl: down(VK_CONTROL.0),
        alt: down(VK_MENU.0),
        caps: unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0,
    }
}

/// The window class of the focused window, upper case; empty if none.
fn focused_class() -> String {
    unsafe {
        let window = GetFocus();
        if window.is_invalid() {
            return String::new();
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(window, &mut class) as usize;
        String::from_utf16_lossy(&class[..len]).to_ascii_uppercase()
    }
}

/// Is the focused window a classic edit control in password mode?
fn password_window() -> bool {
    let class = focused_class();
    (class == "EDIT" || class.contains("RICHEDIT"))
        && unsafe { GetWindowLongW(GetFocus(), GWL_STYLE) } & ES_PASSWORD != 0
}

/// The phrase key as TSF takes it.
fn preserved_key(key: Hotkey) -> TF_PRESERVEDKEY {
    let mut modifiers = 0;
    if key.alt {
        modifiers |= TF_MOD_ALT;
    }
    if key.ctrl {
        modifiers |= TF_MOD_CONTROL;
    }
    if key.shift {
        modifiers |= TF_MOD_SHIFT;
    }
    TF_PRESERVEDKEY { uVKey: key.vk, uModifiers: modifiers }
}

/// Register the phrase key the settings name (or none), replacing the one
/// registered before. Checked when the application gets the focus.
fn sync_hotkey(state: &Shared) {
    let wanted = settings::options().phrase_key;
    let Some((thread_mgr, client, current)) = with(state, |s| (s.thread_mgr.clone(), s.client, s.preserved))
    else {
        return;
    };
    if wanted == current {
        return;
    }
    let Some(keystrokes) = thread_mgr.and_then(|t| t.cast::<ITfKeystrokeMgr>().ok()) else {
        return;
    };
    let mut registered = None;
    unsafe {
        if let Some(old) = current {
            let _ = keystrokes.UnpreserveKey(&GUID_PHRASE_KEY, &preserved_key(old));
        }
        if let Some(key) = wanted {
            let description: Vec<u16> = "片語選單".encode_utf16().collect();
            if keystrokes.PreserveKey(client, &GUID_PHRASE_KEY, &preserved_key(key), &description).is_ok() {
                registered = Some(key);
            }
        }
    }
    with(state, |s| s.preserved = registered);
}

/// A held key arrives again and again; this long between two arrivals
/// makes them separate presses.
const PHRASE_KEY_REPEAT: Duration = Duration::from_millis(150);

/// The phrase key: open the menu, or close it if it is open. True if the
/// key is ours (it is not in a password field).
fn phrase_key(state: &Shared, context: &ITfContext) -> bool {
    let now = Instant::now();
    let Some(last) = with(state, |s| s.phrase_key_at.replace(now)) else {
        return false;
    };
    if last.is_some_and(|at| now.duration_since(at) < PHRASE_KEY_REPEAT) {
        return true;
    }
    if with_controller(state, |c| c.close_menu()).unwrap_or(false) {
        refresh_candidates(state);
        return true;
    }
    let enabled = with_controller(state, |c| c.enabled()).unwrap_or(false);
    if !enabled {
        return false;
    }
    let phrases = settings::load_phrases();
    if phrases.is_empty() {
        tip(state, "還沒有片語：在設定的「快速輸入」新增");
        return true;
    }
    let Some(client) = with(state, |s| s.client) else {
        return false;
    };
    let before = edit::caret_rect(context, client);
    with(state, |s| s.menu_anchor = before);
    let Some(outcome) = with_controller(state, |c| c.open_menu(phrases)) else {
        return false;
    };
    if outcome.commit.is_some() {
        // The composition went into the document; the caret moved with it.
        refresh(state, context, outcome.commit, None);
        let after = edit::caret_rect(context, client);
        with(state, |s| s.menu_anchor = after.or(before));
    }
    refresh_candidates(state);
    outcome.eaten
}

/// Somewhere near the caret, for when the application cannot say where the
/// composition is: the system caret if there is one, else the mouse pointer.
fn fallback_anchor() -> RECT {
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..GUITHREADINFO::default()
        };
        if GetGUIThreadInfo(0, &mut info).is_ok() && !info.hwndCaret.is_invalid() {
            let mut top = POINT { x: info.rcCaret.left, y: info.rcCaret.top };
            let mut bottom = POINT { x: info.rcCaret.right, y: info.rcCaret.bottom };
            if ClientToScreen(info.hwndCaret, &mut top).as_bool()
                && ClientToScreen(info.hwndCaret, &mut bottom).as_bool()
            {
                return RECT { left: top.x, top: top.y, right: bottom.x, bottom: bottom.y };
            }
        }
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        RECT { left: point.x, top: point.y, right: point.x, bottom: point.y + 16 }
    }
}

/// Bring the document in line with the controller. Runs in an edit session.
#[allow(clippy::too_many_arguments)]
fn write(
    state: &Shared,
    context: &ITfContext,
    cookie: u32,
    sink: Option<&ITfCompositionSink>,
    commit: Option<&str>,
    preedit: &str,
    caret: usize,
) -> Result<()> {
    let (mut composition, attribute) =
        with(state, |s| (s.composition.clone(), s.attribute)).ok_or(E_FAIL)?;
    let attribute = Some(attribute).filter(|&atom| atom != 0);

    unsafe {
        if let Some(text) = commit {
            match composition.take() {
                // The committed text replaces the composition, which then ends.
                Some(ending) => {
                    let range = ending.GetRange()?;
                    range.SetText(cookie, 0, &wide(text))?;
                    edit::set_attribute(context, cookie, &range, None);
                    edit::caret_to_end(context, cookie, &range)?;
                    // Forget it first: ending it may call OnCompositionTerminated.
                    with(state, |s| s.composition = None);
                    ending.EndComposition(cookie)?;
                }
                None => edit::insert(context, cookie, &wide(text))?,
            }
        }

        if preedit.is_empty() {
            if let Some(ending) = composition {
                let range = ending.GetRange()?;
                range.SetText(cookie, 0, &[])?;
                with(state, |s| s.composition = None);
                ending.EndComposition(cookie)?;
            }
            with(state, |s| s.anchor = None);
            return Ok(());
        }

        let composition = match composition {
            Some(composition) => composition,
            None => {
                let start = edit::insertion_point(context, cookie)?;
                let owner: ITfContextComposition = context.cast()?;
                let started = owner.StartComposition(cookie, &start, sink.ok_or(E_FAIL)?)?;
                with(state, |s| s.composition = Some(started.clone()));
                started
            }
        };
        let range = composition.GetRange()?;
        range.SetText(cookie, 0, &wide(preedit))?;
        if attribute.is_some() {
            edit::set_attribute(context, cookie, &range, attribute);
        }
        edit::caret_at(context, cookie, &range, caret as i32)?;
        // The candidates belong to the character at the caret: put the
        // window under it, or under the whole composition if the
        // application cannot say where that character is.
        let rect = edit::screen_rect_at(context, cookie, &range, caret as i32)
            .or_else(|| edit::screen_rect(context, cookie, &range));
        if let Some(rect) = rect {
            with(state, |s| s.anchor = Some(rect));
        }
    }
    Ok(())
}

impl TextService_Impl {
    /// Turn the input method off in password fields and back on elsewhere.
    /// Says why it is off, for the timing log.
    fn refresh_enabled(&self, context: &ITfContext) -> Option<&'static str> {
        let (client, last) = with(&self.state, |s| {
            let idle = s.controller.as_ref().is_some_and(|c| !c.composing());
            idle.then_some((s.client, s.password))
        })
        .flatten()?;
        // Only the input scope can be momentarily unreadable, so only its
        // answer is remembered.
        let scope = edit::password_scope(context, client).unwrap_or(last);
        let off = if password_window() {
            Some("password-window")
        } else if let Some(switch) = edit::keyboard_disabled(context) {
            Some(switch)
        } else {
            scope.then_some("password-scope")
        };
        with(&self.state, |s| {
            s.password = scope;
            if let Some(controller) = s.controller.as_mut() {
                controller.set_enabled(off.is_none());
            }
        });
        off
    }

    fn key_down(&self, context: &ITfContext, wparam: WPARAM) -> bool {
        let mut timing = timing::Key::start("down");
        let eaten = self.key_down_timed(context, wparam, &mut timing);
        timing.finish();
        eaten
    }

    fn key_down_timed(&self, context: &ITfContext, wparam: WPARAM, timing: &mut timing::Key) -> bool {
        let key = key_from(wparam);
        self.refresh_enabled(context);
        timing.step("password");
        let Some((starts, client)) = with(&self.state, |s| {
            Some((s.controller.as_ref()?.starts_composition(key), s.client))
        })
        .flatten() else {
            return false;
        };
        if starts {
            // Not readable in every application; the engine copes without.
            // Not readable in every application: then what we committed
            // in this document lately, and nothing in a new one.
            let text = edit::text_before_caret(context, client)
                .unwrap_or_else(|| recent_text(&self.state, context));
            with_controller(&self.state, |c| c.set_context(&text));
            timing.step("context");
        }
        let Some((outcome, formal, sent)) = with_controller(&self.state, |c| {
            c.take_sent(); // the context request is timed above
            let outcome = c.key_down(key);
            (outcome, c.style() == FORMAL, c.take_sent())
        }) else {
            return false;
        };
        if sent.is_empty() {
            timing.step("local");
        } else {
            timing.request(Some(leak_join(&sent)));
            timing.step("engine");
        }
        refresh(&self.state, context, outcome.commit, Some(timing));
        retry_later(&self.state);
        if let Some(notice) = outcome.notice {
            tip(&self.state, &notice);
        }
        if outcome.width_changed {
            let full = with_controller(&self.state, |c| c.full_width()).unwrap_or(false);
            tip(&self.state, if full { "全形" } else { "半形" });
            button_changed(&self.state);
        }
        if outcome.style_changed {
            // Remembered for the other applications and for the next session.
            settings::save_style(if formal { FORMAL } else { CASUAL });
            tip(&self.state, if formal { "正式" } else { "口語" });
        }
        outcome.eaten
    }

    /// Shift on its own switches between Chinese and English. True for the
    /// one release that is eaten: Alt after the phrase key. TSF first asks
    /// (`test`), then delivers; the answer must be the same both times.
    fn key_up(&self, context: &ITfContext, wparam: WPARAM, test: bool) -> bool {
        let key = key_from(wparam);
        if matches!(key.vk, vk::MENU | 0xA4 | vk::RMENU) {
            let eat = with(&self.state, |s| {
                if test { s.eat_alt_up } else { std::mem::take(&mut s.eat_alt_up) }
            });
            if eat.unwrap_or(false) {
                return true;
            }
        }
        let Some((outcome, toggled, chinese)) = with_controller(&self.state, |c| {
            let before = c.chinese();
            let outcome = c.key_up(key);
            (outcome, before != c.chinese(), c.chinese())
        }) else {
            return false;
        };
        if !toggled {
            return false;
        }
        mode_switched(chinese);
        refresh(&self.state, context, outcome.commit, None);
        tip(&self.state, if chinese { "中" } else { "英" });
        button_changed(&self.state);
        false
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, thread_mgr: Ref<ITfThreadMgr>, client: u32) -> Result<()> {
        self.ActivateEx(thread_mgr, client, 0)
    }

    fn Deactivate(&self) -> Result<()> {
        guard(Ok(()), || {
            // The candidate list must not outlive the text service.
            with_controller(&self.state, |c| c.close_candidates());
            refresh_candidates(&self.state);
            let (thread_mgr, client, button, focus_cookie, preserved) = with(&self.state, |s| {
                (s.thread_mgr.take(), s.client, s.button.take(), s.focus_cookie.take(), s.preserved.take())
            })
            .unwrap_or_default();
            if let Some(thread_mgr) = thread_mgr {
                unsafe {
                    if let (Some(key), Ok(keystrokes)) = (preserved, thread_mgr.cast::<ITfKeystrokeMgr>()) {
                        let _ = keystrokes.UnpreserveKey(&GUID_PHRASE_KEY, &preserved_key(key));
                    }
                    if let (Some(cookie), Ok(source)) = (focus_cookie, thread_mgr.cast::<ITfSource>()) {
                        let _ = source.UnadviseSink(cookie);
                    }
                    if let Ok(keystrokes) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                        let _ = keystrokes.UnadviseKeyEventSink(client);
                    }
                    if let (Some(button), Ok(bar)) = (button, thread_mgr.cast::<ITfLangBarItemMgr>())
                    {
                        let _ = bar.RemoveItem(&button);
                    }
                }
            }
            with(&self.state, |s| {
                // Let the engine learn from what was composed, then drop everything.
                if let Some(controller) = s.controller.as_mut() {
                    controller.finish();
                }
                *s = State::default();
            });
            Ok(())
        })
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, thread_mgr: Ref<ITfThreadMgr>, client: u32, _flags: u32) -> Result<()> {
        guard(Err(E_FAIL.into()), || unsafe {
            let thread_mgr = thread_mgr.ok()?.clone();
            let keystrokes: ITfKeystrokeMgr = thread_mgr.cast()?;
            let sink: ITfKeyEventSink = self.to_interface();
            keystrokes.AdviseKeyEventSink(client, &sink, true)?;

            let attribute =
                CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)
                    .and_then(|categories| categories.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE))
                    .unwrap_or(0);
            // Connect now (starting the host if need be) so that the engine
            // is ready by the time the first key arrives.
            let controller = PipeEngine::new(host_path()).map(|mut engine| {
                engine.connect();
                let mut controller = Controller::new(engine);
                if let Some(style) = settings::load_style() {
                    controller.want_style(&style);
                }
                let options = settings::options();
                controller.set_options(options);
                // Nothing is composed yet, so nothing is committed.
                controller.set_chinese(options.start_chinese);
                controller
            });
            // The state must be complete before the mode button is added:
            // adding it makes TSF ask for its icon and hand it the sink
            // through which every later change is announced, and both land
            // in this state. (Overwriting the state afterwards lost the sink,
            // so the taskbar never heard that the mode changed.)
            with(&self.state, |s| {
                *s = State {
                    thread_mgr: Some(thread_mgr.clone()),
                    client,
                    controller,
                    attribute,
                    sink: Some(self.to_interface()),
                    ..State::default()
                };
            });
            // The mode button. The input method works without it.
            let button: ITfLangBarItemButton = ModeButton::new(Rc::clone(&self.state)).into();
            if let Ok(bar) = thread_mgr.cast::<ITfLangBarItemMgr>() {
                if bar.AddItem(&button).is_ok() {
                    with(&self.state, |s| s.button = Some(button));
                }
            }
            // Told whenever this application's thread gets the focus, which is
            // when a mode chosen in another application must be taken on.
            if let Ok(source) = thread_mgr.cast::<ITfSource>() {
                let sink: ITfThreadFocusSink = self.to_interface();
                if let Ok(cookie) = source.AdviseSink(&ITfThreadFocusSink::IID, &sink) {
                    with(&self.state, |s| s.focus_cookie = Some(cookie));
                }
            }
            adopt_shared_mode(&self.state);
            sync_hotkey(&self.state);
            button_changed(&self.state);
            Ok(())
        })
    }
}

impl ITfThreadFocusSink_Impl for TextService_Impl {
    /// The user switched to this application.
    fn OnSetThreadFocus(&self) -> Result<()> {
        guard(Ok(()), || {
            focus_gained(&self.state);
            Ok(())
        })
    }

    fn OnKillThreadFocus(&self) -> Result<()> {
        guard(Ok(()), || {
            with(&self.state, |s| {
                if let Some(popup) = &s.popup {
                    popup.hide();
                }
            });
            Ok(())
        })
    }
}

/// This application has the focus again: settings chosen elsewhere in the
/// meantime apply here now, and the taskbar shows this application's mode.
fn focus_gained(state: &Shared) {
    if let Some(style) = settings::load_style() {
        with_controller(state, |c| c.want_style(&style));
    }
    adopt_shared_mode(state);
    sync_hotkey(state);
    button_changed(state);
    button_changed_later(state);
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        if foreground.as_bool() {
            focus_gained(&self.state);
        } else {
            with(&self.state, |s| {
                if let Some(popup) = &s.popup {
                    popup.hide();
                }
            });
        }
        Ok(())
    }

    fn OnTestKeyDown(&self, context: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            let Some(context) = context.as_ref().filter(|_| !crate::broken()) else {
                return Ok(false.into());
            };
            let mut timing = timing::Key::start("test");
            adopt_shared_mode_if_idle(&self.state);
            let key = key_from(wparam);
            let off = self.refresh_enabled(context);
            timing.step("password");
            if let Some(reason) = off {
                timing.step(reason);
            }
            let options = settings::options();
            let tested = with_controller(&self.state, |c| {
                c.set_options(options);
                c.note_key_down(key);
                let eaten = c.test_key(key);
                // A key that goes to the application closes the phrase menu.
                (eaten, !eaten && c.dismiss_menu(key))
            });
            let (eaten, dismissed) = tested.unwrap_or_default();
            if dismissed {
                refresh_candidates(&self.state);
            }
            // Keys the input method leaves alone are not worth a line, unless
            // it left them alone because it is off: the log says why.
            if eaten || off.is_some() {
                timing.finish();
            }
            Ok(eaten.into())
        })
    }

    fn OnKeyDown(&self, context: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            let Some(context) = context.as_ref().filter(|_| !crate::broken()) else {
                return Ok(false.into());
            };
            Ok(self.key_down(context, wparam).into())
        })
    }

    // Key releases are never eaten; the only one that matters is Shift,
    // and acting on it here keeps the application's view of Shift intact.
    fn OnTestKeyUp(&self, context: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            let eaten = match context.as_ref().filter(|_| !crate::broken()) {
                Some(context) => self.key_up(context, wparam, true),
                None => false,
            };
            Ok(eaten.into())
        })
    }

    fn OnKeyUp(&self, context: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            let eaten = match context.as_ref().filter(|_| !crate::broken()) {
                Some(context) => self.key_up(context, wparam, false),
                None => false,
            };
            Ok(eaten.into())
        })
    }

    fn OnPreservedKey(&self, context: Ref<ITfContext>, guid: *const GUID) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            let Some(context) = context.as_ref().filter(|_| !crate::broken()) else {
                return Ok(false.into());
            };
            if guid.is_null() || unsafe { *guid } != GUID_PHRASE_KEY {
                return Ok(false.into());
            }
            self.refresh_enabled(context);
            let eaten = phrase_key(&self.state, context);
            if eaten {
                let alt = with(&self.state, |s| s.preserved.is_some_and(|key| key.alt)).unwrap_or(false);
                with(&self.state, |s| s.eat_alt_up = alt);
            }
            Ok(eaten.into())
        })
    }
}

impl ITfCompositionSink_Impl for TextService_Impl {
    /// The application ended our composition (a click elsewhere, say).
    /// Whatever is in the document stays there, so make it the text the
    /// engine commits: without the unfinished bopomofo symbols.
    fn OnCompositionTerminated(&self, cookie: u32, composition: Ref<ITfComposition>) -> Result<()> {
        guard(Ok(()), || {
            // Not ours any more: we ended it ourselves.
            if with(&self.state, |s| s.composition.take()).flatten().is_none() {
                return Ok(());
            }
            let Some((shown, text)) = with(&self.state, |s| {
                let controller = s.controller.as_mut()?;
                let shown = std::mem::take(&mut s.shown);
                s.shown_caret = 0;
                s.anchor = None;
                if let Some(popup) = &s.popup {
                    popup.hide();
                }
                Some((shown, controller.finish().unwrap_or_default()))
            })
            .flatten() else {
                return Ok(());
            };
            // The window closed with the composition.
            refresh_candidates(&self.state);
            if let Some(range) = composition.as_ref().and_then(|c| unsafe { c.GetRange() }.ok()) {
                unsafe {
                    if text != shown {
                        let _ = range.SetText(cookie, 0, &wide(&text));
                    }
                    if let Ok(context) = range.GetContext() {
                        edit::set_attribute(&context, cookie, &range, None);
                    }
                }
            }
            Ok(())
        })
    }
}

impl ITfDisplayAttributeProvider_Impl for TextService_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(AttributeEnumerator::create())
    }

    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        if guid.is_null() || unsafe { *guid } != GUID_DISPLAY_ATTRIBUTE {
            return Err(E_INVALIDARG.into());
        }
        Ok(CompositionAttribute::create())
    }
}
