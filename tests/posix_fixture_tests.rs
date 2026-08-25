mod common;
use common::*;
use std::path::PathBuf;

#[tokio::test]
async fn test_posix_fixtures_in_process() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/posix");
    let fixtures = FixtureSuite::discover(&fixture_dir).expect("failed to discover fixtures");

    assert!(!fixtures.is_empty(), "expected to find POSIX fixtures");

    for fixture in &fixtures {
        fixture.run_in_process().await;
    }
}

#[test]
fn test_posix_fixtures_as_subprocesses() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/posix");
    let fixtures = FixtureSuite::discover(&fixture_dir).expect("failed to discover fixtures");

    assert!(!fixtures.is_empty(), "expected to find POSIX fixtures");

    for fixture in &fixtures {
        fixture.run_as_subprocess();
    }
}
