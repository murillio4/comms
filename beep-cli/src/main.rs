use std::io::{Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};
use std::{env, io};
use voice_lib::packet::{
    AssociateUdp, AssociateUdpAccepted, AudioFromClient, FinishConnect, FinishConnectAccepted,
    Hello, Packet, PacketBody,
};
use voice_lib::serialization::{IncomingPacketFrame, OutgoingPacketFrame, SerializeError};

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

fn main() -> Result<(), BeepClientErr> {
    #[cfg(windows)]
    voice_lib::winlib::bump_priority();
    let mut serializer = BeepPacketSerializer::new();

    let mut args = env::args().take(3).skip(1);
    let server_address = args.next();
    let client_name = args.next();

    let server_address = match server_address {
        Some(server_address) => server_address,
        None => {
            eprintln!("Usage: beep-cli server_address [client_name]");
            return Err(BeepClientErr::Usage);
        }
    };

    let client_name = client_name.unwrap_or_else(|| "beep".to_string());

    let server_address_with_port = format!("{}:18847", server_address);
    println!("Connecting to {}...", server_address_with_port);

    let mut tcp = TcpStream::connect(server_address_with_port.clone())?;
    let udp = UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(server_address_with_port)?;

    let parsed_packet = read_next_tcp(&mut tcp)?;
    assert_eq!(parsed_packet.header.kind, Hello::KIND_ID);
    let hello: Packet<Hello> = parsed_packet.finish_parse()?;
    dbg!(&hello);

    udp.send(&serializer.serialize(&AssociateUdp {
        client_id: hello.body.client_id,
    })?)?;

    let parsed_packet = read_next_udp(&udp)?;
    assert_eq!(parsed_packet.header.kind, AssociateUdpAccepted::KIND_ID);
    let accepted: Packet<AssociateUdpAccepted> = parsed_packet.finish_parse()?;
    dbg!(&accepted);

    tcp.write_all(&serializer.serialize(&FinishConnect { name: client_name })?)?;

    let parsed_packet = read_next_tcp(&mut tcp)?;
    assert_eq!(parsed_packet.header.kind, FinishConnectAccepted::KIND_ID);
    let finish_connect_accepted: Packet<FinishConnectAccepted> = parsed_packet.finish_parse()?;
    dbg!(&finish_connect_accepted);

    let sample_rate = 48000f32;
    let mut sample_clock = 0f32;

    // Produce a sinusoid
    let mut next_value = || {
        sample_clock = (sample_clock + 1.0) % sample_rate;
        ((sample_clock * 440.0 * 2.0 * std::f32::consts::PI / sample_rate).sin() * 0.2) as f32
    };

    let start = SystemTime::now();
    let mut sample_count: u128 = 0;

    loop {
        std::thread::sleep(Duration::from_millis(3));
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

            let bytes = chunk
                .iter()
                .flat_map(|sample| sample.to_le_bytes().to_vec())
                .collect();
            udp.send(&serializer.serialize(&AudioFromClient(bytes))?)?;
        }
    }
}

#[derive(Debug)]
enum BeepClientErr {
    Usage,
    Io(io::Error),
    Serialize,
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
