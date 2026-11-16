//! DLL entry points, the class factory, and the identifiers of this text service.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows::core::{implement, IUnknown, Interface, Ref, BOOL, GUID, HRESULT};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, HINSTANCE, HMODULE,
    S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

use crate::service::TextService;
use crate::{guard, register};

/// COM class of the text service.
pub const CLSID_TEXT_SERVICE: GUID = GUID::from_u128(0xb6b6d901_0bbf_4287_a302_f265dbe9a46c);
/// The one language profile (zh-TW) of the text service.
pub const GUID_PROFILE: GUID = GUID::from_u128(0x79effb1b_3ce8_41db_9ca2_3318ec0aed82);
/// Display attribute of the composition (the underline).
pub const GUID_DISPLAY_ATTRIBUTE: GUID = GUID::from_u128(0x6ef85a84_f6e7_4622_b6bf_db6d3fd1bd12);

/// Name shown in the language list.
pub const DISPLAY_NAME: &str = "Bopogram 注音（試用版）";
/// Traditional Chinese (Taiwan).
pub const LANGID_ZH_TW: u16 = 0x0404;
/// The engine host, expected in the same folder as this DLL.
const HOST_EXE: &str = "bopogram-host.exe";

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

#[no_mangle]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0, Ordering::Relaxed);
    }
    true.into()
}

pub fn module() -> HMODULE {
    HMODULE(MODULE.load(Ordering::Relaxed))
}

/// Full path of this DLL.
pub fn module_path() -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    let len = unsafe { GetModuleFileNameW(Some(module()), &mut buf) } as usize;
    (len > 0 && len < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

/// The settings window, a program of its own in the same folder.
pub fn settings_path() -> Option<PathBuf> {
    Some(module_path()?.with_file_name("bopogram-settings.exe"))
}

pub fn host_path() -> Option<PathBuf> {
    Some(module_path()?.with_file_name(HOST_EXE))
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if object.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *object = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        guard(Err(E_FAIL.into()), || {
            let service: IUnknown = TextService::new().into();
            unsafe { service.query(iid, object).ok() }
        })
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

#[no_mangle]
unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    if clsid.is_null() || iid.is_null() || object.is_null() {
        return E_POINTER;
    }
    unsafe { *object = std::ptr::null_mut() };
    if unsafe { *clsid } != CLSID_TEXT_SERVICE {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    guard(E_FAIL, || {
        let factory: IClassFactory = Factory.into();
        unsafe { factory.query(iid, object) }
    })
}

/// Never unload: edit sessions and display attribute objects handed to TSF
/// may outlive the text service, and their code lives here.
#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    guard(E_FAIL, || match register::register() {
        Ok(()) => S_OK,
        Err(err) => {
            // Do not leave half a registration behind.
            let _ = register::unregister();
            err.code()
        }
    })
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    guard(E_FAIL, || register::unregister().map_or_else(|err| err.code(), |_| S_OK))
}
