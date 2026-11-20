//! One HTTP GET to a CDP endpoint's `/json/version` (std only): what
//! `doctor` reports and how the daemon tells a live `--cdp` from a dead one.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// GET `<endpoint>/json/version` and return its `Browser` field.
pub fn probe_version(endpoint: &str) -> Result<String, String> {
    let rest = endpoint
        .strip_prefix("http://")
        .ok_or_else(|| format!("`{endpoint}` is not an http:// endpoint"))?;
    let host = rest.split('/').next().unwrap_or(rest);
    let addr = host
        .to_socket_addrs()
        .map_err(|err| err.to_string())?
        .next()
        .ok_or_else(|| format!("{host} resolves to nothing"))?;
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|err| err.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|err| err.to_string())?;
    let request =
        format!("GET /json/version HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Chrome may keep the socket open: stop at Content-Length, not EOF.
    while !response_complete(&buf) {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) if !buf.is_empty() => break,
            Err(err) => return Err(err.to_string()),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    if !text.starts_with("HTTP/1.1 200") && !text.starts_with("HTTP/1.0 200") {
        let status = text.lines().next().unwrap_or("no response");
        return Err(format!("/json/version answered `{status}`"));
    }
    json_string_field(&text, "Browser").ok_or_else(|| "no Browser in /json/version".to_owned())
}

/// The buffered HTTP response has its whole `Content-Length` body.
pub fn response_complete(buf: &[u8]) -> bool {
    let Some(split) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&buf[..split]).to_ascii_lowercase();
    headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .is_some_and(|length| buf.len() - split - 4 >= length)
}

/// The string value of `key` in a flat JSON body.
pub fn json_string_field(body: &str, key: &str) -> Option<String> {
    let quoted = format!("\"{key}\"");
    let rest = &body[body.find(&quoted)? + quoted.len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}
