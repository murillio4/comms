use std::net::{UdpSocket, TcpStream};
use std::time::{SystemTime, Duration};
use voice_lib::{PacketHeader, serialize, HeaderParsedPacket};
use clap::{App, Arg};
use std::io;
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use voice_lib::packet::{Packet, Hello, PacketBody, AssociateUdpAccepted, FinishConnect, AssociateUdp, FinishConnectAccepted, AudioFromClient};
use std::num::NonZeroU64;
use std::io::{Read, Write};

fn main() -> Result<(), BeepClientErr> {
    #[cfg(windows)]
        bump_priority();

    let matches = App::new("Beep Client")
        .version("0.1")
        .arg(Arg::with_name("server_address").takes_value(true).required(true))
        .get_matches();

    let server_address = match matches.value_of("server_address") {
        Some(server_address) => server_address,
        None => {
            eprintln!("{}", matches.usage());
            return Err(BeepClientErr::Usage);
        }
    };

    let server_address_with_port = format!("{}:18847", server_address);
    println!("Connecting to {}...", server_address_with_port);

    let mut tcp = TcpStream::connect(server_address_with_port.clone())?;
    let udp = UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(server_address_with_port)?;

    let mut buffer = [0u8; 65535];

    tcp.read(&mut buffer).unwrap();
    let parsed_packet = HeaderParsedPacket::from_raw(&buffer)?;
    assert_eq!(parsed_packet.kind(), Hello::KIND_ID);
    let hello: Packet<Hello> = parsed_packet.finish_parse()?;
    dbg!(&hello);

    udp.send(&serialize(
        PacketHeader {
            id: NonZeroU64::new(1).unwrap(),
            reply_to: None,
            kind: AssociateUdp::KIND_ID,
        },
        AssociateUdp {
            client_id: hello.body.client_id,
        },
    )?)?;

    udp.recv(&mut buffer).unwrap();
    let parsed_packet = HeaderParsedPacket::from_raw(&buffer)?;
    assert_eq!(parsed_packet.kind(), AssociateUdpAccepted::KIND_ID);
    let accepted: Packet<AssociateUdpAccepted> = parsed_packet.finish_parse()?;
    dbg!(&accepted);

    tcp.write(&serialize(
        PacketHeader {
            id: NonZeroU64::new(2).unwrap(),
            reply_to: None,
            kind: FinishConnect::KIND_ID,
        },
        FinishConnect {
            username: "beep".to_string(),
        },
    )?)?;

    tcp.read(&mut buffer).unwrap();
    let parsed_packet = HeaderParsedPacket::from_raw(&buffer)?;
    assert_eq!(parsed_packet.kind(), FinishConnectAccepted::KIND_ID);
    let finish_connect_accepted: Packet<FinishConnectAccepted> = parsed_packet.finish_parse()?;
    dbg!(&finish_connect_accepted);

    let sample_rate = 48000f32;
    let mut sample_clock = 0f32;

    // Produce a sinusoid of maximum amplitude.
    let mut next_value = || {
        sample_clock = (sample_clock + 1.0) % sample_rate;
        ((sample_clock * 440.0 * 2.0 * std::f32::consts::PI / sample_rate).sin() * std::i16::MAX as f32 * 0.2) as i16
    };

    let start = SystemTime::now();
    let mut sample_count: u128 = 0;
    let mut next_packet_id = 3;

    loop {
        std::thread::sleep(Duration::from_millis(5));
        let now = SystemTime::now();
        let diff = now.duration_since(start).unwrap();
        let ms = diff.as_millis();
        let total_samples_since_start = ms * 48;
        let sample_count_to_send = total_samples_since_start - sample_count;
        sample_count = total_samples_since_start;

        let mut samples = Vec::with_capacity(sample_count_to_send as usize * 2);
        for _ in 0..sample_count_to_send {
            let value = next_value();
            samples.extend(&value.to_le_bytes());
        }
        udp.send(&serialize(
            PacketHeader {
                id: NonZeroU64::new(next_packet_id).unwrap(),
                reply_to: None,
                kind: AudioFromClient::KIND_ID,
            },
            AudioFromClient(samples),
        )?)?;
        next_packet_id += 1;
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

impl From<voice_lib::DeserializeError> for BeepClientErr {
    fn from(_: voice_lib::DeserializeError) -> Self {
        BeepClientErr::Deserialize
    }
}

impl From<voice_lib::SerializeError> for BeepClientErr {
    fn from(_: voice_lib::SerializeError) -> Self {
        BeepClientErr::Serialize
    }
}