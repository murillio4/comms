use crate::audio_connection::{AudioConnection, DeviceInfo, FoundDevices};
use crate::buffer::{InterruptReceiver, MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use crate::wasapi::default_device_change_notifier::{
    CommsImmNotificationClient, DefaultDeviceChangeHandle,
};
use crate::wasapi::device::{BuildStreamError, DefaultFormatError, InputDevice, OutputDevice};
use crate::wasapi::event_loop::{WasapiEventLoopBuilder, WasapiEventLoopRegistration};
use crate::wasapi::stream::{WasapiMicStream, WasapiPlaybackStream};

pub struct WasapiParameters;

pub struct WasapiAudioConnection<M: MicSink, P: PlaybackSource, SP: MaybeSharedPlaybackSource<P>> {
    pub mic_stream: Option<WasapiMicStream<M>>,
    pub playback_stream: Option<WasapiPlaybackStream<P>>,
    pub on_default_device_change: DefaultDeviceChangeHandle,
    pub mic_sink: M,
    pub playback_source: SP,
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
        let on_default_device_change =
            CommsImmNotificationClient::listen_for_default_device_changes();

        let mic_stream = InputDevice::default()
            .map(|device| {
                let mic_stream = device.into_stream()?;
                mic_stream.raw_stream.start();
                Ok(mic_stream)
            })
            .transpose()?;

        let playback_stream = OutputDevice::default()
            .map(|device| {
                let playback_stream = device.into_stream()?;
                playback_stream.raw_stream.start();
                Ok(playback_stream)
            })
            .transpose()?;

        Ok(WasapiEventLoopBuilder::new(Self {
            mic_stream,
            playback_stream,
            on_default_device_change,
            mic_sink,
            playback_source,
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
