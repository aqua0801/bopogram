//! Loads the built DLL the way COM would and checks that the class factory
//! hands out a text service with the interfaces TSF asks for. Nothing is
//! registered; the DLL is loaded by path.
//!
//!   cargo build --release
//!   cargo test -p bopogram-ime --test com -- --ignored

use std::ffi::c_void;
use std::path::Path;

use windows::core::{s, Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::System::Com::{CoInitializeEx, IClassFactory, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::TextServices::{
    ITfCompositionSink, ITfDisplayAttributeProvider, ITfKeyEventSink, ITfTextInputProcessor,
    ITfTextInputProcessorEx,
};

use bopogram_ime::{CLSID_TEXT_SERVICE, GUID_DISPLAY_ATTRIBUTE};

type GetClassObject =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

#[test]
#[ignore = "needs target/release/bopogram_ime.dll"]
fn class_factory_creates_the_text_service() {
    let dll = Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\target\release\bopogram_ime.dll");
    let dll: Vec<u16> = dll.to_string_lossy().encode_utf16().chain([0]).collect();
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        let library = LoadLibraryW(PCWSTR(dll.as_ptr())).expect("load the DLL");
        let entry = GetProcAddress(library, s!("DllGetClassObject")).expect("export");
        let get_class_object: GetClassObject = std::mem::transmute(entry);

        // Some other class: refused.
        let mut raw = std::ptr::null_mut();
        assert!(get_class_object(&GUID::zeroed(), &IClassFactory::IID, &mut raw).is_err());

        get_class_object(&CLSID_TEXT_SERVICE, &IClassFactory::IID, &mut raw).ok().unwrap();
        let factory = IClassFactory::from_raw(raw);
        let service: ITfTextInputProcessorEx = factory.CreateInstance(None).unwrap();
        service.cast::<ITfTextInputProcessor>().unwrap();
        service.cast::<ITfKeyEventSink>().unwrap();
        service.cast::<ITfCompositionSink>().unwrap();

        let provider: ITfDisplayAttributeProvider = service.cast().unwrap();
        let attributes = provider.EnumDisplayAttributeInfo().unwrap();
        let mut info = [None];
        let mut fetched = 0;
        attributes.Next(&mut info, &mut fetched).unwrap();
        assert_eq!(fetched, 1);
        let info = info[0].take().unwrap();
        assert_eq!(info.GetGUID().unwrap(), GUID_DISPLAY_ATTRIBUTE);
        let mut attribute = Default::default();
        info.GetAttributeInfo(&mut attribute).unwrap();
        provider.GetDisplayAttributeInfo(&GUID_DISPLAY_ATTRIBUTE).unwrap();
        assert!(provider.GetDisplayAttributeInfo(&GUID::zeroed()).is_err());

        // Deactivating a service that was never activated must be harmless.
        service.Deactivate().unwrap();
    }
}
