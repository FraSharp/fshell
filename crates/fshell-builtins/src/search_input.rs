// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::search_output::Output;
use fshell_engine::{PipeStream, PipelinePayload};
use std::io::{self, Read};

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
