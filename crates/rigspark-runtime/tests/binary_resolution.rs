use rigspark_runtime::process_control::{
    FALLBACK_BINARY_DIRS, fallback_directories, resolve_binary_in,
};
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
    // Windows has no package-manager directories outside PATH to add.
    assert_eq!(
        FALLBACK_BINARY_DIRS.is_empty(),
        cfg!(not(any(target_os = "macos", target_os = "linux")))
    );
    for dir in FALLBACK_BINARY_DIRS {
        assert!(std::path::Path::new(dir).is_absolute(), "{dir}");
        assert!(!dir.contains(".."), "{dir}");
    }
}

#[test]
fn only_bundled_runtimes_fall_back_to_the_app_bundle_after_user_installs() {
    let homebrew = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let bundled = executable(bundle.path(), "llama-server");
    executable(bundle.path(), "git");
    let dirs = fallback_directories("llama-server", &[homebrew.path()], Some(bundle.path()));
    assert_eq!(dirs, [homebrew.path(), bundle.path()]);
    assert_eq!(
        resolve_binary_in("llama-server", &OsString::from("/usr/bin:/bin"), &dirs).unwrap(),
        bundled
    );
    let user = executable(homebrew.path(), "llama-server");
    assert_eq!(
        resolve_binary_in("llama-server", &OsString::from("/usr/bin:/bin"), &dirs).unwrap(),
        user,
        "a user-installed llama.cpp wins over the bundled one"
    );
    for other in ["git", "ollama", "python3"] {
        assert_eq!(
            fallback_directories(other, &[homebrew.path()], Some(bundle.path())),
            [homebrew.path()],
            "{other} must never resolve from the app bundle"
        );
    }
    assert_eq!(
        fallback_directories(
            "llama-server",
            &[],
            Some(std::path::Path::new("relative/bin"))
        ),
        Vec::<&std::path::Path>::new()
    );
}

#[test]
fn the_cli_keeps_its_path_exactly_until_an_app_opts_in() {
    // Process-wide flag: this test binary never enables it, so PATH stays authoritative.
    let missing = format!("rigspark-test-missing-{}", std::process::id());
    assert!(rigspark_runtime::process_control::resolve_binary(&missing).is_err());
    assert!(!rigspark_runtime::process_control::resolves_to_bundled(
        "llama-server"
    ));
}
