mod common;
use common::*;
use fshell_capabilities::CapsRegistry;
use fshell_core::ResourceHandle;
use fshell_engine::run_script;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// 1. Strict Mode Denial & Explicit Capability Grants
// ---------------------------------------------------------------------------

#[test]
fn test_strict_mode_denies_all_by_default() {
    let mut registry = CapsRegistry::new();
    registry.strict_mode = true;

    let target_dir = PathBuf::from("/var/log");
    let target_file = PathBuf::from("/etc/passwd");

    // Denied without explicit grant
    assert!(!registry.check_read_dir(&target_dir));
    assert!(!registry.check_write_dir(&target_dir));
    assert!(!registry.check_read_file(&target_file));
    assert!(!registry.check_write_file(&target_file));
    assert!(!registry.check_process_spawn("ls"));

    // Explicitly grant read access
    registry.grant(ResourceHandle::ReadDir(target_dir.clone()));
    registry.grant(ResourceHandle::ReadFile(target_file.clone()));

    assert!(registry.check_read_dir(&target_dir));
    assert!(registry.check_read_file(&target_file));
    // Write access is still denied
    assert!(!registry.check_write_dir(&target_dir));
    assert!(!registry.check_write_file(&target_file));
}

#[test]
fn test_explicit_denial_overrides_held_grant() {
    let mut registry = CapsRegistry::new();
    let target = PathBuf::from("/home/user/.ssh");

    // Grant read then deny it
    registry.grant(ResourceHandle::ReadDir(target.clone()));
    assert!(registry.check_read_dir(&target));

    registry.deny(ResourceHandle::ReadDir(target.clone()));
    // Deny-first rule: denied set always takes precedence
    assert!(!registry.check_read_dir(&target));
}

// ---------------------------------------------------------------------------
// 2. Unforgeable Descriptor-Passing Capabilities (CapFile & CapDir)
// ---------------------------------------------------------------------------

#[test]
fn test_cap_file_read_write_enforcement() {
    let temp_dir = tempfile::tempdir().unwrap();
    let test_file = temp_dir.path().join("secure_document.txt");
    std::fs::write(&test_file, "initial secret content").unwrap();

    let mut registry = CapsRegistry::new();
    registry.grant(ResourceHandle::ReadFile(temp_dir.path().to_path_buf()));
    registry.grant(ResourceHandle::WriteFile(temp_dir.path().to_path_buf()));

    // Open CapFile through CapsRegistry with read + write capabilities
    let mut cap_rw = registry
        .open_file(&test_file, true, true, false, false, false)
        .unwrap();

    let content = cap_rw.read_to_string().unwrap();
    assert_eq!(content, "initial secret content");

    cap_rw.write_all(b"updated secret content").unwrap();
    assert_eq!(
        std::fs::read_to_string(&test_file).unwrap(),
        "initial secret contentupdated secret content"
    );

    // Open CapFile as read-only (write: false)
    let mut cap_ro = registry
        .open_file(&test_file, true, false, false, false, false)
        .unwrap();

    let ro_content = cap_ro.read_to_string().unwrap();
    assert_eq!(ro_content, "initial secret contentupdated secret content");

    // Writing through read-only handle must fail with PermissionDenied
    let write_err = cap_ro.write_all(b"illegal write attempt");
    assert!(write_err.is_err());
    assert_eq!(
        write_err.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[test]
fn test_cap_dir_relative_file_access_and_scoping() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sub_dir = temp_dir.path().join("sub_workspace");
    std::fs::create_dir(&sub_dir).unwrap();
    let nested_file = sub_dir.join("config.json");
    std::fs::write(&nested_file, "{\"env\": \"test\"}").unwrap();

    let mut registry = CapsRegistry::new();
    registry.grant(ResourceHandle::ReadDir(temp_dir.path().to_path_buf()));
    registry.grant(ResourceHandle::ReadFile(temp_dir.path().to_path_buf()));

    let cap_dir = registry.acquire_dir(temp_dir.path(), true, false).unwrap();

    // List directory through capability
    let entries = cap_dir.read_dir().unwrap();
    let mut names = Vec::new();
    for entry in entries {
        if let Ok(e) = entry {
            names.push(e.file_name().to_string_lossy().to_string());
        }
    }
    assert!(names.contains(&"sub_workspace".to_string()));

    // Open file relatively through CapDir
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    let mut cap_file = cap_dir.open_file(std::path::Path::new("sub_workspace/config.json"), &opts).unwrap();
    assert_eq!(cap_file.read_to_string().unwrap(), "{\"env\": \"test\"}");
}

// ---------------------------------------------------------------------------
// 3. Integration with Env & with caps(...) Scoping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_with_caps_block_scoped_privilege_elevation() {
    let env = setup_test_env();

    // Grant a capability into a variable
    env.vars.write().insert(
        "net_cap".to_string(),
        fshell_core::Val::Capability(ResourceHandle::NetworkAll),
    );

    assert!(!env.caps.caps.read().check_network("example.com"));

    let script = r#"
with caps($net_cap) {
    let inside_caps = true
}
let outside_caps = true
"#;

    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("inside_caps"), Some(&fshell_core::Val::Bool(true)));
    assert_eq!(vars.get("outside_caps"), Some(&fshell_core::Val::Bool(true)));

    // After with caps block, NetworkAll capability is not retained
    assert!(!env.caps.caps.read().check_network("example.com"));
}
