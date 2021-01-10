use clap::{App, Arg};
use std::io;
use std::io::{Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};
use voice_lib::packet::{
    AssociateUdp, AssociateUdpAccepted, AudioFromClient, FinishConnect, FinishConnectAccepted,
    Hello, Packet, PacketBody,
};
use voice_lib::serialization::{serialize, DeserializeError, IncomingPacket, SerializeError};

fn read_next_tcp(tcp: &mut TcpStream) -> Result<IncomingPacket, DeserializeError> {
    let mut buffer = [0u8; 65535];
    let size = tcp.read(&mut buffer).unwrap();
    IncomingPacket::from_bytes(&buffer[..size])
}

fn read_next_udp(udp: &UdpSocket) -> Result<IncomingPacket, DeserializeError> {
    let mut buffer = [0u8; 65535];
    let size = udp.recv(&mut buffer).unwrap();
    IncomingPacket::from_bytes(&buffer[..size])
}

fn main() -> Result<(), BeepClientErr> {
    #[cfg(windows)]
    voice_lib::winlib::bump_priority();

    let matches = App::new("Beep Client")
        .version("0.1")
        .arg(
            Arg::with_name("server_address")
                .takes_value(true)
                .required(true),
        )
        .arg(Arg::with_name("client_name").takes_value(true))
        .get_matches();

    let server_address = match matches.value_of("server_address") {
        Some(server_address) => server_address,
        None => {
            eprintln!("{}", matches.usage());
            return Err(BeepClientErr::Usage);
        }
    };

    let client_name = matches.value_of("client_name").unwrap_or("beep");

    let server_address_with_port = format!("{}:18847", server_address);
    println!("Connecting to {}...", server_address_with_port);

    let mut tcp = TcpStream::connect(server_address_with_port.clone())?;
    let udp = UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(server_address_with_port)?;

    let parsed_packet = read_next_tcp(&mut tcp)?;
    assert_eq!(parsed_packet.header.kind, Hello::KIND_ID);
    let hello: Packet<Hello> = parsed_packet.finish_parse()?;
    dbg!(&hello);

    udp.send(&serialize(
        NonZeroU64::new(1).unwrap(),
        None,
        AssociateUdp {
            client_id: hello.body.client_id,
        },
    )?)?;

    let parsed_packet = read_next_udp(&udp)?;
    assert_eq!(parsed_packet.header.kind, AssociateUdpAccepted::KIND_ID);
    let accepted: Packet<AssociateUdpAccepted> = parsed_packet.finish_parse()?;
    dbg!(&accepted);

    tcp.write_all(&serialize(
        NonZeroU64::new(2).unwrap(),
        None,
        FinishConnect {
            name: client_name.to_string(),
        },
    )?)?;

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
    let mut next_packet_id = 3;

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
            udp.send(&serialize(
                NonZeroU64::new(next_packet_id).unwrap(),
                None,
                AudioFromClient(bytes),
            )?)?;
            next_packet_id += 1;
        }
    }
}

#[derive(Debug)]
enum BeepClientErr {
    Usage,
    Io(io::Error),
    Serialize,
    Deserialize,
}

impl From<io::Error> for BeepClientErr {
    fn from(e: io::Error) -> Self {
        BeepClientErr::Io(e)
    }
}

impl From<DeserializeError> for BeepClientErr {
    fn from(_: DeserializeError) -> Self {
        BeepClientErr::Deserialize
    }
}

impl From<SerializeError> for BeepClientErr {
    fn from(_: SerializeError) -> Self {
        BeepClientErr::Serialize
    }
}
