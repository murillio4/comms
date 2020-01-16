use std::{iter, ptr};
use std::mem::MaybeUninit;
use std::slice;
use std::sync::atomic::{AtomicUsize, Ordering};

use winapi::shared::basetsd::UINT32;
use winapi::shared::minwindef::{DWORD, WORD};
use winapi::shared::minwindef;
use winapi::um::{audioclient, avrt, synchapi, winbase, winnt};
use winapi::um::audioclient::{AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_S_BUFFER_EMPTY};

use crate::audio::wasapi::com::check_result;
use crate::audio::wasapi::device::{InputDevice, OutputDevice};
use crate::audio::wasapi::stream::{AudioClientFlow, Format, InputBuffer, OutputBuffer, SampleFormat, StreamData, StreamDataResult, StreamError, StreamId, UnknownTypeInputBuffer, UnknownTypeOutputBuffer};

struct Stream {
    id: StreamId,
    audio_client: *mut audioclient::IAudioClient,
    client_flow: AudioClientFlow,
    handle: winnt::HANDLE,
    playing: bool,
    max_frames_in_buffer: UINT32,
    bytes_per_frame: WORD,
    sample_format: SampleFormat,
}

enum Command {
    NewStream(Stream),
    PlayStream(StreamId),
}

pub struct EventLoop {
    next_stream_id: AtomicUsize,
    streams: Vec<Stream>,
    commands: Vec<Command>,
    pending_commands_handle: winnt::HANDLE,
}

impl EventLoop {
    pub fn new() -> EventLoop {
//        unsafe {
//            let task_name = "Audio\0".to_string();
//            let mut task_index = 0 as DWORD;
//            let handle = avrt::AvSetMmThreadCharacteristicsA(task_name.as_ptr() as _, &mut task_index);
//            if handle.is_null() {
//                eprintln!("Unable to bump thread priority using mmcss");
//            }
//        }
        let pending_scheduled_event = unsafe { synchapi::CreateEventA(ptr::null_mut(), 0, 0, ptr::null()) };
        EventLoop {
            pending_commands_handle: pending_scheduled_event,
            next_stream_id: AtomicUsize::new(0),
            commands: Vec::new(),
            streams: Vec::new(),
        }
    }

    pub fn run<F>(&mut self, mut callback: F) where F: FnMut(StreamId, StreamDataResult) {
        let mut streams_to_remove: Vec<(StreamId, StreamError)> = vec![];

        'stream_loop: loop {
            for (stream_id, err) in streams_to_remove.drain(..) {
                match self.streams.iter().position(|s| s.id == stream_id) {
                    None => continue,
                    Some(p) => {
                        self.streams.remove(p);
                        callback(stream_id, Err(err));
                    }
                }
            }

            process_commands(&mut self.streams, &mut self.commands, &mut callback);

            let handles: Vec<winnt::HANDLE> = self.streams.iter()
                .map(|s| s.handle)
                .chain(iter::once(self.pending_commands_handle))
                .collect();
            let handle_index = match wait_for_handle_signal(&handles) {
                Ok(index) => index,
                Err(description) => {
                    for stream in &self.streams {
                        callback(stream.id.clone(), Err(StreamError::BackendSpecific(description.clone())))
                    }
                    self.streams.clear();
                    break 'stream_loop;
                }
            };

            if handle_index == handles.len() - 1 {
                // The handle is "pending_scheduled_event", there's pending commands to resolve
                continue;
            }

            let stream = &mut self.streams[handle_index];
            let sample_size = stream.sample_format.sample_size();

            match stream.client_flow {
                AudioClientFlow::Capture { capture_client } => {
                    let mut frames_available = 0;
                    let mut buffer = MaybeUninit::<*mut minwindef::BYTE>::uninit();
                    let mut flags = MaybeUninit::<_>::uninit();

                    loop {
                        let hresult = unsafe { (*capture_client).GetNextPacketSize(&mut frames_available) };
                        if let Err(err) = stream_error_from_hresult(hresult) {
                            streams_to_remove.push((stream.id.clone(), err));
                            break;
                        }

                        if frames_available == 0 {
                            break;
                        }

                        let hresult = unsafe {
                            (*capture_client).GetBuffer(
                                buffer.as_mut_ptr(),
                                &mut frames_available,
                                flags.as_mut_ptr(),
                                ptr::null_mut(),
                                ptr::null_mut(),
                            )
                        };

                        if hresult == AUDCLNT_S_BUFFER_EMPTY {
                            continue;
                        } else if let Err(err) = stream_error_from_hresult(hresult) {
                            streams_to_remove.push((stream.id.clone(), err));
                            continue;
                        }

                        let buffer = unsafe { buffer.assume_init() };
                        let buffer_len = frames_available as usize * stream.bytes_per_frame as usize / sample_size;

                        macro_rules! capture_callback {
                            ($T:ty, $Variant:ident) => {{
                                let buffer_data = buffer as *mut _ as *const $T;
                                let slice = unsafe { slice::from_raw_parts(buffer_data, buffer_len) };
                                let unknown_buffer = UnknownTypeInputBuffer::$Variant(InputBuffer {
                                    buffer: slice,
                                });
                                let data = StreamData::Input(unknown_buffer);
                                callback(stream.id.clone(), Ok(data));
                                let hresult = unsafe { (*capture_client).ReleaseBuffer(frames_available) };
                                if let Err(err) = stream_error_from_hresult(hresult) {
                                    streams_to_remove.push((stream.id.clone(), err));
                                    continue;
                                }
                            }};
                        }

                        match stream.sample_format {
                            SampleFormat::F32 => capture_callback!(f32, F32),
                            SampleFormat::I16 => capture_callback!(i16, I16),
                            SampleFormat::U16 => capture_callback!(u16, U16),
                        }
                    }
                }
                AudioClientFlow::Render { render_client } => {
                    let frames_available = match get_available_frames(stream) {
                        Ok(0) => continue,
                        Ok(n) => n,
                        Err(err) => {
                            streams_to_remove.push((stream.id.clone(), err));
                            continue;
                        }
                    };

                    let mut buffer = MaybeUninit::<*mut minwindef::BYTE>::uninit();
                    let hresult = unsafe {
                        (*render_client).GetBuffer(
                            frames_available,
                            buffer.as_mut_ptr(),
                        )
                    };

                    if let Err(err) = stream_error_from_hresult(hresult) {
                        streams_to_remove.push((stream.id.clone(), err));
                        continue;
                    }

                    let buffer = unsafe { buffer.assume_init() };
                    let buffer_len = frames_available as usize * stream.bytes_per_frame as usize / sample_size;

                    macro_rules! render_callback {
                        ($T:ty, $Variant:ident) => {{
                            let buffer_data = buffer as *mut $T;
                            let slice = unsafe { slice::from_raw_parts_mut(buffer_data, buffer_len) };
                            let unknown_buffer = UnknownTypeOutputBuffer::$Variant(OutputBuffer {
                                buffer: slice,
                            });
                            let data = StreamData::Output(unknown_buffer);
                            callback(stream.id.clone(), Ok(data));
                            let hresult = unsafe { (*render_client).ReleaseBuffer(frames_available as u32, 0) };
                            if let Err(err) = stream_error_from_hresult(hresult) {
                                streams_to_remove.push((stream.id.clone(), err));
                                continue;
                            }
                        }}
                    }

                    match stream.sample_format {
                        SampleFormat::F32 => render_callback!(f32, F32),
                        SampleFormat::I16 => render_callback!(i16, I16),
                        SampleFormat::U16 => render_callback!(u16, U16),
                    }
                }
            }
        }
    }

    fn push_command(&mut self, command: Command) {
        self.commands.push(command);
        unsafe {
            let result = synchapi::SetEvent(self.pending_commands_handle);
            assert_ne!(result, 0);
        }
    }

    pub fn input_stream(
        &mut self,
        device: &InputDevice,
        format: &Format,
    ) -> Result<StreamId, BuildStreamError> {
        let waveformatex = device.audio_client.get_waveformatex(format)?;
        let max_buffer_frames = device.audio_client.get_buffer_max_frames()?;
        let event = device.audio_client.create_event_handle()?;
        let capture_client = device.create_client()?;

        let client_flow = AudioClientFlow::Capture {
            capture_client,
        };

        self.new_stream(|stream_id| Stream {
            id: stream_id,
            audio_client: *device.audio_client,
            client_flow,
            handle: event,
            playing: false,
            max_frames_in_buffer: max_buffer_frames,
            bytes_per_frame: waveformatex.nBlockAlign,
            sample_format: format.data_type,
        })
    }

    pub fn output_stream(
        &mut self,
        device: &OutputDevice,
        format: &Format,
    ) -> Result<StreamId, BuildStreamError> {
        let waveformatex = device.audio_client.get_waveformatex(format)?;
        let max_buffer_frames = device.audio_client.get_buffer_max_frames()?;
        let event = device.audio_client.create_event_handle()?;
        let render_client = device.create_client()?;

        let new_stream_id = StreamId(self.next_stream_id.fetch_add(1, Ordering::Relaxed));
        if new_stream_id.0 == usize::max_value() {
            return Err(BuildStreamError::StreamIdOverflow);
        }

        let client_flow = AudioClientFlow::Render {
            render_client
        };

        self.new_stream(|stream_id| Stream {
            id: stream_id,
            audio_client: *device.audio_client,
            client_flow,
            handle: event,
            playing: false,
            max_frames_in_buffer: max_buffer_frames,
            bytes_per_frame: waveformatex.nBlockAlign,
            sample_format: format.data_type,
        })
    }

    fn new_stream<F>(
        &mut self,
        f: F,
    ) -> Result<StreamId, BuildStreamError> where F: FnOnce(StreamId) -> Stream {
        let new_stream_id = StreamId(self.next_stream_id.fetch_add(1, Ordering::Relaxed));
        if new_stream_id.0 == std::usize::MAX {
            return Err(BuildStreamError::StreamIdOverflow);
        }

        let inner = f(new_stream_id.clone());
        self.push_command(Command::NewStream(inner));
        self.push_command(Command::PlayStream(new_stream_id.clone()));

        Ok(new_stream_id)
    }
}

#[derive(Debug)]
pub enum BuildStreamError {
    DeviceNotAvailable,
    FormatNotSupported,
    StreamIdOverflow,
    BackendSpecific(String),
}

fn process_commands(
    streams: &mut Vec<Stream>,
    commands: &mut Vec<Command>,
    callback: &mut dyn FnMut(StreamId, StreamDataResult),
) {
    for command in commands.drain(..) {
        match command {
            Command::NewStream(stream_inner) => {
                streams.push(stream_inner);
            }
            Command::PlayStream(stream_id) => {
                match streams.iter().position(|s| s.id == stream_id) {
                    None => continue,
                    Some(p) => {
                        if streams[p].playing {
                            println!("PlayStream({:?}) command received, but stream is already playing", stream_id);
                        } else {
                            let hresult = unsafe {
                                (*streams[p].audio_client).Start()
                            };
                            match stream_error_from_hresult(hresult) {
                                Ok(()) => streams[p].playing = true,
                                Err(err) => {
                                    callback(stream_id, Err(err));
                                    streams.remove(p);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn wait_for_handle_signal(handles: &[winnt::HANDLE]) -> Result<usize, String> {
    debug_assert!(handles.len() <= winnt::MAXIMUM_WAIT_OBJECTS as usize);
    let result = unsafe {
        synchapi::WaitForMultipleObjectsEx(
            handles.len() as u32,
            handles.as_ptr(),
            minwindef::FALSE,
            winbase::INFINITE,
            minwindef::FALSE,
        )
    };
    if result == winbase::WAIT_FAILED {
        let err = unsafe {
            winapi::um::errhandlingapi::GetLastError()
        };
        let description = format!("WaitForMultipleObjectsEx failed: {}", err);
        return Err(description);
    }

    debug_assert!(result >= winbase::WAIT_OBJECT_0);
    let handle_idx = (result - winbase::WAIT_OBJECT_0) as usize;
    Ok(handle_idx)
}

fn get_available_frames(stream: &Stream) -> Result<u32, StreamError> {
    unsafe {
        let mut padding = MaybeUninit::<u32>::uninit();
        let hresult = (*stream.audio_client).GetCurrentPadding(padding.as_mut_ptr());
        stream_error_from_hresult(hresult)?;
        Ok(stream.max_frames_in_buffer - padding.assume_init())
    }
}

fn stream_error_from_hresult(hresult: winnt::HRESULT) -> Result<(), StreamError> {
    if hresult == AUDCLNT_E_DEVICE_INVALIDATED {
        return Err(StreamError::DeviceNotAvailable);
    } else if let Err(err) = check_result(hresult) {
        let description = format!("{}", err);
        Err(StreamError::BackendSpecific(description))
    } else {
        Ok(())
    }
}