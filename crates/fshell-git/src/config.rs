// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::repo::{Error, Repository};
use gix::bstr::ByteSlice;
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Clone)]
pub struct Config {
    sections: HashMap<String, HashMap<String, String>>,
}

impl Config {
    pub fn parse(content: &str) -> Result<Self, Error> {
        let file = gix::config::File::try_from(content)
            .map_err(|error| Error::InvalidConfig(error.to_string()))?;
        let mut sections = HashMap::new();

        for section in file.sections() {
            let header = section.header();
            let section_name = header.name().to_str_lossy();
            let section_key = match header.subsection_name() {
                Some(subsection) => {
                    format!("{section_name}.{}", subsection.to_str_lossy())
                }
                None => section_name.into_owned(),
            };
            let values = sections.entry(section_key).or_insert_with(HashMap::new);
            let body = section.body();
            for key in body.value_names() {
                if let Some(value) = body.values(&key).last()
                    && let Ok(value) = value.to_str()
                {
                    values.insert(key, value.to_owned());
                }
            }
        }

        Ok(Config { sections })
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .get(section)
            .and_then(|values| values.get(key))
            .map(String::as_str)
    }
}

impl Repository {
    /// Return the repository's shared local configuration file.
    pub fn config(&self) -> Result<Config, Error> {
        let config_path = self
            .inner
            .config_path(gix::config::Source::Local)
            .map_err(|error| Error::Backend(error.to_string()))?;
        match fs::read_to_string(config_path) {
            Ok(content) => Config::parse(&content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Config::parse(""),
            Err(error) => Err(error.into()),
        }
    }

    /// Find the configured upstream's tracking ref and object id for a branch.
    pub fn find_upstream(&self, branch: &str) -> Result<Option<(String, [u8; 20])>, Error> {
        let branch_name = gix::refs::FullName::try_from(format!("refs/heads/{branch}"))
            .map_err(|error| Error::InvalidRef(error.to_string()))?;
        let Some(tracking_ref) = self
            .inner
            .branch_remote_tracking_ref_name(branch_name.as_ref(), gix::remote::Direction::Fetch)
        else {
            return Ok(None);
        };
        let tracking_ref = tracking_ref.map_err(|error| Error::Backend(error.to_string()))?;
        let remote = self
            .inner
            .config_snapshot()
            .string(&format!("branch.{branch}.remote"))
            .map(|value| value.to_str_lossy().into_owned())
            .unwrap_or_default();
        let tracking_ref_name = tracking_ref.as_ref().as_bstr().to_str_lossy();
        let oid = self.resolve_ref(&tracking_ref_name)?;
        Ok(Some((remote, oid)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_config_sections_comments_and_quoted_values() {
        let config = Config::parse(
            "# comment\n[core]\n\tbare = false\n[user]\n\tname = \"Jane Doe\"\n\tname = Updated\n[branch \"feature/topic\"]\n\tremote = origin\n\tmerge = refs/heads/feature/topic\n",
        )
        .expect("valid Git config should parse");

        assert_eq!(config.get("core", "bare"), Some("false"));
        assert_eq!(config.get("user", "name"), Some("Updated"));
        assert_eq!(config.get("branch.feature/topic", "remote"), Some("origin"));
        assert_eq!(config.get("missing", "key"), None);
    }
}
