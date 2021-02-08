use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc};
use std::{fmt, io};

use mio::{Poll, Waker};

use voice_lib::network::{CommsNetworkError, Protocol};
use voice_lib::serialization::{IncomingPacketFrame, OutgoingPacketFrame};

use crate::audio::ipc::AudioHandle;
use crate::network::future::{RequestError, SendResponse};
use crate::network::{NextPollToken, WorkerInit};
use voice_lib::packet::Hello;
use voice_lib::OpusAudioFrame;

#[derive(Debug)]
pub enum NetworkEvent {
    SocketClosed,
    NetworkError,
    Packet(IncomingPacketFrame),
}

#[derive(Debug)]
pub enum ConnectError {
    DestinationUnusable,
    NetworkError,
    AlreadyConnected,
    RequestError(RequestError),
}

impl From<io::Error> for ConnectError {
    fn from(_: io::Error) -> Self {
        ConnectError::NetworkError
    }
}

impl From<RequestError> for ConnectError {
    fn from(e: RequestError) -> Self {
        ConnectError::RequestError(e)
    }
}

pub enum NetworkCommand {
    Packet {
        protocol: Protocol,
        packet: OutgoingPacketFrame,
        send_response: Option<SendResponse<IncomingPacketFrame>>,
    },
    SendAudio(OpusAudioFrame),
    Connect {
        destination: SocketAddr,
        send_response: SendResponse<Result<Hello, ConnectError>>,
    },
    Disconnect {
        send_response: SendResponse<()>,
    },
}

impl Display for NetworkCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Self::Packet {
                protocol,
                packet: outgoing,
                ..
            } => f.write_str(&format!("{:?} Packet of kind {}", protocol, outgoing.kind)),
            Self::Connect { .. } => f.write_str("Connect"),
            Self::Disconnect { .. } => f.write_str("Disconnect"),
            Self::SendAudio { .. } => f.write_str("SendAudio"),
        }
    }
}

#[derive(Clone)]
pub struct NetworkHandle {
    sender: Sender<NetworkCommand>,
    waker: Arc<Waker>,
}

impl NetworkHandle {
    pub fn new_handle_and_worker(
        audio_handle: AudioHandle,
    ) -> (
        Self,
        futures::channel::mpsc::Receiver<NetworkEvent>,
        WorkerInit,
    ) {
        let (event_sender, network_event_receiver) = futures::channel::mpsc::channel(32);
        let (command_sender, command_receiver) = mpsc::channel();

        let mut next_poll_token = NextPollToken::new();
        let poll = Poll::new().unwrap();

        let command_token = next_poll_token.create();
        let waker = Waker::new(poll.registry(), command_token).unwrap();

        (
            NetworkHandle {
                sender: command_sender,
                waker: Arc::new(waker),
            },
            network_event_receiver,
            WorkerInit {
                audio_handle,
                event_sender,
                command_receiver,
                command_token,
                next_poll_token,
                poll,
            },
        )
    }

    pub fn command(&self, command: NetworkCommand) {
        self.sender.send(command).unwrap();
        self.waker.wake().unwrap();
    }

    pub fn audio(&self, frame: OpusAudioFrame) -> Result<(), CommsNetworkError> {
        self.command(NetworkCommand::SendAudio(frame));
        Ok(())
    }
}

pub struct CommandQueue {
    command_queue: Receiver<NetworkCommand>,
}

impl CommandQueue {
    pub(crate) fn new(command_queue: Receiver<NetworkCommand>) -> CommandQueue {
        CommandQueue { command_queue }
    }

    pub(crate) fn next(&self) -> Option<NetworkCommand> {
        self.command_queue.try_recv().ok()
    }
}
