use std::io::Error as IoError;
use std::mem::MaybeUninit;
use std::ptr;
use winapi::um::{combaseapi, mmdeviceapi, objbase, winnt};
use winapi::Interface;

pub fn check_result(result: winnt::HRESULT) -> Result<(), IoError> {
    if result < 0 {
        Err(IoError::from_raw_os_error(result))
    } else {
        Ok(())
    }
}

lazy_static! {
    pub static ref ENUMERATOR: Enumerator = {
        initialized();

        // building the devices enumerator object
        unsafe {
            let mut enumerator = MaybeUninit::<*mut mmdeviceapi::IMMDeviceEnumerator>::uninit();

            let hresult = combaseapi::CoCreateInstance(
                &mmdeviceapi::CLSID_MMDeviceEnumerator,
                ptr::null_mut(),
                combaseapi::CLSCTX_ALL,
                &mmdeviceapi::IMMDeviceEnumerator::uuidof(),
                enumerator.as_mut_ptr() as *mut *mut winapi::ctypes::c_void,
            );

            check_result(hresult).unwrap();
            Enumerator(enumerator.assume_init())
        }
    };
}

pub struct Enumerator(pub *mut mmdeviceapi::IMMDeviceEnumerator);
unsafe impl Send for Enumerator {}
unsafe impl Sync for Enumerator {}

thread_local!(static COM_INITIALIZED: ComInitialized = {
    unsafe {
        check_result(combaseapi::CoInitializeEx(ptr::null_mut(), objbase::COINIT_MULTITHREADED)).unwrap();
        ComInitialized(ptr::null_mut())
    }
});

pub struct ComInitialized(*mut ());

impl Drop for ComInitialized {
    fn drop(&mut self) {
        unsafe { combaseapi::CoUninitialize() };
    }
}

pub fn initialized() {
    COM_INITIALIZED.with(|_| {});
}
