use fshell_core::{FxIndexMap, Val};
use ustr::ustr;

/// Build a `Val::Map` with `name` and `size` keys — shorthand for file entries
/// used in pipeline operator tests.
pub fn mk_file(name: &str, size: i64) -> Val {
    let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(ustr("name"), Val::String(name.into()));
    m.insert(ustr("size"), Val::Int(size));
    Val::Map(m)
}

/// Build a `Val::Map` with `name`, `ext`, and `size` keys.
pub fn mk_file_with_ext(name: &str, ext: &str, size: i64) -> Val {
    let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(ustr("name"), Val::String(name.into()));
    m.insert(ustr("ext"), Val::String(ext.into()));
    m.insert(ustr("size"), Val::Int(size));
    Val::Map(m)
}

/// A standard list of three file entries for pipeline tests.
pub fn make_file_items() -> Val {
    Val::List(vec![
        mk_file("file1.txt", 50),
        mk_file("file2.txt", 150),
        mk_file("file3.txt", 300),
    ])
}

/// Build a simple user entry map: name, age, city.
pub fn mk_user(name: &str, age: i64, city: &str) -> Val {
    let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(ustr("name"), Val::String(name.into()));
    m.insert(ustr("age"), Val::Int(age));
    m.insert(ustr("city"), Val::String(city.into()));
    Val::Map(m)
}

/// Setup an isolated configuration directory with process lock and `FSH_CONFIG_DIR` set.
pub fn setup_config_test() -> (
    tempfile::TempDir,
    String,
    super::guard::ProcessLockGuard<'static>,
) {
    let guard = super::guard::ProcessLockGuard::acquire();
    let tmp = tempfile::tempdir().expect("failed to create temp dir for config test");
    let config_dir = tmp.path().join(".config/fsh");
    std::fs::create_dir_all(&config_dir).expect("failed to create .config/fsh dir");
    let orig = std::env::var("FSH_CONFIG_DIR").ok();
    fshell_core::set_var("FSH_CONFIG_DIR", &config_dir.to_string_lossy());
    (tmp, orig.unwrap_or_default(), guard)
}

/// Teardown the isolated configuration directory and restore `FSH_CONFIG_DIR`.
pub fn teardown_config_test(orig: &str) {
    if orig.is_empty() {
        fshell_core::remove_var("FSH_CONFIG_DIR");
    } else {
        fshell_core::set_var("FSH_CONFIG_DIR", orig);
    }
}
