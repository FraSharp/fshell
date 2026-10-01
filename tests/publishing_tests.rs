//! Release metadata guards: what the workspace must satisfy to publish to
//! crates.io as a family of crates.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

fn manifest_path(member: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(member)
        .join("Cargo.toml")
}

fn read_manifest(member: &str) -> DocumentMut {
    fs::read_to_string(manifest_path(member))
        .unwrap_or_else(|error| panic!("{member}/Cargo.toml could not be read: {error}"))
        .parse()
        .unwrap_or_else(|error| panic!("{member}/Cargo.toml is malformed: {error}"))
}

fn inherits_workspace(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|table| table.get("workspace"))
        .and_then(|value| value.as_bool())
        == Some(true)
}

/// Every intra-workspace dependency must carry the same version as
/// `[workspace.package]`, otherwise a release would publish crates that depend
/// on an older published version of their siblings.
#[test]
fn dependency_versions_match_the_workspace_version() {
    let root = read_manifest(".");
    let version = root["workspace"]["package"]["version"]
        .as_str()
        .expect("[workspace.package] must declare a version");
    let dependencies = root["workspace"]["dependencies"]
        .as_table()
        .expect("[workspace.dependencies] must be a table");

    for (name, item) in dependencies {
        let Some(spec) = item.as_table_like() else {
            continue;
        };
        if spec.get("path").is_none() {
            continue;
        }
        let declared = spec
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or_else(|| {
                panic!("`{name}` is a path dependency without a version and cannot be published")
            });
        assert_eq!(
            declared, version,
            "`{name}` must track the [workspace.package] version"
        );
    }
}

/// Root-package `include` patterns must be anchored to the package root:
/// gitignore-style patterns without a leading `/` match at any depth, so a
/// bare `LICENSE` or `README.md` would drag unrelated files into the crate.
#[test]
fn root_package_include_patterns_are_anchored() {
    let root = read_manifest(".");
    let include = root["package"]["include"]
        .as_array()
        .expect("[package] must declare an include list");

    for pattern in include {
        let pattern = pattern.as_str().expect("include patterns must be strings");
        assert!(
            pattern.starts_with('/'),
            "include pattern `{pattern}` must start with `/`"
        );
    }
}

/// Every published crate declares its version and MSRV by inheriting the
/// workspace fields, and ships the license text alongside its source.
#[test]
fn published_crates_inherit_workspace_metadata_and_ship_the_license() {
    let root = read_manifest(".");
    let mut members = vec![".".to_string()];
    if let Some(array) = root["workspace"]["members"].as_array() {
        members.extend(
            array
                .iter()
                .filter_map(|member| member.as_str().map(str::to_string)),
        );
    }

    for member in members {
        let manifest = read_manifest(&member);
        let unpublished = manifest["package"]
            .get("publish")
            .and_then(|item| item.as_bool())
            == Some(false);
        if unpublished {
            continue;
        }
        for field in ["version", "rust-version"] {
            assert!(
                inherits_workspace(&manifest["package"][field]),
                "{member}/Cargo.toml must inherit `{field}` from the workspace"
            );
        }
        assert!(
            manifest_path(&member).with_file_name("LICENSE").exists(),
            "{member}/LICENSE must exist so published tarballs carry the license"
        );
    }
}
