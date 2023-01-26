use audiopus::coder::Encoder;
use audiopus::{Application, Channels, SampleRate};
use comms_lib::packet::{
    AssociateUdp, AssociateUdpAccepted, AudioFromClient, AudioIndex, FinishConnect,
    FinishConnectAccepted, Hello, Packet, PacketBody,
};
use comms_lib::serialization::{IncomingPacketFrame, OutgoingPacketFrame, SerializeError};
use std::io::{Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::num::NonZeroU64;
use std::thread::sleep;
use std::time::{Duration, SystemTime};
use std::{env, io};
use uuid::Uuid;

fn read_next_tcp(tcp: &mut TcpStream) -> Result<IncomingPacketFrame, SerializeError> {
    let mut buffer = [0u8; 65535];
    let size = tcp.read(&mut buffer).unwrap();
    IncomingPacketFrame::from_bytes(&buffer[..size])
}

fn read_next_udp(udp: &UdpSocket) -> Result<IncomingPacketFrame, SerializeError> {
    let mut buffer = [0u8; 65535];
    let size = udp.recv(&mut buffer).unwrap();
    IncomingPacketFrame::from_bytes(&buffer[..size])
}

struct BeepPacketSerializer {
    next_packet_id: u64,
}

impl BeepPacketSerializer {
    fn new() -> Self {
        Self { next_packet_id: 1 }
    }

    fn serialize<P>(&mut self, packet: &P) -> Result<Vec<u8>, BeepClientErr>
    where
        P: PacketBody,
    {
        let packet_id = self.next_packet_id;
        self.next_packet_id += 1;
        OutgoingPacketFrame::from_packet(None, packet)
            .map_err(|e| e.into())
            .and_then(|p| {
                p.into_raw(NonZeroU64::new(packet_id).unwrap())
                    .map_err(|e| e.into())
            })
    }
}

fn parse_arguments(
    args: impl Iterator<Item = String>,
) -> Result<(Option<String>, Option<String>, bool), BeepClientErr> {
    let mut server_address = None;
    let mut client_name = None;
    let mut use_interactive_mode = false;

    for argument in args {
        // Handle optional flags
        if !use_interactive_mode && argument.to_lowercase() == "--interactive" {
            use_interactive_mode = true
        }
        // Handle positional arguments
        else if !argument.starts_with("--") && server_address.is_none() {
            server_address = Some(argument)
        } else if !argument.starts_with("--") && client_name.is_none() {
            client_name = Some(argument)
        } else {
            eprintln!("Usage: beep-cli server_address [client_name] [--interactive]");
            return Err(BeepClientErr::Usage);
        }
    }

    Ok((server_address, client_name, use_interactive_mode))
}

fn main() -> Result<(), BeepClientErr> {
    let mut serializer = BeepPacketSerializer::new();

    let (server_address, client_name, use_interactive_mode) = parse_arguments(env::args().skip(1))?;

    let server_address = match server_address {
        Some(server_address) => server_address,
        None => {
            eprintln!("Usage: beep-cli server_address [client_name] [--interactive]");
            return Err(BeepClientErr::Usage);
        }
    };

    let client_name = client_name.unwrap_or_else(|| "beep".to_string());

    let server_address_with_port = format!("{server_address}:18847");
    println!("Connecting to {server_address_with_port}...");

    let mut tcp = TcpStream::connect(server_address_with_port.clone())?;
    let udp = UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(server_address_with_port)?;

    let parsed_packet = read_next_tcp(&mut tcp)?;
    assert_eq!(parsed_packet.header.kind, Hello::KIND_ID);
    let hello: Packet<Hello> = parsed_packet.finish_parse()?;
    dbg!(&hello);

    udp.send(&serializer.serialize(&AssociateUdp {
        client_id: hello.body.client_id,
        local_udp_addr: udp.local_addr().unwrap(),
    })?)?;

    let parsed_packet = read_next_udp(&udp)?;
    assert_eq!(parsed_packet.header.kind, AssociateUdpAccepted::KIND_ID);
    let accepted: Packet<AssociateUdpAccepted> = parsed_packet.finish_parse()?;
    dbg!(&accepted);

    tcp.write_all(&serializer.serialize(&FinishConnect {
        name: client_name,
        user_id: Uuid::default(),
    })?)?;

    let parsed_packet = read_next_tcp(&mut tcp)?;
    assert_eq!(parsed_packet.header.kind, FinishConnectAccepted::KIND_ID);
    let finish_connect_accepted: Packet<FinishConnectAccepted> = parsed_packet.finish_parse()?;
    dbg!(&finish_connect_accepted);

    let sample_rate = 48000f32;
    let mut sample_clock = 0f32;

    // Produce a sinusoid
    let mut next_value = || {
        sample_clock = (sample_clock + 1.0) % sample_rate;
        (sample_clock * 440.0 * 2.0 * std::f32::consts::PI / sample_rate).sin() * 0.2
    };

    let start = SystemTime::now();
    let mut sample_count: u128 = 0;

    let encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::LowDelay)?;
    let mut audio_id = AudioIndex::new(1).unwrap();

    if use_interactive_mode {
        loop {
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let start = SystemTime::now();
            sample_count = 0;

            // One second is 48000 samples, hence we send 480 * 50 = 24000 samples (500ms) for one "press"
            for _ in 0..50 {
                let now = SystemTime::now();
                let diff = now.duration_since(start).unwrap();
                let ms = diff.as_millis();
                let samples_since_start_count = ms * 48;

                while samples_since_start_count - sample_count >= 480 {
                    sample_count += 480;
                    let mut chunk = [0f32; 480];
                    for sample in chunk.iter_mut() {
                        *sample = next_value()
                    }

                    let mut opus_buffer = [0u8; 2048];
                    let size = encoder.encode_float(&chunk, &mut opus_buffer)?;
                    let opus_bytes = opus_buffer[..size].to_vec();
                    udp.send(&serializer.serialize(&AudioFromClient {
                        index: audio_id,
                        bytes: opus_bytes,
                    })?)?;
                    audio_id = AudioIndex::new(audio_id.get() + 1).unwrap();
                }

                sleep(Duration::from_millis(3));
            }
        }
    } else {
        loop {
            sleep(Duration::from_millis(3));
            let now = SystemTime::now();
            let diff = now.duration_since(start).unwrap();
            let ms = diff.as_millis();
            let samples_since_start_count = ms * 48;

            while samples_since_start_count - sample_count >= 480 {
                sample_count += 480;
                let mut chunk = [0f32; 480];
                for sample in chunk.iter_mut() {
                    *sample = next_value()
                }

                let mut opus_buffer = [0u8; 2048];
                let size = encoder.encode_float(&chunk, &mut opus_buffer)?;
                let opus_bytes = opus_buffer[..size].to_vec();
                udp.send(&serializer.serialize(&AudioFromClient {
                    index: audio_id,
                    bytes: opus_bytes,
                })?)?;
                audio_id = AudioIndex::new(audio_id.get() + 1).unwrap();
            }
        }
    }
}

#[derive(Debug)]
enum BeepClientErr {
    Usage,
    Io(io::Error),
    Serialize,
    Opus,
}

impl From<io::Error> for BeepClientErr {
    fn from(e: io::Error) -> Self {
        BeepClientErr::Io(e)
    }
}

impl From<SerializeError> for BeepClientErr {
    fn from(_: SerializeError) -> Self {
        BeepClientErr::Serialize
    }
}

impl From<audiopus::Error> for BeepClientErr {
    fn from(_: audiopus::Error) -> Self {
        BeepClientErr::Opus
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ip_only() {
        let test_string = [String::from("127.0.0.1")];
        assert_eq!(
            parse_arguments(test_string.into_iter()).unwrap(),
            (Some("127.0.0.1".to_string()), None, false)
        );
    }

    #[test]
    fn test_parse_interactive_enabled() {
        let test_string = [String::from("127.0.0.1"), String::from("--interactive")];
        assert_eq!(
            parse_arguments(test_string.into_iter()).unwrap(),
            (Some("127.0.0.1".to_string()), None, true)
        );
    }

    #[test]
    fn test_parse_interactive_enabled_badly_spelled() {
        let test_string = [String::from("127.0.0.1"), String::from("---interactive")];
        assert!(parse_arguments(test_string.into_iter()).is_err());
    }

    #[test]
    fn test_parse_client_name_and_interactive_enabled() {
        let test_string = [
            String::from("127.0.0.1"),
            String::from("Bob"),
            String::from("--interactive"),
        ];
        assert_eq!(
            parse_arguments(test_string.into_iter()).unwrap(),
            (Some("127.0.0.1".to_string()), Some("Bob".to_string()), true)
        );
    }

    #[test]
    fn test_parse_double_client_name() {
        let test_string = [
            String::from("127.0.0.1"),
            String::from("Bob"),
            String::from("Mitch"),
        ];
        assert!(parse_arguments(test_string.into_iter()).is_err());
    }

    #[test]
    fn test_parse_bad_optional_argument() {
        let test_string = [
            String::from("127.0.0.1"),
            String::from("--icantspelloptionalargument"),
        ];
        assert!(parse_arguments(test_string.into_iter()).is_err());
    }

    #[test]
    fn test_parse_too_many_args() {
        let test_string = [
            String::from("127.0.0.1"),
            String::from("Bob"),
            String::from("--interactive"),
            String::from("argument"),
        ];
        assert!(parse_arguments(test_string.into_iter()).is_err());
    }

    #[test]
    fn test_parse_dumb_order() {
        let test_string = [String::from("--interactive"), String::from("127.0.0.1")];
        assert_eq!(
            parse_arguments(test_string.into_iter()).unwrap(),
            (Some("127.0.0.1".to_string()), None, true)
        );
    }
}
