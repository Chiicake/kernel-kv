//! # RESP2 Encoding and Parsing
//!
//! Purpose: Encode client commands and parse server responses without
//! external dependencies, keeping allocations under control.
//!
//! ## Design Principles
//! 1. **State-Free Parsing**: Responses are parsed top-down with minimal state.
//! 2. **Buffer Reuse**: Caller provides buffers to avoid per-call allocations.
//! 3. **Binary-Safe**: Bulk strings are treated as raw bytes.
//! 4. **Fail Fast**: Invalid framing returns protocol errors immediately.

use std::io::BufRead;

use crate::client::{ClientError, ClientResult};

/// RESP response value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespValue {
    /// +OK or +PONG style responses.
    Simple(Vec<u8>),
    /// -ERR ... responses.
    Error(Vec<u8>),
    /// :123 responses.
    Integer(i64),
    /// $... bulk strings, with None for null.
    Bulk(Option<Vec<u8>>),
    /// *... arrays (rare in this client).
    Array(Vec<RespValue>),
}

/// Encodes a RESP2 array command into the provided buffer.
pub fn encode_command(args: &[&[u8]], out: &mut Vec<u8>) {
    out.push(b'*');
    push_usize(out, args.len());
    out.extend_from_slice(b"\r\n");
    for arg in args {
        out.push(b'$');
        push_usize(out, arg.len());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(arg);
        out.extend_from_slice(b"\r\n");
    }
}

/// Reads one RESP value from the buffered reader.
pub fn read_response<R: BufRead>(reader: &mut R, line_buf: &mut Vec<u8>) -> ClientResult<RespValue> {
    read_line(reader, line_buf)?;
    if line_buf.is_empty() {
        return Err(ClientError::Protocol);
    }

    match line_buf[0] {
        b'+' => Ok(RespValue::Simple(line_buf[1..].to_vec())),
        b'-' => Ok(RespValue::Error(line_buf[1..].to_vec())),
        b':' => Ok(RespValue::Integer(parse_i64(&line_buf[1..])?)),
        b'$' => {
            let len = parse_i64(&line_buf[1..])?;
            parse_bulk_len(reader, len, line_buf)
        }
        b'*' => {
            let len = parse_i64(&line_buf[1..])?;
            parse_array_len(reader, len, line_buf)
        }
        _ => Err(ClientError::Protocol),
    }
}

fn parse_bulk_len<R: BufRead>(
    reader: &mut R,
    len: i64,
    line_buf: &mut Vec<u8>,
) -> ClientResult<RespValue> {
    if len < 0 {
        return Ok(RespValue::Bulk(None));
    }
    let len = len as usize;
    let mut data = vec![0u8; len];
    reader.read_exact(&mut data)?;

    let mut crlf = [0u8; 2];
    reader.read_exact(&mut crlf)?;
    if crlf != [b'\r', b'\n'] {
        return Err(ClientError::Protocol);
    }

    line_buf.clear();
    Ok(RespValue::Bulk(Some(data)))
}

fn parse_array_len<R: BufRead>(
    reader: &mut R,
    len: i64,
    line_buf: &mut Vec<u8>,
) -> ClientResult<RespValue> {
    if len <= 0 {
        return Ok(RespValue::Array(Vec::new()));
    }

    let mut items = Vec::with_capacity(len as usize);
    for _ in 0..len {
        items.push(read_response(reader, line_buf)?);
    }
    Ok(RespValue::Array(items))
}

fn read_line<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>) -> ClientResult<()> {
    buf.clear();
    let bytes = reader.read_until(b'\n', buf)?;
    if bytes == 0 {
        return Err(ClientError::Protocol);
    }
    if buf.len() < 2 || buf[buf.len() - 2] != b'\r' {
        return Err(ClientError::Protocol);
    }
    buf.truncate(buf.len() - 2);
    Ok(())
}

fn parse_i64(data: &[u8]) -> ClientResult<i64> {
    if data.is_empty() {
        return Err(ClientError::Protocol);
    }
    let mut negative = false;
    let mut idx = 0;
    if data[0] == b'-' {
        negative = true;
        idx = 1;
    }

    let mut value: i64 = 0;
    while idx < data.len() {
        let b = data[idx];
        if b < b'0' || b > b'9' {
            return Err(ClientError::Protocol);
        }
        value = value.saturating_mul(10).saturating_add((b - b'0') as i64);
        idx += 1;
    }

    if negative {
        Ok(-value)
    } else {
        Ok(value)
    }
}

fn push_usize(out: &mut Vec<u8>, mut value: usize) {
    // Write digits into a small stack buffer to avoid heap allocations.
    let mut buf = [0u8; 20];
    let mut len = 0;
    if value == 0 {
        buf[0] = b'0';
        len = 1;
    } else {
        while value > 0 {
            buf[len] = b'0' + (value % 10) as u8;
            value /= 10;
            len += 1;
        }
    }
    for idx in (0..len).rev() {
        out.push(buf[idx]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn encodes_command() {
        let mut buf = Vec::new();
        encode_command(&[b"GET", b"key"], &mut buf);
        assert_eq!(&buf, b"*2\r\n$3\r\nGET\r\n$3\r\nkey\r\n");
    }

    #[test]
    fn parses_simple_string() {
        let mut reader = Cursor::new(b"+OK\r\n".to_vec());
        let mut line = Vec::new();
        let resp = read_response(&mut reader, &mut line).unwrap();
        assert_eq!(resp, RespValue::Simple(b"OK".to_vec()));
    }

    #[test]
    fn parses_bulk_string() {
        let mut reader = Cursor::new(b"$5\r\nhello\r\n".to_vec());
        let mut line = Vec::new();
        let resp = read_response(&mut reader, &mut line).unwrap();
        assert_eq!(resp, RespValue::Bulk(Some(b"hello".to_vec())));
    }

    #[test]
    fn parses_null_bulk_string() {
        let mut reader = Cursor::new(b"$-1\r\n".to_vec());
        let mut line = Vec::new();
        let resp = read_response(&mut reader, &mut line).unwrap();
        assert_eq!(resp, RespValue::Bulk(None));
    }

    #[test]
    fn parses_integer() {
        let mut reader = Cursor::new(b":42\r\n".to_vec());
        let mut line = Vec::new();
        let resp = read_response(&mut reader, &mut line).unwrap();
        assert_eq!(resp, RespValue::Integer(42));
    }

    #[test]
    fn parses_error() {
        let mut reader = Cursor::new(b"-ERR bad\r\n".to_vec());
        let mut line = Vec::new();
        let resp = read_response(&mut reader, &mut line).unwrap();
        assert_eq!(resp, RespValue::Error(b"ERR bad".to_vec()));
    }
}
