use crate::audio_connection::{AudioConnection, DeviceInfo, FoundDevices};
use crate::buffer::{InterruptReceiver, MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use crate::wasapi::device::{BuildStreamError, DefaultFormatError, InputDevice, OutputDevice};
use crate::wasapi::event_loop::{WasapiEventLoopBuilder, WasapiEventLoopRegistration};
use crate::wasapi::sample_rate_conversion::{
    create_mic_sample_rate_converter, create_playback_sample_rate_converter,
};
use crate::wasapi::stream::{RawWasapiStream, WasapiMicStream, WasapiPlaybackStream};

pub struct WasapiParameters;

pub struct WasapiAudioConnection<M: MicSink, P: PlaybackSource, SP: MaybeSharedPlaybackSource<P>> {
    pub mic_stream: Option<WasapiMicStream<M>>,
    pub playback_stream: Option<WasapiPlaybackStream<P, SP>>,
}

#[derive(Debug)]
pub enum WasapiCreationError {
    NoInputFormat(DefaultFormatError),
    NoOutputFormat(DefaultFormatError),
    InputInitializationFailure(BuildStreamError),
    OutputInitializationFailure(BuildStreamError),
}

impl<M: MicSink, P: PlaybackSource, SP: MaybeSharedPlaybackSource<P>>
    WasapiAudioConnection<M, P, SP>
{
    pub fn create<I: InterruptReceiver<WasapiEventLoopRegistration>>(
        mic_sink: M,
        playback_source: SP,
    ) -> Result<WasapiEventLoopBuilder<M, P, SP, I>, WasapiCreationError> {
        let mic_stream = InputDevice::default()
            .map(|device| device.default_format().map(|f| (device, f)))
            .transpose()
            .map_err(WasapiCreationError::NoInputFormat)?
            .map(|(device, format)| {
                let mic_converter = create_mic_sample_rate_converter(format.sample_rate.0, 48000);

                let waveformatex = match device.audio_client.initialize(&format) {
                    Ok(w) => w,
                    Err(e) => return Err(WasapiCreationError::InputInitializationFailure(e)),
                };

                let mic_stream = WasapiMicStream::new(
                    mic_converter,
                    device.create_client().unwrap(),
                    device.audio_client.create_notify().unwrap(),
                    RawWasapiStream::new(waveformatex, device.audio_client, format),
                    mic_sink,
                );
                mic_stream.raw_stream.start();
                Ok(mic_stream)
            })
            .transpose()?;

        let playback_stream = OutputDevice::default()
            .map(|device| device.default_format().map(|f| (device, f)))
            .transpose()
            .map_err(WasapiCreationError::NoOutputFormat)?
            .map(|(device, format)| {
                let playback_converter =
                    create_playback_sample_rate_converter(48000, format.sample_rate.0);
                let waveformatex = match device.audio_client.initialize(&format) {
                    Ok(w) => w,
                    Err(e) => return Err(WasapiCreationError::OutputInitializationFailure(e)),
                };
                let playback_stream = WasapiPlaybackStream::new(
                    playback_converter,
                    device.create_client().unwrap(),
                    device.audio_client.create_notify().unwrap(),
                    RawWasapiStream::new(waveformatex, device.audio_client, format),
                    playback_source,
                );
                playback_stream.raw_stream.start();
                Ok(playback_stream)
            })
            .transpose()?;

        Ok(WasapiEventLoopBuilder::new(Self {
            mic_stream,
            playback_stream,
        }))
    }
}

impl<M: MicSink, P: PlaybackSource, SP: MaybeSharedPlaybackSource<P>> AudioConnection
    for WasapiAudioConnection<M, P, SP>
{
    fn list_devices(&self, callback: impl FnOnce(FoundDevices)) {
        callback(FoundDevices {
            input_devices: InputDevice::get_list()
                .into_iter()
                .map(|device| DeviceInfo {
                    label: device
                        .name()
                        .unwrap_or_else(|e| panic!("{}", e.description)),
                    id: device.id(),
                })
                .collect(),
            output_devices: OutputDevice::get_list()
                .into_iter()
                .map(|device| DeviceInfo {
                    label: device.name().unwrap(),
                    id: device.id(),
                })
                .collect(),
        });
    }
}
