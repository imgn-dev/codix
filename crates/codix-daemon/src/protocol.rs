//! The daemon protocol: newline-delimited JSON frames, one request then one
//! response at a time on a connection.
//!
//! `hello`, `stats` and `stop` keep their shape across codix versions, so any
//! client can identify and stop any daemon, even one it is not compatible
//! with.

use std::io::{self, BufRead, Write};

use codix_core::daemon::DaemonRecord;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A request to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// Who are you? Carries the client's protocol and version.
    Hello {
        /// The client's protocol version.
        protocol: u32,
        /// The client's codix version.
        version: String,
    },
    /// Memory and idle time.
    Stats,
    /// Stop now.
    Stop,
}

/// The daemon's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    /// Answers `hello`.
    Hello(DaemonRecord),
    /// Answers `stats`.
    Stats(Stats),
    /// Answers `stop`; the daemon exits after sending it.
    Stopping,
}

/// What a daemon reports about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// Resident memory, in bytes.
    pub memory_bytes: u64,
    /// Seconds since the last operation request, or since the start.
    pub idle_seconds: u64,
}

/// Writes `frame` as one line.
pub fn write_frame<T: Serialize>(w: &mut impl Write, frame: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Reads one frame; `None` at the end of the stream.
pub fn read_frame<T: DeserializeOwned>(r: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn frames_round_trip_one_per_line() {
        let mut buffer = Vec::new();
        let hello = Request::Hello {
            protocol: 1,
            version: "0.1.0".to_owned(),
        };
        write_frame(&mut buffer, &hello).expect("write");
        write_frame(&mut buffer, &Request::Stop).expect("write");
        assert_eq!(
            String::from_utf8(buffer.clone()).expect("UTF-8"),
            "{\"hello\":{\"protocol\":1,\"version\":\"0.1.0\"}}\n\"stop\"\n"
        );
        let mut reader = BufReader::new(buffer.as_slice());
        assert_eq!(read_frame(&mut reader).expect("read"), Some(hello));
        assert_eq!(read_frame(&mut reader).expect("read"), Some(Request::Stop));
        assert_eq!(read_frame::<Request>(&mut reader).expect("read"), None);
    }

    #[test]
    fn a_malformed_frame_is_invalid_data() {
        let mut reader = BufReader::new("{\"hello\"\n".as_bytes());
        let error = read_frame::<Request>(&mut reader).expect_err("malformed");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
