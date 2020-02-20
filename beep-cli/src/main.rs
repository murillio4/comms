use std::net::{UdpSocket, TcpStream};
use std::time::{SystemTime, Duration};
use voice_lib::{packet, serialize, serialize_head, PacketHeader, serialize_body, deserialize_header, deserialize_remaining};
use clap::{App, Arg};
use std::io;
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use voice_lib::packet::{Packet, Hello, PacketBody, AssociateUdpAccepted};
use std::num::NonZeroU64;
use sample::ring_buffer::Slice;
use std::mem::size_of;
use std::io::{Read, Error};
use std::cmp::min;

#[derive(Debug)]
struct Marker(usize);
struct MarkerRead<'a>(&'a [u8], &'a mut Marker);

impl Read for MarkerRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let marker = (self.1).0;
        let target_slice = &self.0[marker..(marker + buf.len())];
        let len = min(target_slice.len(), buf.len());
        buf[..len].copy_from_slice(target_slice);
        (self.1).0 += buf.len();
        Ok(len)
    }
}

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
    let mut udp = UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(server_address_with_port)?;

    let mut buffer = [0u8; 65535];
    tcp.read(&mut buffer);

    let mut marker = Marker(0);
    let marker_reader = MarkerRead(&buffer, &mut marker);
    let header = deserialize_header(marker_reader).unwrap();
    let hello: Packet<Hello> = deserialize_remaining(&buffer[marker.0..], header).unwrap();
    dbg!(&hello);

    let associate = packet::AssociateUdp {
        client_id: hello.body.client_id,
    };
    let raw_packet = [
        serialize_head(PacketHeader {
            id: NonZeroU64::new(1).unwrap(),
            reply_to: Some(hello.id),
            kind: packet::AssociateUdp::KIND_ID,
        }).unwrap().as_slice(),
        &serialize_body(associate).unwrap().as_slice()
    ].concat();
    udp.send(&raw_packet)?;

    udp.recv(&mut buffer);
    let mut marker = Marker(0);
    let marker_reader = MarkerRead(&buffer, &mut marker);
    let header = deserialize_header(marker_reader).unwrap();
    let accepted: Packet<AssociateUdpAccepted> = deserialize_remaining(&buffer[marker.0..], header).unwrap();
    dbg!(&accepted);

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