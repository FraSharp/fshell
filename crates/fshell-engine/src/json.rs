// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Document-aware JSON decoding shared by `@json` and the `json` builtin.
//!
//! Raw process output arrives in arbitrary text chunks — one line at a time
//! for a byte stream — so a chunk is rarely a complete JSON value on its own.
//! [`StreamDecoder`] buffers text until a value parses, emits every value as
//! soon as it is complete (which keeps line-delimited input streaming), and
//! fails on text that cannot be JSON at all.

use std::sync::Arc;

use tokio::sync::mpsc::Sender;

use crate::{PipelinePayload, Val};

/// A JSON decoder over text pushed in arbitrary chunks.
#[derive(Debug, Default)]
pub struct StreamDecoder {
    buffer: String,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one text chunk, returning every value it completed.
    ///
    /// A chunk that leaves an incomplete tail keeps it buffered for the next
    /// call. Text that cannot be JSON at all clears the buffer and returns
    /// the parse error.
    pub fn push(&mut self, text: &str) -> Result<Vec<serde_json::Value>, String> {
        self.buffer.push_str(text);
        let mut values = Vec::new();
        let mut consumed = 0;
        let mut error = None;
        {
            let mut stream =
                serde_json::Deserializer::from_str(&self.buffer).into_iter::<serde_json::Value>();
            while let Some(item) = stream.next() {
                match item {
                    Ok(value) => {
                        consumed = stream.byte_offset();
                        values.push(value);
                    }
                    Err(parse_error) => {
                        error = Some(parse_error);
                        break;
                    }
                }
            }
        }
        self.buffer.drain(..consumed);
        if let Some(parse_error) = error
            && !parse_error.is_eof()
        {
            self.buffer.clear();
            return Err(format!("JSON parse error: {parse_error}"));
        }
        Ok(values)
    }

    /// Finish the stream. A remaining non-blank buffer is an incomplete
    /// document, and therefore an error.
    pub fn finish(&mut self) -> Result<(), String> {
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return Ok(());
        }
        let detail = serde_json::from_str::<serde_json::Value>(&self.buffer)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unexpected end of input".to_string());
        self.buffer.clear();
        Err(format!("JSON parse error: {detail}"))
    }
}

/// Send one decoded value to a pipeline. A top-level array becomes one item
/// per element, matching how the shell's builtins stream records. Returns
/// false when the receiving stage is gone.
pub async fn send_value(value: serde_json::Value, tx: &Sender<PipelinePayload>) -> bool {
    match Val::from(value) {
        Val::List(items) => {
            for item in items {
                if tx
                    .send(PipelinePayload::Data(Arc::new(item)))
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            true
        }
        other => tx
            .send(PipelinePayload::Data(Arc::new(other)))
            .await
            .is_ok(),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn documents_span_chunks() {
        let mut decoder = StreamDecoder::new();
        assert!(decoder.push("{\n  \"a\": 5\n").unwrap().is_empty());
        let values = decoder.push("}\n").unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["a"], serde_json::json!(5));
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn line_delimited_values_stream_immediately() {
        let mut decoder = StreamDecoder::new();
        let values = decoder.push("{\"a\": 1}\n{\"a\": 2}").unwrap();
        assert_eq!(values.len(), 2);
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn incomplete_input_fails_at_finish_only() {
        let mut decoder = StreamDecoder::new();
        assert!(decoder.push("{\"a\": ").unwrap().is_empty());
        assert!(decoder.finish().is_err());
    }

    #[test]
    fn invalid_input_reports_immediately_and_recovers() {
        let mut decoder = StreamDecoder::new();
        let error = decoder.push("not json").unwrap_err();
        assert!(error.contains("JSON parse error"), "{error}");
        assert_eq!(decoder.push("[1]").unwrap().len(), 1);
        assert!(decoder.finish().is_ok());
    }
}
