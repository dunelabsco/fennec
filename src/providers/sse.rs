//! Minimal byte-level line buffer for Server-Sent Events streaming.
//!
//! Provider streaming paths (Anthropic, OpenAI) previously accumulated
//! incoming HTTP chunks by pushing them into a `String` via
//! `String::from_utf8_lossy(&chunk)`. Two problems with that:
//!
//! 1. A single UTF-8 code point can straddle a chunk boundary. `from_utf8_lossy`
//!    then emits a `U+FFFD REPLACEMENT CHARACTER` for the half it sees, and the
//!    remaining half in the next chunk produces another replacement char.
//!    Multi-byte content (emoji, CJK, accented Latin) corrupts silently.
//!
//! 2. Rebuilding the buffer as `buf = buf[pos+1..].to_string()` after every
//!    line is O(N) per line — O(N²) across the whole stream.
//!
//! `SseBuffer` holds raw bytes in a `VecDeque<u8>` so it can defer the UTF-8
//! decode until a complete line is known, and pops lines from the front in
//! amortized O(line_length) rather than O(buffer_length).
//!
//! Line terminators follow the W3C Server-Sent Events §9.2 definition:
//! `\n`, `\r\n`, and bare `\r` all separate events. If the buffer ends on
//! a lone `\r`, we wait for the next chunk to disambiguate CRLF vs CR.

use std::collections::VecDeque;

/// Byte-level SSE line buffer.
pub struct SseBuffer {
    inner: VecDeque<u8>,
}

impl Default for SseBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl SseBuffer {
    pub fn new() -> Self {
        Self {
            inner: VecDeque::new(),
        }
    }

    /// Append raw bytes received from the network.
    pub fn extend(&mut self, chunk: &[u8]) {
        self.inner.extend(chunk.iter().copied());
    }

    /// Pop the next complete line from the front of the buffer, if any.
    ///
    /// Returns `None` when the buffer doesn't yet contain a full line — the
    /// caller should read another chunk and try again. The line terminator
    /// is consumed but not included in the returned bytes.
    ///
    /// A trailing lone `\r` at the end of the buffer is treated as
    /// "undecided" (could be bare CR or start of CRLF) and yields `None`
    /// until more data arrives.
    pub fn next_line(&mut self) -> Option<Vec<u8>> {
        // Scan for the earliest line terminator. Collect up to that point.
        let mut cut: Option<(usize, usize)> = None; // (line_end_idx, term_len)
        for (i, &b) in self.inner.iter().enumerate() {
            if b == b'\n' {
                cut = Some((i, 1));
                break;
            }
            if b == b'\r' {
                match self.inner.get(i + 1) {
                    Some(&b'\n') => {
                        cut = Some((i, 2));
                        break;
                    }
                    Some(_) => {
                        cut = Some((i, 1));
                        break;
                    }
                    None => {
                        // Lone trailing \r — can't tell if next chunk starts
                        // with \n (making it CRLF) or not. Wait.
                        return None;
                    }
                }
            }
        }

        let (line_end, term_len) = cut?;
        let line: Vec<u8> = self.inner.drain(..line_end).collect();
        for _ in 0..term_len {
            self.inner.pop_front();
        }
        Some(line)
    }

    /// Current buffered byte count. Exposed for tests / diagnostics.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).expect("valid utf-8")
    }

    #[test]
    fn empty_buffer_yields_none() {
        let mut b = SseBuffer::new();
        assert!(b.next_line().is_none());
    }

    #[test]
    fn incomplete_line_yields_none() {
        let mut b = SseBuffer::new();
        b.extend(b"data: hello");
        assert!(b.next_line().is_none());
    }

    #[test]
    fn complete_lf_line() {
        let mut b = SseBuffer::new();
        b.extend(b"data: hello\n");
        assert_eq!(s(&b.next_line().unwrap()), "data: hello");
        assert!(b.next_line().is_none());
    }

    #[test]
    fn complete_crlf_line() {
        let mut b = SseBuffer::new();
        b.extend(b"event: ping\r\n");
        assert_eq!(s(&b.next_line().unwrap()), "event: ping");
    }

    #[test]
    fn complete_cr_line() {
        // Bare CR is rare in practice but legal per SSE spec.
        let mut b = SseBuffer::new();
        b.extend(b"data: old-mac\rdata: next\n");
        assert_eq!(s(&b.next_line().unwrap()), "data: old-mac");
        assert_eq!(s(&b.next_line().unwrap()), "data: next");
    }

    #[test]
    fn trailing_lone_cr_waits_for_more() {
        // Until we see the byte after \r, we can't tell CRLF from CR.
        let mut b = SseBuffer::new();
        b.extend(b"data: x\r");
        assert!(b.next_line().is_none(), "lone trailing \\r must wait");
        // Resolve as CRLF.
        b.extend(b"\n");
        assert_eq!(s(&b.next_line().unwrap()), "data: x");
        // Resolve as CR (different scenario).
        let mut b = SseBuffer::new();
        b.extend(b"data: y\r");
        assert!(b.next_line().is_none());
        b.extend(b"more");
        assert_eq!(s(&b.next_line().unwrap()), "data: y");
    }

    #[test]
    fn multiple_lines_in_one_extend() {
        let mut b = SseBuffer::new();
        b.extend(b"event: a\ndata: 1\n\nevent: b\ndata: 2\n");
        assert_eq!(s(&b.next_line().unwrap()), "event: a");
        assert_eq!(s(&b.next_line().unwrap()), "data: 1");
        assert_eq!(s(&b.next_line().unwrap()), ""); // blank line separator
        assert_eq!(s(&b.next_line().unwrap()), "event: b");
        assert_eq!(s(&b.next_line().unwrap()), "data: 2");
        assert!(b.next_line().is_none());
    }

    #[test]
    fn line_split_across_chunks() {
        let mut b = SseBuffer::new();
        b.extend(b"data: hel");
        assert!(b.next_line().is_none());
        b.extend(b"lo wor");
        assert!(b.next_line().is_none());
        b.extend(b"ld\n");
        assert_eq!(s(&b.next_line().unwrap()), "data: hello world");
    }

    /// This is the headline bug the whole module exists to prevent.
    /// "日" is 3 bytes (0xE6 0x97 0xA5). If the TCP chunk splits after
    /// the first byte, from_utf8_lossy in the old path would emit two
    /// U+FFFD replacement chars. `SseBuffer` defers the UTF-8 decode
    /// until the full line is in hand, so the round-trip is clean.
    #[test]
    fn utf8_split_across_chunks_is_not_mangled() {
        let full_line = "data: 日本語\n";
        let bytes = full_line.as_bytes();
        let mid = 8; // mid of "日" (index 7, 8, or 9)
        let (chunk_a, chunk_b) = bytes.split_at(mid);

        let mut b = SseBuffer::new();
        b.extend(chunk_a);
        assert!(b.next_line().is_none());
        b.extend(chunk_b);
        let line = b.next_line().unwrap();
        assert_eq!(s(&line), "data: 日本語");
        // Crucially, no replacement char.
        assert!(!s(&line).contains('\u{FFFD}'));
    }

    #[test]
    fn crlf_split_across_chunks() {
        // \r arrives in chunk A, \n arrives in chunk B — CRLF.
        let mut b = SseBuffer::new();
        b.extend(b"data: x\r");
        assert!(b.next_line().is_none());
        b.extend(b"\ndata: y\n");
        assert_eq!(s(&b.next_line().unwrap()), "data: x");
        assert_eq!(s(&b.next_line().unwrap()), "data: y");
    }

    #[test]
    fn buffer_is_drained_after_pop() {
        // After popping all lines, the buffer should be empty — making
        // sure drain+pop_front doesn't leave stray bytes behind.
        let mut b = SseBuffer::new();
        b.extend(b"a\nb\r\nc\rd\n");
        while b.next_line().is_some() {}
        assert_eq!(b.len(), 0);
    }
}
