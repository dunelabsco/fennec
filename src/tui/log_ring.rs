//! In-process ring buffer of recent tracing output. Fennec doesn't
//! have a separate gateway process, so `/logs` shows whatever the
//! tracing subsystem has been emitting from the agent / channels
//! / TUI itself.
//!
//! Lives in the TUI module because only TUI mode installs the
//! `LogRingLayer` — non-TUI binaries log straight to stderr via
//! the default subscriber. Capped at 200 formatted lines (a
//! comfortable scrollback for `/logs 80`).

use std::sync::Arc;

use parking_lot::Mutex;

/// Capacity of the ring buffer. Older lines are dropped FIFO.
/// 200 is enough for the `/logs [N]` cap of 80 + comfortable
/// headroom for the most recent burst.
const RING_CAP: usize = 200;

/// Thread-safe FIFO ring of recent log lines. Cloneable handle
/// (it's just `Arc<Mutex<…>>`); both the tracing layer and the
/// `App` hold one and the renderer drains via `tail`.
#[derive(Clone, Default)]
pub struct LogRing {
    inner: Arc<Mutex<std::collections::VecDeque<String>>>,
}

impl LogRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a formatted line. Drops the oldest entry when at
    /// capacity so memory stays bounded.
    pub fn push(&self, line: String) {
        let mut g = self.inner.lock();
        if g.len() == RING_CAP {
            g.pop_front();
        }
        g.push_back(line);
    }

    /// Take the last `n` lines in chronological (oldest-first)
    /// order. Used by `/logs [N]` (default 20, capped at 80).
    pub fn tail(&self, n: usize) -> Vec<String> {
        let g = self.inner.lock();
        let len = g.len();
        let start = len.saturating_sub(n);
        g.iter().skip(start).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}

/// Per-event writer handed back to `tracing-subscriber`'s fmt
/// layer. Owns a clone of the [`LogRing`] handle (Arc-shared
/// inner) and a small per-write line buffer so a single event's
/// `write` call can deliver multiple newline-terminated lines.
pub struct LogRingWriter {
    ring: LogRing,
    pending: String,
}

impl LogRingWriter {
    pub fn new(ring: LogRing) -> Self {
        Self {
            ring,
            pending: String::new(),
        }
    }
}

impl std::io::Write for LogRingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        self.pending.push_str(&s);
        while let Some(nl) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=nl).collect();
            let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
            if !trimmed.is_empty() {
                self.ring.push(trimmed);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Push any partial-line residue on flush — tracing-subscriber
        // calls flush at the end of each event, so an event without
        // a trailing newline still lands in the ring.
        if !self.pending.is_empty() {
            let trimmed = self
                .pending
                .trim_end_matches(['\n', '\r'])
                .to_string();
            self.pending.clear();
            if !trimmed.is_empty() {
                self.ring.push(trimmed);
            }
        }
        Ok(())
    }
}

/// `MakeWriter` impl so `tracing_subscriber::fmt::Layer::with_writer`
/// can construct a fresh per-event writer. The ring is `Arc`-shared
/// inside `LogRing`, so all events land in the same buffer.
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogRing {
    type Writer = LogRingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogRingWriter::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn push_and_tail_round_trip() {
        let ring = LogRing::new();
        ring.push("a".into());
        ring.push("b".into());
        ring.push("c".into());
        assert_eq!(ring.tail(2), vec!["b".to_string(), "c".to_string()]);
        assert_eq!(ring.tail(10), vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn evicts_at_cap() {
        let ring = LogRing::new();
        for i in 0..RING_CAP + 50 {
            ring.push(format!("line-{i}"));
        }
        assert_eq!(ring.len(), RING_CAP);
        let last_two = ring.tail(2);
        assert_eq!(
            last_two,
            vec![
                format!("line-{}", RING_CAP + 50 - 2),
                format!("line-{}", RING_CAP + 50 - 1),
            ]
        );
    }

    #[test]
    fn writer_splits_on_newlines() {
        let ring = LogRing::new();
        let mut writer = LogRingWriter::new(ring.clone());
        writer.write_all(b"hello\n").unwrap();
        writer.write_all(b"part").unwrap();
        writer.write_all(b"ial\n").unwrap();
        assert_eq!(ring.tail(10), vec!["hello".to_string(), "partial".to_string()]);
    }

    #[test]
    fn writer_flushes_partial_line() {
        let ring = LogRing::new();
        let mut writer = LogRingWriter::new(ring.clone());
        writer.write_all(b"no-newline-yet").unwrap();
        writer.flush().unwrap();
        assert_eq!(ring.tail(10), vec!["no-newline-yet".to_string()]);
    }
}
