// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::search_output::Output;
use fshell_engine::{PipeStream, PipelinePayload};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, BorrowedFd};

/// Read stdin without getting stuck in a blocking read when Ctrl+C cancels the
/// pipeline. Tokio handles SIGINT, so stdio's EINTR retries are not sufficient.
/// Borrow the descriptor rather than spawning a reader thread that could keep
/// consuming terminal input after the command has returned.
pub(crate) struct InterruptibleReader<'a> {
    fd: BorrowedFd<'a>,
    output: &'a Output,
}

impl<'a> InterruptibleReader<'a> {
    pub fn new(fd: BorrowedFd<'a>, output: &'a Output) -> Self {
        Self { fd, output }
    }
}

impl Read for InterruptibleReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if self.output.stopped() {
                return Ok(0);
            }
            let mut pollfd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: pollfd is valid for one descriptor for the duration of poll.
            let ready = unsafe { libc::poll(&mut pollfd, 1, 50) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready == 0 || self.output.stopped() {
                continue;
            }
            // Read the fd directly: stdio can retry EINTR internally, blocking
            // again without letting us check cancellation.
            // SAFETY: the borrowed fd is live and buffer is writable for its length.
            let count = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if count >= 0 {
                return Ok(count as usize);
            }
            let error = io::Error::last_os_error();
            if matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            ) {
                continue;
            }
            return Err(error);
        }
    }
}

/// A contiguous byte segment of a mixed typed pipeline, exposed directly to
/// the buffered content scanner. A record ends the byte segment and is retained
/// for the record adapter; it is never stringified or swallowed. Diagnostics
/// retain their independent transport while reading.
pub(crate) struct ByteSegment<'a> {
    input: &'a mut PipeStream,
    output: &'a Output,
    pending: &'a mut Option<PipelinePayload>,
    chunk: bytes::Bytes,
    position: usize,
}

impl<'a> ByteSegment<'a> {
    pub fn new(
        input: &'a mut PipeStream,
        output: &'a Output,
        pending: &'a mut Option<PipelinePayload>,
        chunk: bytes::Bytes,
    ) -> Self {
        Self {
            input,
            output,
            pending,
            chunk,
            position: 0,
        }
    }
}

impl Read for ByteSegment<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.output.stopped() {
            return Ok(0);
        }
        while self.position == self.chunk.len() {
            match self.output.receive(self.input) {
                Some(PipelinePayload::Bytes(bytes)) => {
                    self.chunk = bytes;
                    self.position = 0;
                }
                Some(PipelinePayload::Structured(diag)) => {
                    if !self.output.send(PipelinePayload::Structured(diag)) {
                        return Ok(0);
                    }
                }
                Some(record) => {
                    *self.pending = Some(record);
                    return Ok(0);
                }
                None => return Ok(0),
            }
        }
        let count = buffer.len().min(self.chunk.len() - self.position);
        buffer[..count].copy_from_slice(&self.chunk[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}
