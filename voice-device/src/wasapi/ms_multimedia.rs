use winapi::um::{audioclient, audiosessiontypes, combaseapi};
use winapi::shared::{mmreg, winerror, ksmedia, guiddef};
use crate::wasapi::com::check_result;
use crate::wasapi::stream::{Format, SampleFormat, SampleRate};
use winapi::shared::minwindef::{WORD, DWORD};
use std::mem;

#[derive(Debug)]
pub enum MmregSupportedFormatsError {
    DeviceNotAvailable,
}

pub fn is_format_supported(
    client: *const audioclient::IAudioClient,
    waveformatex_ptr: *const mmreg::WAVEFORMATEX,
) -> Result<bool, MmregSupportedFormatsError> {
    let is_supported = |waveformatex_ptr, mut closest_waveformatex_ptr| {
        let result = unsafe {
            (*client).IsFormatSupported(
                audiosessiontypes::AUDCLNT_SHAREMODE_SHARED,
                waveformatex_ptr,
                &mut closest_waveformatex_ptr,
            )
        };

        if result == winerror::S_FALSE {
            Ok(false)
        } else {
            match check_result(result) {
                Err(ref e) if e.raw_os_error() == Some(audioclient::AUDCLNT_E_DEVICE_INVALIDATED) => {
                    Err(MmregSupportedFormatsError::DeviceNotAvailable)
                }
                Err(_) => Ok(false),
                Ok(()) => Ok(true),
            }
        }
    };

    unsafe {
        match (*waveformatex_ptr).wFormatTag {
            mmreg::WAVE_FORMAT_PCM | mmreg::WAVE_FORMAT_IEEE_FLOAT => {
                let mut closest_waveformatex = *waveformatex_ptr;
                let closest_waveformatex_ptr = &mut closest_waveformatex as *mut _;
                is_supported(waveformatex_ptr, closest_waveformatex_ptr)
            }
            mmreg::WAVE_FORMAT_EXTENSIBLE => {
                let waveformatextensible_ptr = waveformatex_ptr as *const mmreg::WAVEFORMATEXTENSIBLE;
                let mut closest_waveformatextensible = *waveformatextensible_ptr;
                let closest_waveformatextensible_ptr = &mut closest_waveformatextensible as *mut _;
                let closest_waveformatex_ptr = closest_waveformatextensible_ptr as *mut mmreg::WAVEFORMATEX;
                is_supported(waveformatex_ptr, closest_waveformatex_ptr)
            }
            _ => Ok(false),
        }
    }
}

pub fn format_to_waveformatextensible(format: &Format) -> Option<mmreg::WAVEFORMATEXTENSIBLE> {
    let format_tag = match format.data_type {
        SampleFormat::I16 => MmregFormat::PCM,
        SampleFormat::F32 => MmregFormat::EXTENSIBLE,
    };
    let channels = format.channels as WORD;
    let sample_rate = format.sample_rate.0 as DWORD;
    let sample_bytes = format.data_type.sample_size() as WORD;
    let avg_bytes_per_sec = channels as DWORD * sample_rate * sample_bytes as DWORD;
    let block_align = channels * sample_bytes;
    let bits_per_sample = 8 * sample_bytes;
    let cb_size = match format_tag {
        MmregFormat::PCM => 0,
        MmregFormat::EXTENSIBLE => {
            let extensible_size = mem::size_of::<mmreg::WAVEFORMATEXTENSIBLE>();
            let ex_size = mem::size_of::<mmreg::WAVEFORMATEX>();
            (extensible_size - ex_size) as WORD
        }
    };
    let waveformatex = mmreg::WAVEFORMATEX {
        wFormatTag: format_tag.as_word(),
        nChannels: channels,
        nSamplesPerSec: sample_rate,
        nAvgBytesPerSec: avg_bytes_per_sec,
        nBlockAlign: block_align,
        wBitsPerSample: bits_per_sample,
        cbSize: cb_size,
    };

    const KSAUDIO_SPEAKER_DIRECTOUT: DWORD = 0;
    let channel_mask = KSAUDIO_SPEAKER_DIRECTOUT;

    let sub_format = match format_tag {
        MmregFormat::PCM => ksmedia::KSDATAFORMAT_SUBTYPE_PCM,
        MmregFormat::EXTENSIBLE => ksmedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    };
    let waveformatextensible = mmreg::WAVEFORMATEXTENSIBLE {
        Format: waveformatex,
        Samples: bits_per_sample as WORD,
        dwChannelMask: channel_mask,
        SubFormat: sub_format,
    };
    Some(waveformatextensible)
}

pub unsafe fn format_from_waveformatex_ptr(waveformatex_ptr: *const mmreg::WAVEFORMATEX) -> Option<Format> {
    fn cmp_guid(a: &guiddef::GUID, b: &guiddef::GUID) -> bool {
        a.Data1 == b.Data1
            && a.Data2 == b.Data2
            && a.Data3 == b.Data3
            && a.Data4 == b.Data4
    }
    let data_type = match ((*waveformatex_ptr).wBitsPerSample, (*waveformatex_ptr).wFormatTag) {
        (16, mmreg::WAVE_FORMAT_PCM) => SampleFormat::I16,
        (32, mmreg::WAVE_FORMAT_IEEE_FLOAT) => SampleFormat::F32,
        (n_bits, mmreg::WAVE_FORMAT_EXTENSIBLE) => {
            let waveformatextensible_ptr = waveformatex_ptr as *const mmreg::WAVEFORMATEXTENSIBLE;
            let sub = (*waveformatextensible_ptr).SubFormat;
            if n_bits == 16 && cmp_guid(&sub, &ksmedia::KSDATAFORMAT_SUBTYPE_PCM) {
                SampleFormat::I16
            } else if n_bits == 32 && cmp_guid(&sub, &ksmedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) {
                SampleFormat::F32
            } else {
                return None;
            }
        }
        _ => return None,
    };
    let format = Format {
        channels: (*waveformatex_ptr).nChannels as _,
        sample_rate: SampleRate((*waveformatex_ptr).nSamplesPerSec),
        data_type,
    };
    Some(format)
}

pub struct WaveFormatExPtr(pub *mut mmreg::WAVEFORMATEX);

impl Drop for WaveFormatExPtr {
    fn drop(&mut self) {
        unsafe {
            combaseapi::CoTaskMemFree(self.0 as *mut _);
        }
    }
}

enum MmregFormat {
    PCM,
    EXTENSIBLE,
}

impl MmregFormat {
    fn as_word(&self) -> WORD {
        match self {
            MmregFormat::PCM => mmreg::WAVE_FORMAT_PCM,
            MmregFormat::EXTENSIBLE => mmreg::WAVE_FORMAT_EXTENSIBLE
        }
    }
}