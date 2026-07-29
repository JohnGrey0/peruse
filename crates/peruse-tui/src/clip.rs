//! The clipboard. Peruse uses the terminal escape sequence OSC 52.
//!
//! OSC 52 is a sequence of characters that Peruse writes to the terminal. The
//! terminal then puts the text on the clipboard. Peruse does not use a library
//! for the clipboard of the operating system, for three reasons:
//!
//! * The program needs no more dependencies.
//! * The code needs no separate part for X11, for Wayland and for macOS.
//! * The clipboard works when Peruse runs on another machine through SSH. The
//!   text goes to the clipboard of the machine of the user.

use std::io::{self, Write};

/// The largest number of bytes that Peruse sends to the terminal.
///
/// A terminal discards a very long OSC 52 sequence. An error message is better
/// for the user than one half of a value on the clipboard.
const MAX_BYTES: usize = 96 * 1024;

/// Writes bytes in the base64 form of the standard RFC 4648.
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Puts a text on the clipboard of the terminal.
pub fn copy(text: &str) -> io::Result<()> {
    let bytes = text.as_bytes();
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other(format!(
            "value is {} KB — too large for the terminal clipboard",
            bytes.len() / 1024
        )));
    }
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(bytes))?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_including_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_high_bytes() {
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
        assert_eq!(base64("日本".as_bytes()), "5pel5pys");
    }

    #[test]
    fn oversized_payloads_are_refused_rather_than_truncated() {
        let big = "x".repeat(MAX_BYTES + 1);
        let err = copy(&big).unwrap_err();
        assert!(err.to_string().contains("too large"), "got {err}");
    }
}
