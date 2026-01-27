use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use hkv_client::{ClientConfig, ClientTtl, KVClient};

fn spawn_server(expected_commands: usize, handler: fn(usize, Vec<Vec<u8>>, &mut TcpStream)) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        for idx in 0..expected_commands {
            let args = read_command(&mut reader).expect("read command");
            handler(idx, args, &mut stream);
        }
    });

    addr
}

fn read_command(reader: &mut BufReader<TcpStream>) -> std::io::Result<Vec<Vec<u8>>> {
    let mut line = Vec::new();
    read_line(reader, &mut line)?.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof"))?;
    if line.first() != Some(&b'*') {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "expected array"));
    }
    let count = parse_usize(&line[1..])?;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        read_line(reader, &mut line)?.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof"))?;
        if line.first() != Some(&b'$') {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "expected bulk"));
        }
        let len = parse_usize(&line[1..])?;
        let mut data = vec![0u8; len];
        reader.read_exact(&mut data)?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf)?;
        if crlf != [b'\r', b'\n'] {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "missing crlf"));
        }
        args.push(data);
    }
    Ok(args)
}

fn read_line(reader: &mut BufReader<TcpStream>, buf: &mut Vec<u8>) -> std::io::Result<Option<()>> {
    buf.clear();
    let bytes = reader.read_until(b'\n', buf)?;
    if bytes == 0 {
        return Ok(None);
    }
    if buf.len() < 2 || buf[buf.len() - 2] != b'\r' {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid line"));
    }
    buf.truncate(buf.len() - 2);
    Ok(Some(()))
}

fn parse_usize(data: &[u8]) -> std::io::Result<usize> {
    if data.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "empty"));
    }
    let mut value = 0usize;
    for &b in data {
        if b < b'0' || b > b'9' {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "digit"));
        }
        value = value.saturating_mul(10).saturating_add((b - b'0') as usize);
    }
    Ok(value)
}

fn write_simple(stream: &mut TcpStream, msg: &str) {
    let _ = stream.write_all(b"+");
    let _ = stream.write_all(msg.as_bytes());
    let _ = stream.write_all(b"\r\n");
    let _ = stream.flush();
}

fn write_bulk(stream: &mut TcpStream, data: &[u8]) {
    let _ = stream.write_all(b"$");
    let _ = stream.write_all(data.len().to_string().as_bytes());
    let _ = stream.write_all(b"\r\n");
    let _ = stream.write_all(data);
    let _ = stream.write_all(b"\r\n");
    let _ = stream.flush();
}

fn write_integer(stream: &mut TcpStream, value: i64) {
    let _ = stream.write_all(b":");
    let _ = stream.write_all(value.to_string().as_bytes());
    let _ = stream.write_all(b"\r\n");
    let _ = stream.flush();
}

fn client_with_addr(addr: String) -> KVClient {
    let config = ClientConfig {
        addr,
        max_idle: 1,
        max_total: 1,
        read_timeout: Some(Duration::from_secs(1)),
        write_timeout: Some(Duration::from_secs(1)),
        connect_timeout: Some(Duration::from_secs(1)),
    };
    KVClient::with_config(config).expect("client")
}

#[test]
fn client_set_get_roundtrip() {
    let addr = spawn_server(2, |idx, args, stream| {
        if idx == 0 {
            assert_eq!(args[0], b"SET");
            assert_eq!(args[1], b"key");
            assert_eq!(args[2], b"value");
            write_simple(stream, "OK");
        } else {
            assert_eq!(args[0], b"GET");
            assert_eq!(args[1], b"key");
            write_bulk(stream, b"value");
        }
    });

    let client = client_with_addr(addr);
    client.set(b"key", b"value").expect("set");
    let value = client.get(b"key").expect("get");
    assert_eq!(value, Some(b"value".to_vec()));
}

#[test]
fn client_ttl_and_delete() {
    let addr = spawn_server(2, |idx, args, stream| {
        if idx == 0 {
            assert_eq!(args[0], b"TTL");
            assert_eq!(args[1], b"key");
            write_integer(stream, 5);
        } else {
            assert_eq!(args[0], b"DEL");
            assert_eq!(args[1], b"key");
            write_integer(stream, 1);
        }
    });

    let client = client_with_addr(addr);
    let ttl = client.ttl(b"key").expect("ttl");
    assert_eq!(ttl, ClientTtl::ExpiresIn(Duration::from_secs(5)));
    let removed = client.delete(b"key").expect("delete");
    assert!(removed);
}
