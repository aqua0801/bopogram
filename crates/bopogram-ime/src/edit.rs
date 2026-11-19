//! Edit sessions, and the operations on the document that need one.
//!
//! TSF only lets a text service read or change a document inside an edit
//! session: the service asks for one and is called back with a cookie.

use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::rc::Rc;

use windows::core::{implement, IUnknown, Interface, Result};
use windows::Win32::Foundation::{E_FAIL, RECT};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::TextServices::{
    ITfCompartmentMgr, ITfContext, ITfEditSession, ITfEditSession_Impl, ITfInputScope,
    ITfInsertAtSelection, ITfRange, InputScope, GUID_COMPARTMENT_EMPTYCONTEXT,
    GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_PROP_ATTRIBUTE, GUID_PROP_INPUTSCOPE,
    INSERT_TEXT_AT_SELECTION_FLAGS, IS_PASSWORD, TF_ANCHOR_END, TF_ANCHOR_START,
    TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_DEFAULT_SELECTION, TF_ES_READ, TF_ES_SYNC,
    TF_IAS_QUERYONLY, TF_SELECTION,
};

use crate::guard;

/// Characters of left context reported to the engine.
const CONTEXT_CHARS: i32 = 40;

#[implement(ITfEditSession)]
struct EditSession(Box<dyn Fn(u32) -> Result<()>>);

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        guard(Err(E_FAIL.into()), || (self.0)(cookie))
    }
}

/// Ask for an edit session and run `f` in it. True if the session was
/// granted and `f` succeeded (or, for an asynchronous request, was queued).
pub fn run(
    context: &ITfContext,
    client: u32,
    flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    f: impl Fn(u32) -> Result<()> + 'static,
) -> bool {
    let session: ITfEditSession = EditSession(Box::new(f)).into();
    matches!(unsafe { context.RequestEditSession(client, &session, flags) }, Ok(hr) if hr.is_ok())
}

/// The range of the current selection (the caret, when nothing is selected).
pub fn selection(context: &ITfContext, cookie: u32) -> Result<ITfRange> {
    let mut selections = [TF_SELECTION::default()];
    let mut fetched = 0;
    unsafe {
        context.GetSelection(cookie, TF_DEFAULT_SELECTION, &mut selections, &mut fetched)?;
        ManuallyDrop::take(&mut selections[0].range).ok_or_else(|| E_FAIL.into())
    }
}

/// Put the caret at the end of `range`.
pub fn caret_to_end(context: &ITfContext, cookie: u32, range: &ITfRange) -> Result<()> {
    unsafe {
        let caret = range.Clone()?;
        caret.Collapse(cookie, TF_ANCHOR_END)?;
        let mut selection = TF_SELECTION {
            range: ManuallyDrop::new(Some(caret)),
            ..TF_SELECTION::default()
        };
        let set = context.SetSelection(cookie, std::slice::from_ref(&selection));
        ManuallyDrop::drop(&mut selection.range);
        set
    }
}

/// Put the caret `units` UTF-16 units into `range`.
pub fn caret_at(context: &ITfContext, cookie: u32, range: &ITfRange, units: i32) -> Result<()> {
    unsafe {
        let caret = range.Clone()?;
        caret.Collapse(cookie, TF_ANCHOR_START)?;
        let mut moved = 0;
        caret.ShiftEnd(cookie, units, &mut moved, std::ptr::null())?;
        caret_to_end(context, cookie, &caret)
    }
}

/// Insert `text` at the caret and leave the caret after it.
pub fn insert(context: &ITfContext, cookie: u32, text: &[u16]) -> Result<()> {
    unsafe {
        let inserter: ITfInsertAtSelection = context.cast()?;
        let range =
            inserter.InsertTextAtSelection(cookie, INSERT_TEXT_AT_SELECTION_FLAGS(0), text)?;
        caret_to_end(context, cookie, &range)
    }
}

/// An empty range at the caret, where a new composition can start.
pub fn insertion_point(context: &ITfContext, cookie: u32) -> Result<ITfRange> {
    unsafe {
        let inserter: ITfInsertAtSelection = context.cast()?;
        inserter.InsertTextAtSelection(cookie, TF_IAS_QUERYONLY, &[])
    }
}

/// Mark `range` with the composition's display attribute, or clear the mark.
pub fn set_attribute(context: &ITfContext, cookie: u32, range: &ITfRange, atom: Option<u32>) {
    unsafe {
        let Ok(property) = context.GetProperty(&GUID_PROP_ATTRIBUTE) else {
            return;
        };
        let _ = match atom {
            Some(atom) => property.SetValue(cookie, range, &VARIANT::from(atom as i32)),
            None => property.Clear(cookie, range),
        };
    }
}

/// Where `range` is on the screen, if the application can tell.
pub fn screen_rect(context: &ITfContext, cookie: u32, range: &ITfRange) -> Option<RECT> {
    unsafe {
        let view = context.GetActiveView().ok()?;
        let mut rect = RECT::default();
        let mut clipped = Default::default();
        view.GetTextExt(cookie, range, &mut rect, &mut clipped).ok()?;
        (rect.bottom > rect.top).then_some(rect)
    }
}

/// Where the character at `units` into `range` is on the screen (the one
/// before it when `units` is at the end).
pub fn screen_rect_at(context: &ITfContext, cookie: u32, range: &ITfRange, units: i32) -> Option<RECT> {
    unsafe {
        let character = range.Clone().ok()?;
        character.Collapse(cookie, TF_ANCHOR_START).ok()?;
        let mut moved = 0;
        character.ShiftStart(cookie, units, &mut moved, std::ptr::null()).ok()?;
        character.Collapse(cookie, TF_ANCHOR_START).ok()?;
        character.ShiftEnd(cookie, 1, &mut moved, std::ptr::null()).ok()?;
        if moved == 0 {
            character.ShiftStart(cookie, -1, &mut moved, std::ptr::null()).ok()?;
        }
        screen_rect(context, cookie, &character)
    }
}

/// The text just before the caret. `None` if the application does not let
/// us read it; the engine then falls back on what it committed itself.
pub fn text_before_caret(context: &ITfContext, client: u32) -> Option<String> {
    let found = Rc::new(RefCell::new(None));
    let out = Rc::clone(&found);
    let document = context.clone();
    run(context, client, TF_ES_SYNC | TF_ES_READ, move |cookie| unsafe {
        let range = selection(&document, cookie)?;
        range.Collapse(cookie, TF_ANCHOR_START)?;
        let mut moved = 0;
        range.ShiftStart(cookie, -CONTEXT_CHARS, &mut moved, std::ptr::null())?;
        let mut buf = [0u16; CONTEXT_CHARS as usize];
        let mut len = 0;
        range.GetText(cookie, 0, &mut buf, &mut len)?;
        *out.borrow_mut() = Some(String::from_utf16_lossy(&buf[..len as usize]));
        Ok(())
    });
    found.take()
}

/// Has the application switched keyboard input methods off for this context?
/// Applications do this for fields that must not be composed into; browsers
/// do it for password fields. Needs no edit session.
pub fn keyboard_disabled(context: &ITfContext) -> bool {
    let Ok(compartments) = context.cast::<ITfCompartmentMgr>() else {
        return false;
    };
    [GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_COMPARTMENT_EMPTYCONTEXT].iter().any(|guid| unsafe {
        compartments
            .GetCompartment(guid)
            .and_then(|compartment| compartment.GetValue())
            .ok()
            .and_then(|value| i32::try_from(&value).ok())
            .is_some_and(|value| value != 0)
    })
}

/// Does the application declare the field at the caret as a password field?
/// `None` if that cannot be determined.
pub fn password_scope(context: &ITfContext, client: u32) -> Option<bool> {
    let found = Rc::new(Cell::new(None));
    let out = Rc::clone(&found);
    let document = context.clone();
    run(context, client, TF_ES_SYNC | TF_ES_READ, move |cookie| unsafe {
        let property = document.GetAppProperty(&GUID_PROP_INPUTSCOPE)?;
        // Some applications only answer for a range that has text in it, so
        // if the caret gets no answer, ask about the whole document.
        let at_caret = selection(&document, cookie)
            .and_then(|range| property.GetValue(cookie, &range))
            .ok()
            .filter(|value| !value.is_empty());
        let value = match at_caret {
            Some(value) => value,
            None => {
                let range = document.GetStart(cookie)?;
                let end = document.GetEnd(cookie)?;
                range.ShiftEndToRange(cookie, &end, TF_ANCHOR_END)?;
                property.GetValue(cookie, &range)?
            }
        };
        // No input scope declared at all is a definite "not a password field".
        let Ok(scope) = IUnknown::try_from(&value).and_then(|u| u.cast::<ITfInputScope>()) else {
            out.set(Some(false));
            return Ok(());
        };
        let mut scopes: *mut InputScope = std::ptr::null_mut();
        let mut count = 0;
        scope.GetInputScopes(&mut scopes, &mut count)?;
        let password = !scopes.is_null()
            && std::slice::from_raw_parts(scopes, count as usize).contains(&IS_PASSWORD);
        CoTaskMemFree(Some(scopes as *const _));
        out.set(Some(password));
        Ok(())
    });
    found.get()
}
