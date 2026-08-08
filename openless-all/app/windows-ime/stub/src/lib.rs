use std::os::raw::c_void;

type HRESULT = i32;
type REFCLSID = *const c_void;
type REFIID = *const c_void;

const S_OK: HRESULT = 0;
const CLASS_E_CLASSNOTAVAILABLE: HRESULT = 0x8004_0111u32 as i32; // 0x80040111

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_OK
}

#[no_mangle]
pub extern "system" fn DllGetClassObject(_rclsid: REFCLSID, _riid: REFIID, _ppv: *mut *mut c_void) -> HRESULT {
    CLASS_E_CLASSNOTAVAILABLE
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    S_OK
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    S_OK
}
