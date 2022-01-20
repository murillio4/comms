use crate::wasapi::com;
use crate::wasapi::device::DefaultFormatError::BackendSpecific;
use crate::wasapi::event_loop::WasapiEventLoopRegistration;
use crate::wasapi::ms_multimedia::{
    format_from_waveformatex_ptr, format_to_waveformatextensible, is_format_supported,
    MmregSupportedFormatsError, WaveFormatExPtr,
};
use crate::wasapi::stream::Format;
use std::ffi::OsString;
use std::ops::Deref;
use std::os::windows::ffi::OsStringExt;
use std::{ptr, slice};
use windows::core::Interface;
use windows::Win32::Devices::Properties;
use windows::Win32::Foundation;
use windows::Win32::Media::Audio;
use windows::Win32::System::Com::StructuredStorage;
use windows::Win32::System::{Com, Ole, Threading};

pub struct InputDevice {
    device: IMMDeviceWrapper,
    pub audio_client: IAudioClientWrapper,
}

pub struct OutputDevice {
    device: IMMDeviceWrapper,
    pub audio_client: IAudioClientWrapper,
}

pub struct IAudioClientWrapper(Audio::IAudioClient);

impl IAudioClientWrapper {
    pub fn initialize(&self, format: &Format) -> Result<Audio::WAVEFORMATEX, BuildStreamError> {
        let audio_client = &self.0;
        let mut format_attempt = format_to_waveformatextensible(format);

        match is_format_supported(audio_client, &format_attempt.Format) {
            Ok(None) => {}
            Ok(Some(format)) => {
                format_attempt.Format.nChannels = unsafe { (*format).nChannels };
                format_attempt.Format.nSamplesPerSec = unsafe { (*format).nSamplesPerSec };
                format_attempt.Format.nAvgBytesPerSec = unsafe { (*format).nAvgBytesPerSec };
                format_attempt.Format.nBlockAlign = unsafe { (*format).nBlockAlign };
                format_attempt.Format.wBitsPerSample = unsafe { (*format).wBitsPerSample };
                format_attempt.Format.cbSize = unsafe { (*format).cbSize };
            }
            Err(MmregSupportedFormatsError::DeviceNotAvailable) => {
                return Err(BuildStreamError::DeviceNotAvailable)
            }
            Err(MmregSupportedFormatsError::Other) => {
                return Err(BuildStreamError::BackendSpecific(
                    "Unexpected failure during format-support checking".to_string(),
                ))
            }
        }

        match unsafe {
            audio_client.Initialize(
                Audio::AUDCLNT_SHAREMODE_SHARED,
                Audio::AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                0,
                0,
                &format_attempt.Format,
                ptr::null(),
            )
        } {
            Err(ref e) if e.code() == Audio::AUDCLNT_E_DEVICE_INVALIDATED => {
                return Err(BuildStreamError::DeviceNotAvailable);
            }
            Err(e) => {
                let description = format!("{}", e);
                return Err(BuildStreamError::BackendSpecific(description));
            }
            Ok(()) => (),
        };

        Ok(format_attempt.Format)
    }

    pub fn get_buffer_max_frames(&self) -> Result<u32, BuildStreamError> {
        match unsafe { self.0.GetBufferSize() } {
            Err(ref e) if e.code() == Audio::AUDCLNT_E_DEVICE_INVALIDATED => {
                Err(BuildStreamError::DeviceNotAvailable)
            }
            Err(e) => {
                let description = format!("failed to obtain buffer size: {}", e);
                Err(BuildStreamError::BackendSpecific(description))
            }
            Ok(max_frames_in_buffer) => Ok(max_frames_in_buffer),
        }
    }

    pub fn create_notify(&self) -> Result<WasapiEventLoopRegistration, BuildStreamError> {
        let event = unsafe { Threading::CreateEventA(ptr::null(), false, false, None) };
        if event.is_invalid() {
            return Err(BuildStreamError::BackendSpecific(
                "failed to create event".to_string(),
            ));
        }

        if let Err(e) = unsafe { self.0.SetEventHandle(event) } {
            let description = format!("failed to call SetEventHandle: {}", e);
            return Err(BuildStreamError::BackendSpecific(description));
        }

        Ok(WasapiEventLoopRegistration { win_handle: event })
    }

    fn create_client<T>(&self, interface_id: &windows::core::GUID) -> Result<T, BuildStreamError> {
        let mut client_ptr: Option<T> = None;
        match unsafe {
            self.0
                .GetService(interface_id, &mut client_ptr as *mut _ as _)
        } {
            Err(ref e) if e.code() == Audio::AUDCLNT_E_DEVICE_INVALIDATED => {
                return Err(BuildStreamError::DeviceNotAvailable);
            }
            Err(e) => {
                let description = format!("failed to build render client: {}", e);
                return Err(BuildStreamError::BackendSpecific(description));
            }
            Ok(()) => (),
        };

        Ok(client_ptr.unwrap())
    }
}

impl Deref for IAudioClientWrapper {
    type Target = Audio::IAudioClient;

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
        let format_ptr = match unsafe { self.0.GetMixFormat() } {
            Err(ref e) if e.code() == Audio::AUDCLNT_E_DEVICE_INVALIDATED => {
                return Err(DefaultFormatError::DeviceNotAvailable);
            }
            Err(e) => {
                let description = format!("{}", e);
                return Err(BackendSpecific(description));
            }
            Ok(format) => format,
        };

        let format_ptr = WaveFormatExPtr(format_ptr);
        unsafe {
            format_from_waveformatex_ptr(format_ptr.0)
                .ok_or(DefaultFormatError::StreamTypeNotSupported)
        }
    }

    fn from_immdevice(
        immdevice: &Audio::IMMDevice,
    ) -> Result<IAudioClientWrapper, windows::core::Error> {
        let mut audio_client: Option<Audio::IAudioClient> = None;
        unsafe {
            immdevice.Activate(
                &Audio::IAudioClient::IID,
                Com::CLSCTX_ALL,
                ptr::null(),
                &mut audio_client as *mut _ as _,
            )
        }?;
        Ok(IAudioClientWrapper(audio_client.unwrap()))
    }
}

struct IMMDeviceWrapper(Audio::IMMDevice);

#[derive(Debug)]
pub struct DeviceNameError {
    pub description: String,
}

impl IMMDeviceWrapper {
    fn name(&self) -> Result<String, DeviceNameError> {
        let property_store =
            unsafe { self.0.OpenPropertyStore(StructuredStorage::STGM_READ) }.unwrap();

        let mut property_value = match unsafe {
            property_store.GetValue(&Properties::DEVPKEY_Device_FriendlyName)
        } {
            Ok(property_value) => property_value,
            Err(err) => {
                let description = format!("failed to retrieve name from property store: {}", err);
                return Err(DeviceNameError { description });
            }
        };

        if unsafe { property_value.Anonymous.Anonymous.vt } != Ole::VT_LPWSTR as u16 {
            let description = format!("property store produced invalid data: {:?}", unsafe {
                property_value.Anonymous.Anonymous.vt
            });
            return Err(DeviceNameError { description });
        }
        let ptr_usize: usize =
            unsafe { *(&property_value.Anonymous.Anonymous.Anonymous as *const _ as *const usize) };
        let ptr_utf16 = ptr_usize as *const u16;

        let mut len = 0;
        while unsafe { *ptr_utf16.offset(len) } != 0 {
            len += 1;
        }

        let name_slice = unsafe { slice::from_raw_parts(ptr_utf16, len as usize) };
        let name_os_string: OsString = OsStringExt::from_wide(name_slice);
        let name_string = name_os_string.to_string_lossy().into();

        unsafe { StructuredStorage::PropVariantClear(&mut property_value) }.unwrap();
        Ok(name_string)
    }

    fn id(&self) -> String {
        // TODO: Return the actual ID --- more pointer work
        "LATER NERD".to_string()
    }

    fn default(data_flow: Audio::EDataFlow) -> Option<IMMDeviceWrapper> {
        let device = match unsafe {
            com::ENUMERATOR
                .0
                .GetDefaultAudioEndpoint(data_flow, Audio::eConsole)
        } {
            Ok(device) => device,
            Err(ref e) if e.win32_error() == Some(Foundation::ERROR_NOT_FOUND) => {
                // TODO test that !mic doesn't fail here due to no default device existing
                return None;
            }
            Err(e) => panic!("{}", e), // hmm, should we bubble up the error instead?
        };
        Some(IMMDeviceWrapper(device))
    }

    fn endpoints(data_flow: Audio::EDataFlow) -> Vec<IMMDeviceWrapper> {
        let filled_collection = unsafe {
            com::ENUMERATOR
                .0
                .EnumAudioEndpoints(data_flow, Audio::DEVICE_STATE_ACTIVE)
        }
        .unwrap();

        // TODO: This is kinda nasty -- thanks Billy Gates
        unsafe {
            let device_count = filled_collection.GetCount().unwrap();
            println!("Found {} audio devices", device_count);

            let mut result: Vec<IMMDeviceWrapper> = Vec::new();
            for i in 0..device_count {
                let device = filled_collection.Item(i).unwrap();
                let device_wrapper = IMMDeviceWrapper(device);
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

    pub fn create_client(&self) -> Result<Audio::IAudioCaptureClient, BuildStreamError> {
        self.audio_client
            .create_client(&Audio::IAudioCaptureClient::IID)
    }

    pub fn default() -> Option<InputDevice> {
        com::initialized();
        IMMDeviceWrapper::default(Audio::eCapture).and_then(|device| {
            IAudioClientWrapper::from_immdevice(&device.0)
                .ok()
                .map(|audio_client| InputDevice {
                    device,
                    audio_client,
                })
        })
    }

    pub fn get_list() -> Vec<InputDevice> {
        com::initialized();
        IMMDeviceWrapper::endpoints(Audio::eCapture)
            .into_iter()
            .map(|wrapper| InputDevice {
                audio_client: IAudioClientWrapper::from_immdevice(&wrapper.0).unwrap(),
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

    pub fn create_client(&self) -> Result<Audio::IAudioRenderClient, BuildStreamError> {
        self.audio_client
            .create_client(&Audio::IAudioRenderClient::IID)
    }

    pub fn default() -> Option<OutputDevice> {
        com::initialized();
        IMMDeviceWrapper::default(Audio::eRender).and_then(|device| {
            IAudioClientWrapper::from_immdevice(&device.0)
                .ok()
                .map(|audio_client| OutputDevice {
                    device,
                    audio_client,
                })
        })
    }

    pub fn get_list() -> Vec<OutputDevice> {
        com::initialized();
        IMMDeviceWrapper::endpoints(Audio::eRender)
            .into_iter()
            .map(|wrapper| OutputDevice {
                audio_client: IAudioClientWrapper::from_immdevice(&wrapper.0).unwrap(),
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
