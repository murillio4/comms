// TODO impl tcp
//fn read_from_tcp_stream(stream: &mut TcpStream) -> Result<TcpRead, io::Error> {
//    let mut received_data = Vec::new();
//    loop {
//        let mut buf = [0; 256];
//        match stream.read(&mut buf) {
//            Ok(0) => return Ok(TcpRead::ConnectionClosed),
//            Ok(n) => received_data.extend_from_slice(&buf[..n]),
//            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(TcpRead::Data(received_data)),
//            Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
//            Err(err) => return Err(err),
//        }
//    }
//}
//
//enum TcpRead {
//    ConnectionClosed,
//    Data(Vec<u8>),
//}