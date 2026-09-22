//! Loader for the semantic evaluation corpus.
//!
//! Each case pairs a natural-language prompt with the semantic outcome we expect a
//! function-calling model to produce. The corpus is the contract that will later be used
//! to evaluate a stock (and then fine-tuned) FunctionGemma: it is intentionally tiny and
//! hand-written for now.

use std::path::{Path, PathBuf};

/// One prompt → expected-semantics pair.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SemanticCase {
    /// Stable identifier (also the file stem by convention).
    pub id: String,
    /// The natural-language request.
    pub prompt: String,
    /// The expected `Intent` (or bare `Action`) as JSON.
    pub expected: serde_json::Value,
    /// Parameter names expected to be reported as incomplete after validation.
    #[serde(default)]
    pub expect_issues: Vec<String>,
    /// Whether the request is expected to be ambiguous between several readings.
    #[serde(default)]
    pub ambiguous: bool,
}

impl SemanticCase {
    /// Parse a single case value.
    fn from_value(value: serde_json::Value) -> std::io::Result<Self> {
        serde_json::from_value(value).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })
    }

    /// Parse a case file. A file may contain a single case object or an array of cases.
    pub fn parse(path: &Path) -> std::io::Result<Vec<Self>> {
        let content = std::fs::read_to_string(path)?;
        let value: serde_json::Value = serde_json::from_str(&content).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        })?;
        match value {
            serde_json::Value::Array(items) => items.into_iter().map(Self::from_value).collect(),
            other => Ok(vec![Self::from_value(other)?]),
        }
    }

    /// Discover every `*.json` case in a directory, sorted by name.
    pub fn discover(dir: &Path) -> std::io::Result<Vec<Self>> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        let mut cases = Vec::new();
        for path in &paths {
            cases.extend(Self::parse(path)?);
        }
        Ok(cases)
    }

    /// The expected intent, deserialized.
    pub fn intent(&self) -> fshell_semantic::Intent {
        serde_json::from_value(self.expected.clone())
            .expect("semantic fixture must contain a valid Intent")
    }
}
