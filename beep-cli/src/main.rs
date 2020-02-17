use std::net::{UdpSocket, TcpStream};
use std::time::{SystemTime, Duration};
use voice_lib::{packet, serialize, serialize_head, PacketHeader, serialize_body};
use clap::{App, Arg};
use std::io;
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use voice_lib::packet::Packet;
use std::num::NonZeroU64;
use sample::ring_buffer::Slice;
use std::mem::size_of;

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

    let tcp = TcpStream::connect(server_address_with_port.clone())?;
    let udp = UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(server_address_with_port)?;
    dbg!(size_of::<NonZeroU64>());
    dbg!(size_of::<PacketHeader>());

    let associate = packet::AssociateUdp {
        client_id: 4,
    };
    let raw_packet = [
        serialize_head(PacketHeader {
            id: NonZeroU64::new(1).unwrap(),
            reply_to: None,
            kind: 00,
        }).unwrap().as_slice(),
        &serialize_body(associate).unwrap().as_slice()
    ].concat();
    udp.send(&raw_packet)?;
    Ok(())

//    let sample_rate = 48000f32;
//    let mut sample_clock = 0f32;
//
//    // Produce a sinusoid of maximum amplitude.
//    let mut next_value = || {
//        sample_clock = (sample_clock + 1.0) % sample_rate;
//        ((sample_clock * 440.0 * 2.0 * std::f32::consts::PI / sample_rate).sin() * std::i16::MAX as f32 * 0.2) as i16
//    };
//
//    let socket = UdpSocket::bind("0.0.0.0:0")?;
//    socket.connect(server_address_with_port)?;
//    let start = SystemTime::now();
//    let mut sample_count: u128 = 0;
//    loop {
//        std::thread::sleep(Duration::from_millis(5));
//        let now = SystemTime::now();
//        let diff = now.duration_since(start).unwrap();
//        let ms = diff.as_millis();
//        let total_samples_since_start = ms * 48;
//        let sample_count_to_send = total_samples_since_start - sample_count;
//        sample_count = total_samples_since_start;
//
//        let mut samples = Vec::with_capacity(sample_count_to_send as usize * 2);
//        for _ in 0..sample_count_to_send {
//            let value = next_value();
//            samples.extend(&value.to_le_bytes());
//        }
//        let packet = packet::AudioFromClient(&samples);
//        socket.send(&serialize(packet)?)?;
//    }
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

impl From<voice_lib::SerializeError> for BeepClientErr {
    fn from(_: voice_lib::SerializeError) -> Self {
        BeepClientErr::Serialize
    }
}