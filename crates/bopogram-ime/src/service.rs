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

use windows::core::{implement, IUnknownImpl, Interface, Ref, Result, BOOL, GUID};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, LPARAM, POINT, RECT, WPARAM};
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
    ITfLangBarItemButton, ITfLangBarItemMgr, ITfLangBarItemSink, ITfThreadMgr,
    TF_ES_ASYNCDONTCARE, TF_ES_READWRITE, TF_LBI_ICON, TF_LBI_TEXT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetGUIThreadInfo, GetWindowLongW, GUITHREADINFO, GWL_STYLE,
};

use crate::controller::{Controller, Key, CASUAL, FORMAL};
use crate::display::{AttributeEnumerator, CompositionAttribute};
use crate::dll::{host_path, settings_path, GUID_DISPLAY_ATTRIBUTE};
use crate::langbar::ModeButton;
use crate::pipe::PipeEngine;
use crate::popup::Popup;
use crate::{edit, guard, settings, wide};

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
    button_sink: Option<ITfLangBarItemSink>,
}

pub(crate) type Shared = Rc<RefCell<State>>;

// -- what the mode button needs ----------------------------------------------

pub(crate) fn is_chinese(state: &Shared) -> bool {
    with_controller(state, |c| c.chinese()).unwrap_or(true)
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
    let Some((commit, thread_mgr, client)) = with(state, |s| {
        let commit = s.controller.as_mut()?.toggle_chinese();
        s.shown.clear();
        s.shown_caret = 0;
        if let Some(popup) = &s.popup {
            popup.hide();
        }
        Some((commit, s.thread_mgr.clone()?, s.client))
    })
    .flatten() else {
        return;
    };
    if let Some(text) = commit {
        let context = unsafe { thread_mgr.GetFocus().and_then(|document| document.GetTop()) };
        if let Ok(context) = context {
            let state = Rc::clone(state);
            let document = context.clone();
            edit::run(&context, client, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE, move |cookie| {
                write(&state, &document, cookie, None, Some(&text), "", 0)
            });
        }
    }
    button_changed(state);
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
    ITfDisplayAttributeProvider
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

/// Is the focused window a classic edit control in password mode?
fn password_window() -> bool {
    unsafe {
        let window = GetFocus();
        if window.is_invalid() {
            return false;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(window, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..len]).to_ascii_uppercase();
        (class == "EDIT" || class.contains("RICHEDIT"))
            && GetWindowLongW(window, GWL_STYLE) & ES_PASSWORD != 0
    }
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
        if let Some(rect) = edit::screen_rect(context, cookie, &range) {
            with(state, |s| s.anchor = Some(rect));
        }
    }
    Ok(())
}

impl TextService_Impl {
    /// Turn the input method off in password fields and back on elsewhere.
    fn refresh_enabled(&self, context: &ITfContext) {
        let Some((client, last)) = with(&self.state, |s| {
            let idle = s.controller.as_ref().is_some_and(|c| !c.composing());
            idle.then_some((s.client, s.password))
        })
        .flatten() else {
            return;
        };
        // Only the input scope can be momentarily unreadable, so only its
        // answer is remembered.
        let scope = edit::password_scope(context, client).unwrap_or(last);
        let password = password_window() || edit::keyboard_disabled(context) || scope;
        with(&self.state, |s| {
            s.password = scope;
            if let Some(controller) = s.controller.as_mut() {
                controller.set_enabled(!password);
            }
        });
    }

    /// Write the controller's composition (and any committed text) to the
    /// document, then update the candidate window.
    fn update(&self, context: &ITfContext, commit: Option<String>) {
        let Some((client, preedit, caret, unchanged)) = with(&self.state, |s| {
            let controller = s.controller.as_ref()?;
            let preedit = controller.preedit().to_owned();
            let caret = controller.caret_units();
            let unchanged = commit.is_none() && preedit == s.shown && caret == s.shown_caret;
            s.shown.clone_from(&preedit);
            s.shown_caret = caret;
            Some((s.client, preedit, caret, unchanged))
        })
        .flatten() else {
            return;
        };
        if !unchanged {
            let sink: ITfCompositionSink = self.to_interface();
            let state = Rc::clone(&self.state);
            let document = context.clone();
            edit::run(context, client, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE, move |cookie| {
                write(&state, &document, cookie, Some(&sink), commit.as_deref(), &preedit, caret)
            });
        }
        self.refresh_popup();
    }

    fn refresh_popup(&self) {
        with(&self.state, |s| {
            let Some(controller) = s.controller.as_ref() else { return };
            let Some((items, selected, page, pages)) = controller.candidate_page() else {
                if let Some(popup) = &s.popup {
                    popup.hide();
                }
                return;
            };
            let lines: Vec<String> =
                items.iter().enumerate().map(|(i, word)| format!("{} {word}", i + 1)).collect();
            let footer = (pages > 1).then(|| format!("{}/{pages}", page + 1));
            if s.popup.is_none() {
                s.popup = Popup::new();
            }
            if let Some(popup) = &s.popup {
                popup.show(&lines, Some(selected), footer, s.anchor.unwrap_or_else(fallback_anchor));
            }
        });
    }

    fn key_down(&self, context: &ITfContext, wparam: WPARAM) -> bool {
        let key = key_from(wparam);
        self.refresh_enabled(context);
        let Some((starts, client)) = with(&self.state, |s| {
            Some((s.controller.as_ref()?.starts_composition(key), s.client))
        })
        .flatten() else {
            return false;
        };
        if starts {
            // Not readable in every application; the engine copes without.
            if let Some(text) = edit::text_before_caret(context, client) {
                with_controller(&self.state, |c| c.set_context(&text));
            }
        }
        let Some((outcome, formal)) =
            with_controller(&self.state, |c| (c.key_down(key), c.style() == FORMAL))
        else {
            return false;
        };
        self.update(context, outcome.commit);
        if outcome.style_changed {
            // Remembered for the other applications and for the next session.
            settings::save_style(if formal { FORMAL } else { CASUAL });
            self.tip(if formal { "正式" } else { "口語" });
        }
        outcome.eaten
    }

    /// A short notice near the caret.
    fn tip(&self, text: &str) {
        with(&self.state, |s| {
            if s.popup.is_none() {
                s.popup = Popup::new();
            }
            if let Some(popup) = &s.popup {
                popup.show_tip(text, s.anchor.unwrap_or_else(fallback_anchor));
            }
        });
    }

    /// Shift on its own switches between Chinese and English.
    fn key_up(&self, context: &ITfContext, wparam: WPARAM) {
        let key = key_from(wparam);
        let Some((outcome, toggled, chinese)) = with_controller(&self.state, |c| {
            let before = c.chinese();
            let outcome = c.key_up(key);
            (outcome, before != c.chinese(), c.chinese())
        }) else {
            return;
        };
        if !toggled {
            return;
        }
        self.update(context, outcome.commit);
        self.tip(if chinese { "中" } else { "英" });
        button_changed(&self.state);
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, thread_mgr: Ref<ITfThreadMgr>, client: u32) -> Result<()> {
        self.ActivateEx(thread_mgr, client, 0)
    }

    fn Deactivate(&self) -> Result<()> {
        guard(Ok(()), || {
            let (thread_mgr, client, button) =
                with(&self.state, |s| (s.thread_mgr.take(), s.client, s.button.take()))
                    .unwrap_or_default();
            if let Some(thread_mgr) = thread_mgr {
                unsafe {
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
                controller
            });
            // The mode button. The input method works without it.
            let button: ITfLangBarItemButton = ModeButton::new(Rc::clone(&self.state)).into();
            let button = thread_mgr
                .cast::<ITfLangBarItemMgr>()
                .and_then(|bar| bar.AddItem(&button))
                .is_ok()
                .then_some(button);
            with(&self.state, |s| {
                *s = State {
                    thread_mgr: Some(thread_mgr),
                    client,
                    controller,
                    attribute,
                    button,
                    ..State::default()
                };
            });
            Ok(())
        })
    }
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        if foreground.as_bool() {
            // The style may have been changed in another application.
            if let Some(style) = settings::load_style() {
                with_controller(&self.state, |c| c.want_style(&style));
            }
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
            let Some(context) = context.as_ref() else {
                return Ok(false.into());
            };
            let key = key_from(wparam);
            self.refresh_enabled(context);
            let eaten = with_controller(&self.state, |c| {
                c.note_key_down(key);
                c.test_key(key)
            });
            Ok(eaten.unwrap_or(false).into())
        })
    }

    fn OnKeyDown(&self, context: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            let Some(context) = context.as_ref() else {
                return Ok(false.into());
            };
            Ok(self.key_down(context, wparam).into())
        })
    }

    // Key releases are never eaten; the only one that matters is Shift,
    // and acting on it here keeps the application's view of Shift intact.
    fn OnTestKeyUp(&self, context: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            if let Some(context) = context.as_ref() {
                self.key_up(context, wparam);
            }
            Ok(false.into())
        })
    }

    fn OnKeyUp(&self, context: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            if let Some(context) = context.as_ref() {
                self.key_up(context, wparam);
            }
            Ok(false.into())
        })
    }

    fn OnPreservedKey(&self, _: Ref<ITfContext>, _: *const GUID) -> Result<BOOL> {
        Ok(false.into())
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
