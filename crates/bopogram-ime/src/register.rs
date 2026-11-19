//! Registration of the text service. This is what `regsvr32` runs.
//!
//! It writes machine-wide state and needs administrator rights:
//! - `HKLM\SOFTWARE\Classes\CLSID\{CLSID}`: where the COM class lives.
//! - `HKLM\SOFTWARE\Microsoft\CTF\TIP\{CLSID}`: the language profile and the
//!   categories, written by the TSF registration interfaces, not by us.
//!
//! Nothing in the input method calls these functions on its own.

use windows::core::{Error, Result, GUID, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, E_FAIL, WIN32_ERROR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
    KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::{
    ITfCategoryMgr, ITfInputProcessorProfileMgr, CLSID_TF_CategoryMgr,
    CLSID_TF_InputProcessorProfiles, GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    GUID_TFCAT_TIP_KEYBOARD,
};

use crate::dll::{icon_path, module_path, CLSID_TEXT_SERVICE, DISPLAY_NAME, GUID_PROFILE, LANGID_ZH_TW};
use crate::{wide, wide_z};

const CATEGORIES: [GUID; 4] = [
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    // Without these two, Windows 8 and later hide the input method.
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
];

fn class_key() -> String {
    format!(r"SOFTWARE\Classes\CLSID\{{{CLSID_TEXT_SERVICE:?}}}")
}

fn check(status: WIN32_ERROR) -> Result<()> {
    status.ok()
}

fn set_string(key: &str, name: &str, value: &str) -> Result<()> {
    let key = wide_z(key);
    let name = wide_z(name);
    let value = wide_z(value);
    let bytes: Vec<u8> = value.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    unsafe {
        let mut handle = HKEY::default();
        check(RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut handle,
            None,
        ))?;
        let written = RegSetValueExW(handle, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes));
        let _ = RegCloseKey(handle);
        check(written)
    }
}

/// Run `f` with COM initialised on this thread.
fn with_com<R>(f: impl FnOnce() -> Result<R>) -> Result<R> {
    unsafe {
        let initialised = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = f();
        if initialised {
            CoUninitialize();
        }
        result
    }
}

pub fn register() -> Result<()> {
    let path = module_path().ok_or_else(|| Error::from(E_FAIL))?;
    let path = path.to_string_lossy();
    let key = class_key();
    set_string(&key, "", DISPLAY_NAME)?;
    let server = format!(r"{key}\InprocServer32");
    set_string(&server, "", &path)?;
    set_string(&server, "ThreadingModel", "Apartment")?;

    // Installed beside the programs; without it Windows shows its generic icon.
    let icon = icon_path().filter(|icon| icon.exists()).map(|icon| wide(&icon.to_string_lossy()));
    with_com(|| unsafe {
        let profiles: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        profiles.RegisterProfile(
            &CLSID_TEXT_SERVICE,
            LANGID_ZH_TW,
            &GUID_PROFILE,
            &wide(DISPLAY_NAME),
            icon.as_deref().unwrap_or(&[]),
            0,
            HKL::default(),
            0,
            true,
            0,
        )?;
        let categories: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        for category in &CATEGORIES {
            categories.RegisterCategory(&CLSID_TEXT_SERVICE, category, &CLSID_TEXT_SERVICE)?;
        }
        Ok(())
    })
}

/// Remove everything `register` wrote. Parts that are already gone are skipped.
pub fn unregister() -> Result<()> {
    let _ = with_com(|| unsafe {
        if let Ok(categories) = CoCreateInstance::<_, ITfCategoryMgr>(
            &CLSID_TF_CategoryMgr,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            for category in &CATEGORIES {
                let _ = categories.UnregisterCategory(
                    &CLSID_TEXT_SERVICE,
                    category,
                    &CLSID_TEXT_SERVICE,
                );
            }
        }
        let profiles: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        profiles.UnregisterProfile(&CLSID_TEXT_SERVICE, LANGID_ZH_TW, &GUID_PROFILE, 0)
    });
    let key = wide_z(&class_key());
    let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr())) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(status)
}
