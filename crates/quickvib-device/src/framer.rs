//! Little-endian `f32` framing over any [`Read`] (`docs/PLAN.md` 7.3, D5).
//!
//! The input is `impl Read` rather than a socket, so the bug-prone part of the M300 path is
//! driven in tests from a `&[u8]` cursor and from a deliberately pathological reader that
//! yields one byte per call. `chunks_exact(4)` plus [`f32::from_le_bytes`] keeps the hot loop
//! free of both bounds checks and `unsafe`, and is correct on any host endianness.

use std::fmt;
use std::io::{ErrorKind, Read};

/// Size of the reusable read buffer.
pub const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Bytes per sample on the wire.
pub const BYTES_PER_SAMPLE: usize = 4;

/// Framing failures.
#[derive(Debug)]
#[non_exhaustive]
pub enum FramerError {
    /// The peer closed mid-sample, leaving 1–3 bytes that can never be completed.
    TruncatedSample {
        /// How many bytes were left over.
        leftover: usize,
    },
    /// The underlying reader failed.
    Io {
        /// The underlying operating-system error.
        source: std::io::Error,
    },
}

impl fmt::Display for FramerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedSample { leftover } => {
                write!(
                    f,
                    "stream ended mid-sample with {leftover} trailing byte(s)"
                )
            }
            Self::Io { source } => write!(f, "read failed: {source}"),
        }
    }
}

impl std::error::Error for FramerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source } => Some(source),
            Self::TruncatedSample { .. } => None,
        }
    }
}

impl From<std::io::Error> for FramerError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

/// What a single call to [`Framer::read_batch`] produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framed {
    /// Samples are available in [`Framer::samples`].
    Batch,
    /// The read timed out with nothing new. The caller should check for cancellation and try
    /// again.
    WouldBlock,
    /// The peer closed cleanly on a sample boundary.
    Eof,
}

/// Converts a byte stream into `f32` samples, carrying a 1–3 byte remainder across reads.
#[derive(Debug)]
pub struct Framer<R> {
    reader: R,
    bytes: Vec<u8>,
    remainder: [u8; BYTES_PER_SAMPLE],
    remainder_len: usize,
    samples: Vec<f32>,
    total_samples: u64,
}

impl<R: Read> Framer<R> {
    /// A framer with the default [`READ_BUFFER_BYTES`] buffer.
    pub fn new(reader: R) -> Self {
        Self::with_capacity(reader, READ_BUFFER_BYTES)
    }

    /// A framer with an explicit read-buffer size. Tests use tiny buffers to force the
    /// remainder path.
    ///
    /// # Panics
    /// Never; a capacity below one sample is rounded up.
    pub fn with_capacity(reader: R, capacity: usize) -> Self {
        let capacity = capacity.max(BYTES_PER_SAMPLE);
        Self {
            reader,
            bytes: vec![0; capacity],
            remainder: [0; BYTES_PER_SAMPLE],
            remainder_len: 0,
            samples: Vec::with_capacity(capacity / BYTES_PER_SAMPLE + 1),
            total_samples: 0,
        }
    }

    /// The samples decoded by the most recent [`Framer::read_batch`].
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Total samples decoded since the framer was created.
    #[must_use]
    pub const fn total_samples(&self) -> u64 {
        self.total_samples
    }

    /// Bytes currently held back because they do not complete a sample.
    #[must_use]
    pub const fn pending_bytes(&self) -> usize {
        self.remainder_len
    }

    /// Read once and decode whatever complete samples became available.
    ///
    /// A read of zero bytes is a clean close: [`Framed::Eof`] if nothing is pending, and
    /// [`FramerError::TruncatedSample`] otherwise. `ErrorKind::Interrupted` is retried;
    /// `WouldBlock` and `TimedOut` surface as [`Framed::WouldBlock`] so a caller with a read
    /// timeout can poll its cancellation token.
    ///
    /// # Errors
    /// [`FramerError::Io`] for any other read failure, or [`FramerError::TruncatedSample`]
    /// when the peer closes mid-sample.
    pub fn read_batch(&mut self) -> Result<Framed, FramerError> {
        self.samples.clear();

        let read = loop {
            match self.reader.read(&mut self.bytes) {
                Ok(n) => break n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    return Ok(Framed::WouldBlock)
                }
                Err(e) => return Err(e.into()),
            }
        };

        if read == 0 {
            if self.remainder_len != 0 {
                return Err(FramerError::TruncatedSample {
                    leftover: self.remainder_len,
                });
            }
            return Ok(Framed::Eof);
        }

        self.decode(read);
        Ok(Framed::Batch)
    }

    /// Decode `read` freshly-read bytes, prepending any carried remainder.
    fn decode(&mut self, read: usize) {
        let mut offset = 0;

        // Finish the sample straddling the previous read, if there is one.
        if self.remainder_len != 0 {
            let needed = BYTES_PER_SAMPLE - self.remainder_len;
            let take = needed.min(read);
            self.remainder[self.remainder_len..self.remainder_len + take]
                .copy_from_slice(&self.bytes[..take]);
            self.remainder_len += take;
            offset = take;
            if self.remainder_len == BYTES_PER_SAMPLE {
                self.samples.push(f32::from_le_bytes(self.remainder));
                self.remainder_len = 0;
            }
        }

        let body = &self.bytes[offset..read];
        let mut chunks = body.chunks_exact(BYTES_PER_SAMPLE);
        for chunk in chunks.by_ref() {
            let mut word = [0u8; BYTES_PER_SAMPLE];
            word.copy_from_slice(chunk);
            self.samples.push(f32::from_le_bytes(word));
        }

        let tail = chunks.remainder();
        if !tail.is_empty() {
            self.remainder[..tail.len()].copy_from_slice(tail);
            self.remainder_len = tail.len();
        }

        self.total_samples += self.samples.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// A reader that hands out at most `chunk` bytes per call, to force every possible
    /// split point across a sample boundary.
    struct ChunkedReader<'a> {
        data: &'a [u8],
        position: usize,
        chunk: usize,
    }

    impl Read for ChunkedReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let remaining = self.data.len() - self.position;
            let n = remaining.min(self.chunk).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.position..self.position + n]);
            self.position += n;
            Ok(n)
        }
    }

    fn encode(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn drain<R: Read>(mut framer: Framer<R>) -> Result<Vec<f32>, FramerError> {
        let mut out = Vec::new();
        loop {
            match framer.read_batch()? {
                Framed::Batch => out.extend_from_slice(framer.samples()),
                Framed::WouldBlock => continue,
                Framed::Eof => return Ok(out),
            }
        }
    }

    #[test]
    fn known_bytes_decode_to_known_floats() {
        // 1.0f32 == 0x3F800000, -2.0f32 == 0xC0000000, little-endian on the wire.
        let bytes = [0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x00, 0xC0];
        let framer = Framer::new(&bytes[..]);
        assert_eq!(drain(framer).unwrap(), vec![1.0, -2.0]);
    }

    #[test]
    fn decoding_is_independent_of_chunk_boundaries() {
        let values: Vec<f32> = (0..257).map(|i| i as f32 * -0.375).collect();
        let bytes = encode(&values);
        // Every chunk size from one byte per call up past the buffer size, and every
        // buffer size from one sample to several, exercises all four split offsets.
        for chunk in 1..=13 {
            for buffer in [4, 5, 7, 8, 64, 4096] {
                let reader = ChunkedReader {
                    data: &bytes,
                    position: 0,
                    chunk,
                };
                let framer = Framer::with_capacity(reader, buffer);
                assert_eq!(
                    drain(framer).unwrap(),
                    values,
                    "chunk={chunk} buffer={buffer}"
                );
            }
        }
    }

    #[test]
    fn one_byte_at_a_time_is_handled() {
        let values = vec![3.5_f32, -1.25, 1e-9, 6.0e20];
        let bytes = encode(&values);
        let reader = ChunkedReader {
            data: &bytes,
            position: 0,
            chunk: 1,
        };
        assert_eq!(drain(Framer::new(reader)).unwrap(), values);
    }

    #[test]
    fn trailing_partial_sample_is_retained_across_reads() {
        let bytes = [0x00, 0x00, 0x80];
        let mut framer = Framer::with_capacity(&bytes[..], 4);
        assert_eq!(framer.read_batch().unwrap(), Framed::Batch);
        assert!(framer.samples().is_empty());
        assert_eq!(framer.pending_bytes(), 3);
    }

    #[test]
    fn close_mid_sample_is_a_truncation_error() {
        let bytes = [0x00, 0x00, 0x80, 0x3F, 0x11, 0x22];
        let framer = Framer::with_capacity(&bytes[..], 4);
        let err = drain(framer).unwrap_err();
        match err {
            FramerError::TruncatedSample { leftover } => assert_eq!(leftover, 2),
            other => panic!("expected truncation, got {other}"),
        }
    }

    #[test]
    fn clean_close_on_a_sample_boundary_is_eof() {
        let mut framer = Framer::new(&[][..]);
        assert_eq!(framer.read_batch().unwrap(), Framed::Eof);
    }

    #[test]
    fn would_block_is_reported_without_losing_state() {
        struct Blocking {
            served: bool,
        }
        impl Read for Blocking {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                if self.served {
                    return Ok(0);
                }
                self.served = true;
                Err(std::io::Error::new(ErrorKind::WouldBlock, "timeout"))
            }
        }
        let mut framer = Framer::new(Blocking { served: false });
        assert_eq!(framer.read_batch().unwrap(), Framed::WouldBlock);
        assert_eq!(framer.read_batch().unwrap(), Framed::Eof);
    }

    #[test]
    fn interrupted_reads_are_retried() {
        struct Interrupting {
            calls: usize,
        }
        impl Read for Interrupting {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.calls += 1;
                if self.calls == 1 {
                    return Err(std::io::Error::new(ErrorKind::Interrupted, "signal"));
                }
                let bytes = 7.0_f32.to_le_bytes();
                buf[..4].copy_from_slice(&bytes);
                Ok(4)
            }
        }
        let mut framer = Framer::new(Interrupting { calls: 0 });
        assert_eq!(framer.read_batch().unwrap(), Framed::Batch);
        assert_eq!(framer.samples(), &[7.0]);
    }

    #[test]
    fn other_io_errors_surface() {
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(ErrorKind::ConnectionReset, "reset"))
            }
        }
        let mut framer = Framer::new(Failing);
        assert!(matches!(framer.read_batch(), Err(FramerError::Io { .. })));
    }

    #[test]
    fn large_stream_throughput_sanity() {
        let values: Vec<f32> = (0..200_000).map(|i| (i % 1000) as f32).collect();
        let bytes = encode(&values);
        let framer = Framer::new(&bytes[..]);
        let decoded = drain(framer).unwrap();
        assert_eq!(decoded.len(), values.len());
        assert_eq!(decoded[199_999], 999.0);
    }

    #[test]
    fn total_samples_accumulates() {
        let values = vec![1.0_f32; 10];
        let bytes = encode(&values);
        let reader = ChunkedReader {
            data: &bytes,
            position: 0,
            chunk: 3,
        };
        let mut framer = Framer::with_capacity(reader, 8);
        while framer.read_batch().unwrap() != Framed::Eof {}
        assert_eq!(framer.total_samples(), 10);
    }
}
