use crate::audio_connection::{AudioConnection, DeviceInfo, FoundDevices};
use crate::buffer::{InterruptReceiver, MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use crate::wasapi::device::{InputDevice, OutputDevice};
use crate::wasapi::event_loop::{WasapiEventLoopBuilder, WasapiEventLoopRegistration};
use crate::wasapi::sample_rate_conversion::{
    create_mic_sample_rate_converter, create_playback_sample_rate_converter,
};
use crate::wasapi::stream::{RawWasapiStream, WasapiMicStream, WasapiPlaybackStream};

pub struct WasapiParameters;

pub struct WasapiAudioConnection<M: MicSink, P: PlaybackSource, SP: MaybeSharedPlaybackSource<P>> {
    pub mic_stream: WasapiMicStream<M>,
    pub playback_stream: WasapiPlaybackStream<P>,
    pub playback_source: SP,
    pub mic_sink: M,
}

impl<M: MicSink, P: PlaybackSource, SP: MaybeSharedPlaybackSource<P>>
    WasapiAudioConnection<M, P, SP>
{
    pub fn create<I: InterruptReceiver<WasapiEventLoopRegistration>>(
        mic_sink: M,
        playback_source: SP,
    ) -> WasapiEventLoopBuilder<M, P, SP, I> {
        let input_device = InputDevice::default().unwrap();
        let output_device = OutputDevice::default().unwrap();

        let input_format = input_device.default_format().unwrap();
        let output_format = output_device.default_format().unwrap();

        let mic_converter = create_mic_sample_rate_converter(input_format.sample_rate.0, 48000);
        let waveformatex = input_device.audio_client.initialize(&input_format).unwrap();
        let mic_stream = WasapiMicStream::new(
            mic_converter,
            input_device.create_client().unwrap(),
            input_device.audio_client.create_notify().unwrap(),
            RawWasapiStream::new(waveformatex, input_device.audio_client, input_format),
        );
        mic_stream.raw_stream.start();

        let playback_converter =
            create_playback_sample_rate_converter(48000, output_format.sample_rate.0);
        let waveformatex = output_device
            .audio_client
            .initialize(&output_format)
            .unwrap();
        let playback_stream = WasapiPlaybackStream::new(
            playback_converter,
            output_device.create_client().unwrap(),
            output_device.audio_client.create_notify().unwrap(),
            RawWasapiStream::new(waveformatex, output_device.audio_client, output_format),
        );
        playback_stream.raw_stream.start();
        WasapiEventLoopBuilder::new(Self {
            mic_sink,
            mic_stream,
            playback_stream,
            playback_source,
        })
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
                    label: device.name().unwrap(),
                    id: device.id().unwrap(),
                })
                .collect(),
            output_devices: OutputDevice::get_list()
                .into_iter()
                .map(|device| DeviceInfo {
                    label: device.name().unwrap(),
                    id: device.id().unwrap(),
                })
                .collect(),
        });
    }

    fn change_input_device(_: String) {
        eprintln!("can't change input device yet")
    }

    fn change_output_device(_: String) {
        eprintln!("can't change output device yet")
    }
}
