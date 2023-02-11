use crate::audio_connection::{EventLoop, EventLoopNotify};
use crate::buffer::{InterruptReceiver, MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use crate::wasapi::audio_connection::WasapiAudioConnection;
use windows::Win32::Foundation;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::{SystemServices, Threading};

pub struct WasapiEventLoopRegistration {
    pub win_handle: HANDLE,
}

unsafe impl Send for WasapiEventLoopRegistration {}

#[derive(Clone)]
pub struct WasapiEventLoopNotify {
    pub win_handle: HANDLE,
}

unsafe impl Send for WasapiEventLoopNotify {}

impl EventLoopNotify for WasapiEventLoopNotify {
    fn set_ready(&self) {
        unsafe {
            Threading::SetEvent(self.win_handle).ok().unwrap();
        }
    }
}

pub fn event_loop_channel() -> (WasapiEventLoopRegistration, WasapiEventLoopNotify) {
    let win_handle = unsafe { Threading::CreateEventA(None, false, false, None) }.unwrap();
    (
        WasapiEventLoopRegistration { win_handle },
        WasapiEventLoopNotify { win_handle },
    )
}

#[derive(Clone)]
enum WasapiHandle {
    MicStream(HANDLE),
    PlaybackStream(HANDLE),
    Receiver(HANDLE),
}

impl WasapiHandle {
    fn get_win_handle(&self) -> HANDLE {
        match self {
            WasapiHandle::MicStream(handle) => *handle,
            WasapiHandle::PlaybackStream(handle) => *handle,
            WasapiHandle::Receiver(handle) => *handle,
        }
    }
}

trait SelectWasapiHandle {
    fn select(&mut self) -> Result<WasapiHandle, String>;
}

impl SelectWasapiHandle for Vec<WasapiHandle> {
    fn select(&mut self) -> Result<WasapiHandle, String> {
        let win_handles: Vec<HANDLE> = self.iter().map(|h| h.get_win_handle()).collect();
        let handle_index = match wait_for_handle_signal(&win_handles) {
            Ok(index) => index,
            Err(description) => return Err(description),
        };

        Ok(self.get(handle_index).cloned().unwrap())
    }
}

pub struct WasapiEventLoopBuilder<
    M: MicSink,
    P: PlaybackSource,
    SP: MaybeSharedPlaybackSource<P>,
    I: InterruptReceiver<WasapiEventLoopRegistration>,
> {
    connection: WasapiAudioConnection<M, P, SP>,
    receiver: Option<I>,
}

impl<
        M: MicSink,
        P: PlaybackSource,
        SP: MaybeSharedPlaybackSource<P>,
        I: InterruptReceiver<WasapiEventLoopRegistration>,
    > WasapiEventLoopBuilder<M, P, SP, I>
{
    pub fn new(connection: WasapiAudioConnection<M, P, SP>) -> Self {
        Self {
            connection,
            receiver: None,
        }
    }

    pub fn with_interrupt(mut self, receiver: I) -> Self {
        self.receiver = Some(receiver);
        self
    }

    pub fn create_event_loop(self) -> WasapiEventLoop<M, P, SP, I> {
        WasapiEventLoop {
            connection: self.connection,
            receiver: self.receiver,
        }
    }
}

pub struct WasapiEventLoop<
    M: MicSink,
    P: PlaybackSource,
    SP: MaybeSharedPlaybackSource<P>,
    I: InterruptReceiver<WasapiEventLoopRegistration>,
> {
    connection: WasapiAudioConnection<M, P, SP>,
    receiver: Option<I>,
}

impl<
        M: MicSink,
        P: PlaybackSource,
        SP: MaybeSharedPlaybackSource<P>,
        I: InterruptReceiver<WasapiEventLoopRegistration>,
    > EventLoop for WasapiEventLoop<M, P, SP, I>
{
    type Registration = WasapiEventLoopRegistration;
    type Notify = WasapiEventLoopNotify;
    type AudioConnection = WasapiAudioConnection<M, P, SP>;

    fn run(mut self) -> ! {
        let mut handles: Vec<WasapiHandle> = vec![
            WasapiHandle::MicStream(self.connection.mic_stream.notify.win_handle),
            WasapiHandle::PlaybackStream(self.connection.playback_stream.notify.win_handle),
        ];
        if let Some(receiver) = &self.receiver {
            handles.push(WasapiHandle::Receiver(receiver.registration().win_handle));
        }

        loop {
            match handles.select() {
                Err(e) => {
                    unimplemented!("Failed to select WASAPI handle: {:?}", e)
                }
                Ok(WasapiHandle::MicStream(_)) => {
                    if let Err(e) = self
                        .connection
                        .mic_stream
                        .invoke(&mut self.connection.mic_sink)
                    {
                        unimplemented!("Mic error: {:?}", e);
                    }
                }
                Ok(WasapiHandle::PlaybackStream(_)) => {
                    let source = &mut self.connection.playback_source;
                    let stream = &mut self.connection.playback_stream;
                    source.with(|playback| {
                        if let Err(e) = stream.invoke(playback) {
                            unimplemented!("Playback error: {:?}", e);
                        }
                    });
                }
                Ok(WasapiHandle::Receiver(_)) => {
                    if let Some(receiver) = &mut self.receiver {
                        receiver.invoke(&self.connection);
                    }
                }
            };
        }
    }
}

fn wait_for_handle_signal(handles: &[HANDLE]) -> Result<usize, String> {
    debug_assert!(handles.len() <= SystemServices::MAXIMUM_WAIT_OBJECTS as usize);
    let result = unsafe { Threading::WaitForMultipleObjectsEx(handles, false, u32::MAX, false) };
    if result == Foundation::WAIT_FAILED {
        let err = unsafe { Foundation::GetLastError() };
        let description = format!("WaitForMultipleObjectsEx failed: {err:?}");
        return Err(description);
    }

    let handle_idx = (result.0 - Foundation::WAIT_OBJECT_0.0) as usize;
    Ok(handle_idx)
}
