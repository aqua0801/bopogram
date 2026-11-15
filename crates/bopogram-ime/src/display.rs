//! The display attribute of the composition: a dotted underline, so the
//! user can tell text that is still being composed from committed text.

use std::cell::Cell;

use windows::core::{implement, Error, Result, BSTR, GUID};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, S_FALSE};
use windows::Win32::UI::TextServices::{
    IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl, ITfDisplayAttributeInfo,
    ITfDisplayAttributeInfo_Impl, TF_ATTR_INPUT, TF_DISPLAYATTRIBUTE, TF_LS_DOT,
};

use crate::dll::GUID_DISPLAY_ATTRIBUTE;

#[implement(ITfDisplayAttributeInfo)]
pub struct CompositionAttribute;

impl CompositionAttribute {
    pub fn create() -> ITfDisplayAttributeInfo {
        CompositionAttribute.into()
    }
}

impl ITfDisplayAttributeInfo_Impl for CompositionAttribute_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(GUID_DISPLAY_ATTRIBUTE)
    }

    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("Bopogram composition"))
    }

    fn GetAttributeInfo(&self, attribute: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        if attribute.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // Colours left at "none": the application's own text colours apply.
        unsafe {
            *attribute = TF_DISPLAYATTRIBUTE {
                lsStyle: TF_LS_DOT,
                bAttr: TF_ATTR_INPUT,
                ..TF_DISPLAYATTRIBUTE::default()
            };
        }
        Ok(())
    }

    fn SetAttributeInfo(&self, _attribute: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Reset(&self) -> Result<()> {
        Ok(())
    }
}

/// Enumerates the single attribute above.
#[implement(IEnumTfDisplayAttributeInfo)]
pub struct AttributeEnumerator {
    done: Cell<bool>,
}

impl AttributeEnumerator {
    pub fn create() -> IEnumTfDisplayAttributeInfo {
        AttributeEnumerator { done: Cell::new(false) }.into()
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for AttributeEnumerator_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(AttributeEnumerator { done: Cell::new(self.done.get()) }.into())
    }

    fn Next(
        &self,
        count: u32,
        info: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> Result<()> {
        if info.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut n = 0;
        if count > 0 && !self.done.replace(true) {
            unsafe { info.write(Some(CompositionAttribute::create())) };
            n = 1;
        }
        if !fetched.is_null() {
            unsafe { *fetched = n };
        }
        if n == count {
            Ok(())
        } else {
            Err(Error::from_hresult(S_FALSE))
        }
    }

    fn Reset(&self) -> Result<()> {
        self.done.set(false);
        Ok(())
    }

    fn Skip(&self, count: u32) -> Result<()> {
        if count > 0 && self.done.replace(true) {
            return Err(Error::from_hresult(S_FALSE));
        }
        Ok(())
    }
}
