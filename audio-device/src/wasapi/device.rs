use crate::wasapi::com;
use crate::wasapi::com::check_result;
use crate::wasapi::device::DefaultFormatError::BackendSpecific;
use crate::wasapi::event_loop::WasapiEventLoopRegistration;
use crate::wasapi::ms_multimedia::{
    format_from_waveformatex_ptr, format_to_waveformatextensible, is_format_supported,
    WaveFormatExPtr,
};
use crate::wasapi::stream::Format;
use std::ffi::OsString;
use std::io::Error as IoError;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::os::windows::ffi::OsStringExt;
use std::{ptr, slice};
use winapi::ctypes;
use winapi::shared::{devpkey, guiddef, mmreg, winerror, wtypes};
use winapi::um::audioclient::AUDCLNT_E_DEVICE_INVALIDATED;
use winapi::um::audiosessiontypes::AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
use winapi::um::mmdeviceapi::IMMDevice;
use winapi::um::mmdeviceapi::IMMDeviceCollection;
use winapi::um::mmdeviceapi::DEVICE_STATE_ACTIVE;
use winapi::um::winsock2::u_int;
use winapi::um::{
    audioclient, audiosessiontypes, combaseapi, coml2api, mmdeviceapi, synchapi, winnt,
};

pub struct InputDevice {
    device: IMMDeviceWrapper,
    pub audio_client: IAudioClientWrapper,
}

pub struct OutputDevice {
    device: IMMDeviceWrapper,
    pub audio_client: IAudioClientWrapper,
}

pub struct IAudioClientWrapper(*mut audioclient::IAudioClient);

impl IAudioClientWrapper {
    pub fn initialize(&self, format: &Format) -> Result<mmreg::WAVEFORMATEX, BuildStreamError> {
        let audio_client = self.0;
        let format_attempt =
            format_to_waveformatextensible(format).ok_or(BuildStreamError::FormatNotSupported)?;
        let share_mode = audiosessiontypes::AUDCLNT_SHAREMODE_SHARED;

        match is_format_supported(audio_client, &format_attempt.Format) {
            Ok(false) => return Err(BuildStreamError::FormatNotSupported),
            Err(_) => return Err(BuildStreamError::DeviceNotAvailable),
            _ => (),
        }

        let hresult = unsafe {
            (*audio_client).Initialize(
                share_mode,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                0,
                0,
                &format_attempt.Format,
                ptr::null(),
            )
        };
        match check_result(hresult) {
            Err(ref e) if e.raw_os_error() == Some(AUDCLNT_E_DEVICE_INVALIDATED) => {
                unsafe { (*audio_client).Release() };
                return Err(BuildStreamError::DeviceNotAvailable);
            }
            Err(e) => {
                unsafe { (*audio_client).Release() };
                let description = format!("{}", e);
                return Err(BuildStreamError::BackendSpecific(description));
            }
            Ok(()) => (),
        };

        Ok(format_attempt.Format)
    }

    pub fn get_buffer_max_frames(&self) -> Result<u32, BuildStreamError> {
        let audio_client = self.0;
        let mut max_frames_in_buffer = MaybeUninit::<u32>::uninit();
        let hresult = unsafe { (*audio_client).GetBufferSize(max_frames_in_buffer.as_mut_ptr()) };

        match check_result(hresult) {
            Err(ref e) if e.raw_os_error() == Some(AUDCLNT_E_DEVICE_INVALIDATED) => {
                unsafe { (*audio_client).Release() };
                return Err(BuildStreamError::DeviceNotAvailable);
            }
            Err(e) => {
                unsafe { (*audio_client).Release() };
                let description = format!("failed to obtain buffer size: {}", e);
                return Err(BuildStreamError::BackendSpecific(description));
            }
            Ok(()) => (),
        }

        Ok(unsafe { max_frames_in_buffer.assume_init() })
    }

    pub fn create_notify(&self) -> Result<WasapiEventLoopRegistration, BuildStreamError> {
        let audio_client = self.0;
        let event = unsafe { synchapi::CreateEventA(ptr::null_mut(), 0, 0, ptr::null()) };
        if event.is_null() {
            unsafe { (*audio_client).Release() };
            return Err(BuildStreamError::BackendSpecific(
                "failed to create event".to_string(),
            ));
        }

        if let Err(e) = check_result(unsafe { (*audio_client).SetEventHandle(event) }) {
            unsafe { (*audio_client).Release() };
            let description = format!("failed to call SetEventHandle: {}", e);
            return Err(BuildStreamError::BackendSpecific(description));
        }

        Ok(WasapiEventLoopRegistration { win_handle: event })
    }

    fn create_client<T>(&self, interface_id: &guiddef::GUID) -> Result<T, BuildStreamError> {
        let audio_client = self.0;
        let mut render_client = MaybeUninit::<T>::uninit();
        let hresult = unsafe {
            (*audio_client).GetService(
                interface_id,
                render_client.as_mut_ptr() as *mut *mut ctypes::c_void,
            )
        };

        match check_result(hresult) {
            Err(ref e) if e.raw_os_error() == Some(AUDCLNT_E_DEVICE_INVALIDATED) => {
                unsafe { (*audio_client).Release() };
                return Err(BuildStreamError::DeviceNotAvailable);
            }
            Err(e) => {
                unsafe { (*audio_client).Release() };
                let description = format!("failed to build render client: {}", e);
                return Err(BuildStreamError::BackendSpecific(description));
            }
            Ok(()) => (),
        };

        Ok(unsafe { render_client.assume_init() })
    }
}

impl Deref for IAudioClientWrapper {
    type Target = *mut audioclient::IAudioClient;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
pub enum DefaultFormatError {
    DeviceNotAvailable,
    StreamTypeNotSupported,
    BackendSpecific(String),
}

impl IAudioClientWrapper {
    fn default_format(&self) -> Result<Format, DefaultFormatError> {
        let client = self.0;
        let mut format_ptr = MaybeUninit::<*mut mmreg::WAVEFORMATEX>::uninit();
        match com::check_result(unsafe { (*client).GetMixFormat(format_ptr.as_mut_ptr()) }) {
            Err(ref e) if e.raw_os_error() == Some(AUDCLNT_E_DEVICE_INVALIDATED) => {
                return Err(DefaultFormatError::DeviceNotAvailable);
            }
            Err(e) => {
                let description = format!("{}", e);
                return Err(BackendSpecific(description));
            }
            Ok(()) => (),
        };

        let format_ptr = WaveFormatExPtr(unsafe { format_ptr.assume_init() });
        unsafe {
            format_from_waveformatex_ptr(format_ptr.0)
                .ok_or(DefaultFormatError::StreamTypeNotSupported)
        }
    }

    fn from_immdevice(
        immdevice: *mut mmdeviceapi::IMMDevice,
    ) -> Result<IAudioClientWrapper, IoError> {
        let mut audio_client = MaybeUninit::<*mut audioclient::IAudioClient>::uninit();
        let hresult = unsafe {
            (*immdevice).Activate(
                &audioclient::IID_IAudioClient,
                combaseapi::CLSCTX_ALL,
                ptr::null_mut(),
                audio_client.as_mut_ptr() as *mut *mut ctypes::c_void,
            )
        };
        com::check_result(hresult)?;
        Ok(IAudioClientWrapper(unsafe { audio_client.assume_init() }))
    }
}

struct IMMDeviceWrapper(*mut mmdeviceapi::IMMDevice);

#[derive(Debug)]
pub struct DeviceNameError {
    pub description: String,
}

impl IMMDeviceWrapper {
    fn name(&self) -> Result<String, DeviceNameError> {
        let mut property_store = ptr::null_mut();
        unsafe { (*self.0).OpenPropertyStore(coml2api::STGM_READ, &mut property_store) };

        let mut property_value = MaybeUninit::uninit();
        if let Err(err) = com::check_result(unsafe {
            (*property_store).GetValue(
                &devpkey::DEVPKEY_Device_FriendlyName as *const _ as *const _,
                property_value.as_mut_ptr(),
            )
        }) {
            let description = format!("failed to retrieve name from property store: {}", err);
            return Err(DeviceNameError { description });
        }

        let mut property_value = unsafe { property_value.assume_init() };
        if property_value.vt != wtypes::VT_LPWSTR as u16 {
            let description = format!(
                "property store produced invalid data: {:?}",
                property_value.vt
            );
            return Err(DeviceNameError { description });
        }
        let ptr_usize: usize = unsafe { *(&property_value.data as *const _ as *const usize) };
        let ptr_utf16 = ptr_usize as *const u16;

        let mut len = 0;
        while unsafe { *ptr_utf16.offset(len) } != 0 {
            len += 1;
        }

        let name_slice = unsafe { slice::from_raw_parts(ptr_utf16, len as usize) };
        let name_os_string: OsString = OsStringExt::from_wide(name_slice);
        let name_string = name_os_string.to_string_lossy().into();

        unsafe { combaseapi::PropVariantClear(&mut property_value) };
        Ok(name_string)
    }

    fn id(&self) -> String {
        // TODO: Return the actual ID --- more pointer work
        "LATER NERD".to_string()
    }

    fn default(data_flow: mmdeviceapi::EDataFlow) -> Option<IMMDeviceWrapper> {
        let mut device = MaybeUninit::uninit();
        let hres = unsafe {
            (*com::ENUMERATOR.0).GetDefaultAudioEndpoint(
                data_flow,
                mmdeviceapi::eConsole,
                device.as_mut_ptr(),
            )
        };

        // This should be a constant once HRESULT_FROM_WIN32(..) is a const fn
        let e_notfound: winnt::HRESULT = winerror::HRESULT_FROM_WIN32(winerror::ERROR_NOT_FOUND);
        match com::check_result(hres) {
            Err(ref e) if e.raw_os_error() == Some(e_notfound) => {
                return None;
            }
            Err(e) => {
                panic!("{}", e) // hmm, should we bubble up the error instead?
            }
            Ok(()) => (),
        };
        Some(IMMDeviceWrapper(unsafe { device.assume_init() }))
    }

    fn endpoints(data_flow: mmdeviceapi::EDataFlow) -> Vec<IMMDeviceWrapper> {
        let mut device_collection: MaybeUninit<*mut IMMDeviceCollection> = MaybeUninit::uninit();
        let hres = unsafe {
            (*com::ENUMERATOR.0).EnumAudioEndpoints(
                data_flow,
                DEVICE_STATE_ACTIVE,
                device_collection.as_mut_ptr(),
            )
        };
        match com::check_result(hres) {
            Err(e) => {
                panic!("{}", e)
            }
            Ok(()) => (),
        }

        // TODO: This is kinda nasty -- thanks Billy Gates
        unsafe {
            let filled_collection: *mut IMMDeviceCollection = device_collection.assume_init();

            let mut device_count: MaybeUninit<u_int> = MaybeUninit::uninit();
            let hres = (*filled_collection).GetCount(device_count.as_mut_ptr());
            com::check_result(hres).unwrap();

            let device_count = device_count.assume_init();
            println!("Found {} audio devices", device_count);

            let mut result: Vec<IMMDeviceWrapper> = Vec::new();
            for i in 0..device_count {
                let mut device: MaybeUninit<*mut IMMDevice> = MaybeUninit::uninit();
                let hres = (*filled_collection).Item(i, device.as_mut_ptr());
                com::check_result(hres).unwrap();

                let device_wrapper = IMMDeviceWrapper(device.assume_init());
                println!("Found {}", device_wrapper.name().unwrap());

                result.push(device_wrapper);
            }

            result
        }
    }
}

impl InputDevice {
    pub fn name(&self) -> Result<String, DeviceNameError> {
        self.device.name()
    }

    pub fn id(&self) -> String {
        self.device.id()
    }

    pub fn default_format(&self) -> Result<Format, DefaultFormatError> {
        self.audio_client.default_format()
    }

    pub fn create_client(&self) -> Result<*mut audioclient::IAudioCaptureClient, BuildStreamError> {
        self.audio_client
            .create_client(&audioclient::IID_IAudioCaptureClient)
    }

    pub fn default() -> Option<InputDevice> {
        com::initialized();
        IMMDeviceWrapper::default(mmdeviceapi::eCapture).and_then(|device| {
            IAudioClientWrapper::from_immdevice(device.0)
                .ok()
                .map(|audio_client| InputDevice {
                    device,
                    audio_client,
                })
        })
    }

    pub fn get_list() -> Vec<InputDevice> {
        com::initialized();
        IMMDeviceWrapper::endpoints(mmdeviceapi::eCapture)
            .into_iter()
            .map(|wrapper| InputDevice {
                audio_client: IAudioClientWrapper::from_immdevice(wrapper.0).unwrap(),
                device: wrapper,
            })
            .collect()
    }
}

impl OutputDevice {
    pub fn name(&self) -> Result<String, DeviceNameError> {
        self.device.name()
    }

    pub fn id(&self) -> String {
        self.device.id()
    }

    pub fn default_format(&self) -> Result<Format, DefaultFormatError> {
        self.audio_client.default_format()
    }

    pub fn create_client(&self) -> Result<*mut audioclient::IAudioRenderClient, BuildStreamError> {
        self.audio_client
            .create_client(&audioclient::IID_IAudioRenderClient)
    }

    pub fn default() -> Option<OutputDevice> {
        com::initialized();
        IMMDeviceWrapper::default(mmdeviceapi::eRender).and_then(|device| {
            IAudioClientWrapper::from_immdevice(device.0)
                .ok()
                .map(|audio_client| OutputDevice {
                    device,
                    audio_client,
                })
        })
    }

    pub fn get_list() -> Vec<OutputDevice> {
        com::initialized();
        IMMDeviceWrapper::endpoints(mmdeviceapi::eRender)
            .into_iter()
            .map(|wrapper| OutputDevice {
                audio_client: IAudioClientWrapper::from_immdevice(wrapper.0).unwrap(),
                device: wrapper,
            })
            .collect()
    }
}

#[derive(Debug)]
pub enum BuildStreamError {
    DeviceNotAvailable,
    FormatNotSupported,
    BackendSpecific(String),
}
