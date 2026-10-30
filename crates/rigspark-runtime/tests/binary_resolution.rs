use rigspark_runtime::process_control::{FALLBACK_BINARY_DIRS, resolve_binary_in};
use std::ffi::OsString;

fn executable(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"#!/bin/sh\n").unwrap();
    path.canonicalize().unwrap()
}

#[test]
fn path_wins_and_fixed_directories_are_only_a_fallback() {
    let on_path = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    let preferred = executable(on_path.path(), "ollama");
    let backup = executable(fallback.path(), "ollama");
    let path = std::env::join_paths([on_path.path()]).unwrap();
    assert_eq!(
        resolve_binary_in("ollama", &path, &[fallback.path()]).unwrap(),
        preferred
    );
    assert_eq!(
        resolve_binary_in(
            "ollama",
            &OsString::from("/usr/bin:/bin"),
            &[fallback.path()]
        )
        .unwrap(),
        backup,
        "an app launched from Finder has a minimal PATH"
    );
    assert!(resolve_binary_in("ollama", &OsString::new(), &[]).is_err());
    assert!(resolve_binary_in("missing-runtime", &path, &[fallback.path()]).is_err());
}

#[test]
fn fallback_directories_are_absolute_and_fixed() {
    assert!(!FALLBACK_BINARY_DIRS.is_empty());
    for dir in FALLBACK_BINARY_DIRS {
        assert!(std::path::Path::new(dir).is_absolute(), "{dir}");
        assert!(!dir.contains(".."), "{dir}");
    }
}
