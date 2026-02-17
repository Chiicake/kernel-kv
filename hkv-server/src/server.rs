//! # TCP Server
//!
//! Accept RESP2 connections, parse commands, and dispatch them to the
//! storage engine with minimal overhead.

use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use hkv_engine::{KVEngine, MemoryEngine, TtlStatus};

use crate::protocol::{RespError, RespParser};

/// Handles a single TCP client connection.
pub async fn handle_connection(stream: TcpStream, engine: Arc<MemoryEngine>) -> std::io::Result<()> {
    let mut stream = stream;
    let mut buffer = BytesMut::with_capacity(8 * 1024);
    let mut parser = RespParser::new();

    loop {
        let bytes = stream.read_buf(&mut buffer).await?;
        if bytes == 0 {
            break;
        }

        loop {
            match parser.parse(&mut buffer) {
                Ok(Some(args)) => {
                    let response = dispatch_command(&args, engine.as_ref());
                    stream.write_all(&response).await?;
                }
                Ok(None) => break,
                Err(RespError::Protocol) => {
                    stream.write_all(&*resp_error("protocol error")).await?;
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn dispatch_command(args: &[Vec<u8>], engine: &MemoryEngine) -> Vec<u8> {
    if args.is_empty() {
        return resp_error("empty command");
    }

    let cmd = &args[0];
    if eq_ignore_ascii_case(cmd, b"PING") {
        return handle_ping(args);
    }
    if eq_ignore_ascii_case(cmd, b"GET") {
        return handle_get(args, engine);
    }
    if eq_ignore_ascii_case(cmd, b"SET") {
        return handle_set(args, engine);
    }
    if eq_ignore_ascii_case(cmd, b"DEL") {
        return handle_del(args, engine);
    }
    if eq_ignore_ascii_case(cmd, b"EXPIRE") {
        return handle_expire(args, engine);
    }
    if eq_ignore_ascii_case(cmd, b"TTL") {
        return handle_ttl(args, engine);
    }
    if eq_ignore_ascii_case(cmd, b"INFO") {
        return handle_info();
    }

    resp_error("unknown command")
}

fn handle_ping(args: &[Vec<u8>]) -> Vec<u8> {
    match args.len() {
        1 => resp_simple("PONG"),
        2 => resp_bulk(&args[1]),
        _ => resp_error("wrong number of arguments for PING"),
    }
}

fn handle_get(args: &[Vec<u8>], engine: &MemoryEngine) -> Vec<u8> {
    if args.len() != 2 {
        return resp_error("wrong number of arguments for GET");
    }
    match engine.get(&args[1]) {
        Ok(Some(value)) => resp_bulk(&value),
        Ok(None) => resp_null(),
        Err(_) => resp_error("engine error"),
    }
}

fn handle_set(args: &[Vec<u8>], engine: &MemoryEngine) -> Vec<u8> {
    if args.len() < 3 {
        return resp_error("wrong number of arguments for SET");
    }

    let key = args[1].clone();
    let value = args[2].clone();

    if args.len() == 3 {
        if engine.set(key, value).is_ok() {
            return resp_simple("OK");
        }
        return resp_error("engine error");
    }

    if args.len() == 5 && eq_ignore_ascii_case(&args[3], b"EX") {
        let seconds = match parse_u64(&args[4]) {
            Ok(value) => value,
            Err(resp) => return resp,
        };

        if engine.set(key, value).is_err() {
            return resp_error("engine error");
        }

        if engine.expire(&args[1], Duration::from_secs(seconds)).is_err() {
            return resp_error("engine error");
        }

        return resp_simple("OK");
    }

    resp_error("unsupported SET options")
}

fn handle_del(args: &[Vec<u8>], engine: &MemoryEngine) -> Vec<u8> {
    if args.len() < 2 {
        return resp_error("wrong number of arguments for DEL");
    }

    let mut removed = 0i64;
    for key in &args[1..] {
        match engine.delete(key) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(_) => return resp_error("engine error"),
        }
    }

    resp_integer(removed)
}

fn handle_expire(args: &[Vec<u8>], engine: &MemoryEngine) -> Vec<u8> {
    if args.len() != 3 {
        return resp_error("wrong number of arguments for EXPIRE");
    }

    let seconds = match parse_u64(&args[2]) {
        Ok(value) => value,
        Err(resp) => return resp,
    };

    match engine.expire(&args[1], Duration::from_secs(seconds)) {
        Ok(()) => resp_integer(1),
        Err(err) if err == hkv_common::HkvError::NotFound => resp_integer(0),
        Err(_) => resp_error("engine error"),
    }
}

fn handle_ttl(args: &[Vec<u8>], engine: &MemoryEngine) -> Vec<u8> {
    if args.len() != 2 {
        return resp_error("wrong number of arguments for TTL");
    }

    match engine.ttl(&args[1]) {
        Ok(TtlStatus::Missing) => resp_integer(-2),
        Ok(TtlStatus::NoExpiry) => resp_integer(-1),
        Ok(TtlStatus::ExpiresIn(remaining)) => resp_integer(remaining.as_secs() as i64),
        Err(_) => resp_error("engine error"),
    }
}

fn handle_info() -> Vec<u8> {
    let info = b"role:master\r\nengine:hybridkv\r\n";
    resp_bulk(info)
}

fn resp_simple(message: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(message.len() + 3);
    buf.extend_from_slice(b"+");
    buf.extend_from_slice(message.as_bytes());
    buf.extend_from_slice(b"\r\n");
    buf
}

fn resp_error(message: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(message.len() + 6);
    buf.extend_from_slice(b"-ERR ");
    buf.extend_from_slice(message.as_bytes());
    buf.extend_from_slice(b"\r\n");
    buf
}

fn resp_integer(value: i64) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b":");
    buf.extend_from_slice(value.to_string().as_bytes());
    buf.extend_from_slice(b"\r\n");
    buf
}

fn resp_bulk(data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"$");
    buf.extend_from_slice(data.len().to_string().as_bytes());
    buf.extend_from_slice(b"\r\n");
    buf.extend_from_slice(data);
    buf.extend_from_slice(b"\r\n");
    buf
}

fn resp_null() -> Vec<u8> {
    b"$-1\r\n".to_vec()
}

fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

fn parse_u64(arg: &[u8]) -> Result<u64, Vec<u8>> {
    if arg.is_empty() {
        return Err(resp_error("invalid integer"));
    }
    let mut value: u64 = 0;
    for &b in arg {
        if b < b'0' || b > b'9' {
            return Err(resp_error("invalid integer"));
        }
        value = value.saturating_mul(10).saturating_add((b - b'0') as u64);
    }
    Ok(value)
}
