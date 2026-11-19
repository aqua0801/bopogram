//! The candidate list as a TSF UI element.
//!
//! While the candidate window is open the text service announces this
//! element to TSF. An application that draws input method UI itself (full
//! screen games, typically) answers that our window must not be shown; it
//! then reads the candidates from here and may choose one through the
//! behaviour interface. Everything else gets our own window, as before.
//!
//! The element holds no candidates of its own: it reads the controller's
//! through the text service every time it is asked.

use std::rc::Weak;

use windows::core::{implement, Result, BOOL, BSTR, GUID};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_POINTER};
use windows::Win32::UI::TextServices::{
    ITfCandidateListUIElement, ITfCandidateListUIElementBehavior,
    ITfCandidateListUIElementBehavior_Impl, ITfCandidateListUIElement_Impl, ITfDocumentMgr,
    ITfUIElement, ITfUIElement_Impl, TF_CLUIE_COUNT, TF_CLUIE_CURRENTPAGE, TF_CLUIE_DOCUMENTMGR,
    TF_CLUIE_PAGEINDEX, TF_CLUIE_SELECTION, TF_CLUIE_STRING,
};

use crate::service::{self, Shared};
use crate::wide;

/// Identifies this element among the text service's UI elements.
const GUID_CANDIDATE_LIST: GUID = GUID::from_u128(0x3b0f_4c51_8a8e_4d36_9f2f_6a1c_52d7_e0b4);

#[implement(ITfCandidateListUIElementBehavior, ITfCandidateListUIElement, ITfUIElement)]
pub struct CandidateList {
    /// Weak: the text service owns this element, not the other way round.
    state: Weak<std::cell::RefCell<service::State>>,
    document: Option<ITfDocumentMgr>,
}

impl CandidateList {
    pub(crate) fn new(state: &Shared, document: Option<ITfDocumentMgr>) -> Self {
        CandidateList { state: std::rc::Rc::downgrade(state), document }
    }

    fn state(&self) -> Result<Shared> {
        self.state.upgrade().ok_or_else(|| E_FAIL.into())
    }

    /// The whole list and the highlighted index.
    fn list(&self) -> Result<(Vec<String>, usize)> {
        service::candidate_list(&self.state()?).ok_or_else(|| E_FAIL.into())
    }
}

impl ITfUIElement_Impl for CandidateList_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("Bopogram 候選字"))
    }

    fn GetGUID(&self) -> Result<GUID> {
        Ok(GUID_CANDIDATE_LIST)
    }

    fn Show(&self, show: BOOL) -> Result<()> {
        service::show_candidate_window(&self.state()?, show.as_bool());
        Ok(())
    }

    fn IsShown(&self) -> Result<BOOL> {
        Ok(service::candidate_window_shown(&self.state()?).into())
    }
}

impl ITfCandidateListUIElement_Impl for CandidateList_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        // Everything may have changed with any update; the list is short.
        Ok(TF_CLUIE_DOCUMENTMGR
            | TF_CLUIE_COUNT
            | TF_CLUIE_SELECTION
            | TF_CLUIE_STRING
            | TF_CLUIE_PAGEINDEX
            | TF_CLUIE_CURRENTPAGE)
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        self.document.clone().ok_or_else(|| E_FAIL.into())
    }

    fn GetCount(&self) -> Result<u32> {
        Ok(self.list()?.0.len() as u32)
    }

    fn GetSelection(&self) -> Result<u32> {
        Ok(self.list()?.1 as u32)
    }

    fn GetString(&self, index: u32) -> Result<BSTR> {
        let (words, _) = self.list()?;
        let word = words.get(index as usize).ok_or(E_INVALIDARG)?;
        Ok(BSTR::from_wide(&wide(word)))
    }

    /// With a null buffer, only the page count is wanted.
    fn GetPageIndex(&self, index: *mut u32, size: u32, count: *mut u32) -> Result<()> {
        if count.is_null() {
            return Err(E_POINTER.into());
        }
        let pages = service::page_starts(&self.state()?);
        unsafe {
            *count = pages.len() as u32;
            if !index.is_null() {
                let room = (size as usize).min(pages.len());
                for (i, start) in pages.iter().take(room).enumerate() {
                    *index.add(i) = *start as u32;
                }
            }
        }
        Ok(())
    }

    /// Our pages have the size set in the settings; other paging is declined.
    fn SetPageIndex(&self, _index: *const u32, _count: u32) -> Result<()> {
        Err(E_FAIL.into())
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        let state = self.state()?;
        let current = service::page_start(&state);
        Ok(service::page_starts(&state).iter().position(|&start| start == current).unwrap_or(0) as u32)
    }
}

impl ITfCandidateListUIElementBehavior_Impl for CandidateList_Impl {
    fn SetSelection(&self, index: u32) -> Result<()> {
        service::highlight_candidate(&self.state()?, index as usize);
        Ok(())
    }

    fn Finalize(&self) -> Result<()> {
        let state = self.state()?;
        let (_, selected) = self.list()?;
        service::pick_candidate(&state, selected);
        Ok(())
    }

    fn Abort(&self) -> Result<()> {
        service::close_candidates(&self.state()?);
        Ok(())
    }
}

/// The element as TSF wants it for `BeginUIElement`.
pub(crate) fn element(state: &Shared, document: Option<ITfDocumentMgr>) -> ITfUIElement {
    CandidateList::new(state, document).into()
}
