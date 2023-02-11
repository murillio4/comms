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
    pub mic_stream: WasapiMicStream<M>,
    pub playback_stream: WasapiPlaybackStream<P, SP>,
}

#[derive(Debug)]
pub enum WasapiCreationError {
    NoInputDevice,
    NoOutputDevice,
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
        let input_device = match InputDevice::default() {
            Some(d) => d,
            None => return Err(WasapiCreationError::NoInputDevice),
        };
        let output_device = match OutputDevice::default() {
            Some(d) => d,
            None => return Err(WasapiCreationError::NoOutputDevice),
        };

        let input_format = match input_device.default_format() {
            Ok(f) => f,
            Err(e) => return Err(WasapiCreationError::NoInputFormat(e)),
        };
        let output_format = match output_device.default_format() {
            Ok(f) => f,
            Err(e) => return Err(WasapiCreationError::NoOutputFormat(e)),
        };

        let mic_converter = create_mic_sample_rate_converter(input_format.sample_rate.0, 48000);

        let waveformatex = match input_device.audio_client.initialize(&input_format) {
            Ok(w) => w,
            Err(e) => return Err(WasapiCreationError::InputInitializationFailure(e)),
        };

        let mic_stream = WasapiMicStream::new(
            mic_converter,
            input_device.create_client().unwrap(),
            input_device.audio_client.create_notify().unwrap(),
            RawWasapiStream::new(waveformatex, input_device.audio_client, input_format),
            mic_sink,
        );
        mic_stream.raw_stream.start();

        let playback_converter =
            create_playback_sample_rate_converter(48000, output_format.sample_rate.0);
        let waveformatex = match output_device.audio_client.initialize(&output_format) {
            Ok(w) => w,
            Err(e) => return Err(WasapiCreationError::OutputInitializationFailure(e)),
        };
        let playback_stream = WasapiPlaybackStream::new(
            playback_converter,
            output_device.create_client().unwrap(),
            output_device.audio_client.create_notify().unwrap(),
            RawWasapiStream::new(waveformatex, output_device.audio_client, output_format),
            playback_source,
        );
        playback_stream.raw_stream.start();
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
