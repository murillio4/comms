use std::{fmt, io};
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc};

use mio::{Poll, Waker};
use serde::Serialize;

use voice_lib::network::Protocol;
use voice_lib::packet::{Hello, PacketBody};
use voice_lib::serialization::{OwnedHeaderParsedPacket, SerializeError, WritePacketBuilder};

use crate::audio::ipc::AudioCommand;
use crate::network::{WorkerInit, NextPollToken};
use crate::network::future::{RequestError, SendResponse};
#[cfg(windows)]
use winapi::um::winnt;

#[derive(Debug)]
pub enum NetworkEvent {
    SocketClosed,
    NetworkError,
    Packet(OwnedHeaderParsedPacket),
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
        outgoing: WritePacketBuilder,
        send_response: Option<SendResponse<OwnedHeaderParsedPacket>>,
    },
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
        match &self {
            &Self::Packet { protocol, outgoing, .. } => f.write_str(&format!("{:?} Packet of kind {}", protocol, outgoing.kind)),
            &Self::Connect { .. } => f.write_str("Connect"),
            &Self::Disconnect { .. } => f.write_str("Disconnect"),
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
        audio_sender: Sender<AudioCommand>,
        #[cfg(windows)]
        _winnt_handle: winnt::HANDLE
    ) -> (Self, futures::channel::mpsc::Receiver<NetworkEvent>, WorkerInit) {
        let (event_sender, network_event_receiver) = futures::channel::mpsc::channel(32);
        let (command_sender, command_receiver) = mpsc::channel();

        let mut next_poll_token = NextPollToken::new();
        let poll = Poll::new().unwrap();

        let command_token = next_poll_token.create();
        let waker = Waker::new(poll.registry(), command_token).unwrap();

        return (
            NetworkHandle {
                sender: command_sender,
                waker: Arc::new(waker),
            },
            network_event_receiver,
            WorkerInit {
                audio_sender,
                event_sender,
                command_receiver,
                command_token,
                next_poll_token,
                poll,
                // TODO #22
                // #[cfg(windows)]
                // winnt_handle
            }
        );
    }

    pub fn command(&self, command: NetworkCommand) {
        self.sender.send(command).unwrap();
        self.waker.wake().unwrap();
    }

    pub fn audio<B>(&self, packet: B) -> Result<(), SerializeError>
        where B: PacketBody + Serialize
    {
        self.command(NetworkCommand::Packet {
            protocol: Protocol::Udp,
            outgoing: WritePacketBuilder::new(None, &packet)?,
            send_response: None,
        });
        Ok(())
    }
}


pub struct CommandQueue {
    command_queue: Receiver<NetworkCommand>,
}

impl CommandQueue {
    pub(crate) fn new(command_queue: Receiver<NetworkCommand>) -> CommandQueue {
        CommandQueue {
            command_queue,
        }
    }

    pub(crate) fn next(&self) -> Option<NetworkCommand> {
        self.command_queue.try_recv().ok()
    }
}