// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! `json`: parse JSON input and select values with a path query.
//!
//! Input may be a whole document, line-delimited values, or already-typed
//! pipeline values. The query is a jq-like path: `.name`, `.a.b`, `[0]`,
//! `[-1]`, `[]` (iterate), and combinations such as `.users[0].name` or
//! `.items[]`. A path that does not exist selects `null` rather than failing,
//! so probing optional fields is safe.

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::json::{StreamDecoder, send_value};
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;

/// Stands in for a path that does not exist.
const NULL: serde_json::Value = serde_json::Value::Null;

/// One step of a query path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// `.name`
    Member(String),
    /// `[n]`; negative counts from the end.
    Index(i64),
    /// `[]`
    Iterate,
}

/// A parsed path query.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Query {
    steps: Vec<Step>,
}

impl Query {
    /// Parse `.a.b[0][]`-style expressions. A leading `.` is optional, and an
    /// empty query (`""` or `"."`) selects whole documents.
    fn parse(text: &str) -> Result<Self, String> {
        let mut steps = Vec::new();
        let mut rest = text.strip_prefix('.').unwrap_or(text);
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('[') {
                let (inner, tail) = after
                    .split_once(']')
                    .ok_or_else(|| format!("json: unterminated '[' in query '{text}'"))?;
                if inner.trim().is_empty() {
                    steps.push(Step::Iterate);
                } else {
                    let index: i64 = inner.trim().parse().map_err(|_| {
                        format!("json: invalid index '[{inner}]' in query '{text}'")
                    })?;
                    steps.push(Step::Index(index));
                }
                rest = tail;
            } else {
                let end = rest.find(['.', '[']).unwrap_or(rest.len());
                let name = &rest[..end];
                if name.is_empty() || name.contains(']') {
                    return Err(format!("json: malformed query '{text}'"));
                }
                steps.push(Step::Member(name.to_string()));
                rest = &rest[end..];
            }
            if let Some(after) = rest.strip_prefix('.') {
                rest = after;
                if rest.is_empty() || rest.starts_with(['.', ']']) {
                    return Err(format!("json: malformed query '{text}'"));
                }
            }
        }
        Ok(Self { steps })
    }

    /// Select every value the path reaches in `document`.
    fn select<'a>(&self, document: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
        let mut current = vec![document];
        for step in &self.steps {
            let mut next = Vec::new();
            for value in current {
                match step {
                    Step::Member(name) => match value {
                        serde_json::Value::Object(map) => next.push(map.get(name).unwrap_or(&NULL)),
                        _ => next.push(&NULL),
                    },
                    Step::Index(index) => match value {
                        serde_json::Value::Array(items) => {
                            let resolved = if *index < 0 {
                                items.len() as i64 + index
                            } else {
                                *index
                            };
                            if resolved < 0 {
                                next.push(&NULL);
                            } else {
                                next.push(items.get(resolved as usize).unwrap_or(&NULL));
                            }
                        }
                        _ => next.push(&NULL),
                    },
                    Step::Iterate => match value {
                        serde_json::Value::Array(items) => next.extend(items.iter()),
                        serde_json::Value::Object(map) => next.extend(map.values()),
                        _ => next.push(&NULL),
                    },
                }
            }
            current = next;
        }
        out.extend(current);
    }
}

pub fn json_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let query = match args.as_slice() {
        [] => None,
        [Val::String(text)] => Some(
            Query::parse(text)
                .map_err(|message| ShellError::new(ErrorCode::InvalidArgument, message))?,
        ),
        [other] => {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                format!("json: query must be a string, got {}", other.type_name()),
            ));
        }
        _ => {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                "json: expected at most one query expression".to_string(),
            ));
        }
    };

    let env = env.clone();
    tokio::spawn(async move {
        let Some(mut rx) = in_rx else {
            return;
        };
        let mut decoder = StreamDecoder::new();
        while let Some(payload) = rx.recv().await {
            match payload {
                PipelinePayload::Data(value) => match (*value).clone() {
                    Val::String(text) => {
                        if !feed(&mut decoder, &text, query.as_ref(), &env, &tx).await {
                            return;
                        }
                    }
                    Val::Blob(bytes) => match std::str::from_utf8(&bytes) {
                        Ok(text) => {
                            if !feed(&mut decoder, text, query.as_ref(), &env, &tx).await {
                                return;
                            }
                        }
                        Err(error) => {
                            report(
                                &env,
                                &tx,
                                format!("json: input is not valid UTF-8: {error}"),
                            )
                            .await;
                        }
                    },
                    // Already-typed values are queried directly.
                    other => {
                        if !select_into_pipeline((&other).into(), query.as_ref(), &tx).await {
                            return;
                        }
                    }
                },
                PipelinePayload::Bytes(bytes) => match std::str::from_utf8(&bytes) {
                    Ok(text) => {
                        if !feed(&mut decoder, text, query.as_ref(), &env, &tx).await {
                            return;
                        }
                    }
                    Err(error) => {
                        report(
                            &env,
                            &tx,
                            format!("json: input is not valid UTF-8: {error}"),
                        )
                        .await;
                    }
                },
                PipelinePayload::Structured(document) => {
                    let _ = tx.send(PipelinePayload::Structured(document)).await;
                }
            }
        }
        if let Err(message) = decoder.finish() {
            report(&env, &tx, message).await;
        }
    });
    Ok(())
}

/// Parse `text` and send every selected value. Returns false when the
/// receiving stage is gone.
async fn feed(
    decoder: &mut StreamDecoder,
    text: &str,
    query: Option<&Query>,
    env: &Env,
    tx: &PipeSender,
) -> bool {
    match decoder.push(text) {
        Ok(values) => {
            for value in values {
                if !select_into_pipeline(value, query, tx).await {
                    return false;
                }
            }
            true
        }
        Err(message) => {
            report(env, tx, message).await;
            true
        }
    }
}

/// Apply the query to one document and stream the results.
async fn select_into_pipeline(
    document: serde_json::Value,
    query: Option<&Query>,
    tx: &PipeSender,
) -> bool {
    match query {
        Some(query) => {
            let mut selected = Vec::new();
            query.select(&document, &mut selected);
            for value in selected {
                if !send_value(value.clone(), tx).await {
                    return false;
                }
            }
            true
        }
        None => send_value(document, tx).await,
    }
}

/// Report a stage failure the way the engine's boundary operators do.
async fn report(env: &Env, tx: &PipeSender, message: String) {
    env.report_stage_error();
    let _ = tx.send(PipelinePayload::Structured(message.into())).await;
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn select(query: &str, document: &str) -> Vec<serde_json::Value> {
        let query = Query::parse(query).unwrap();
        let value: serde_json::Value = serde_json::from_str(document).unwrap();
        let mut selected = Vec::new();
        query.select(&value, &mut selected);
        selected.into_iter().cloned().collect()
    }

    #[test]
    fn member_index_and_iterate_paths() {
        let doc = r#"{"users":[{"name":"ada"},{"name":"bob"}]}"#;
        assert_eq!(
            select(".users[0].name", doc),
            vec![serde_json::json!("ada")]
        );
        assert_eq!(select("users[1].name", doc), vec![serde_json::json!("bob")]);
        assert_eq!(
            select(".users[-1].name", doc),
            vec![serde_json::json!("bob")]
        );
        assert_eq!(
            select(".users[].name", doc),
            vec![serde_json::json!("ada"), serde_json::json!("bob")]
        );
        assert_eq!(select(".users[]", doc).len(), 2);
    }

    #[test]
    fn missing_paths_select_null() {
        assert_eq!(
            select(".missing", r#"{"a":1}"#),
            vec![serde_json::Value::Null]
        );
        assert_eq!(
            select(".a[3]", r#"{"a":[1]}"#),
            vec![serde_json::Value::Null]
        );
        assert_eq!(select(".a.b", r#"{"a":1}"#), vec![serde_json::Value::Null]);
        assert_eq!(select("[]", "5"), vec![serde_json::Value::Null]);
    }

    #[test]
    fn identity_selects_the_document() {
        assert_eq!(select(".", r#"{"a":1}"#), vec![serde_json::json!({"a": 1})]);
        assert_eq!(select("", "3"), vec![serde_json::json!(3)]);
    }

    #[test]
    fn malformed_queries_are_rejected() {
        assert!(Query::parse(".a[").is_err());
        assert!(Query::parse("..a").is_err());
        assert!(Query::parse("[x]").is_err());
    }
}
