use rigspark_core::catalog::Catalog;
use std::{fs, path::Path, process::Command};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-bootstrap"));
    command
        .current_dir(root)
        .env("PATH", "")
        .env("RIGSPARK_HOME", root.join("unused-home"));
    command
}

#[test]
fn native_bootstrap_is_offline_deterministic_and_matches_oracle() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("crates/rigspark-core/data")).unwrap();
    let path = root.path().join("crates/rigspark-core/data/models.json");
    let output = command(root.path()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("bootstrap: wrote 70 models to "));
    let first = fs::read_to_string(&path).unwrap();
    let expected = Catalog::parse(include_str!(
        "../../rigspark-core/fixtures/bootstrap-oracle.json"
    ))
    .unwrap();
    let mut historical = Catalog::parse(&first).unwrap();
    assert!(
        historical
            .models
            .iter()
            .any(|model| model.id == "qwen3.6:35b")
    );
    assert!(
        historical
            .models
            .iter()
            .any(|model| model.id == "qwen3.5:4b")
    );
    assert!(
        historical
            .models
            .iter()
            .any(|model| model.id == "bonsai:8b")
    );
    historical.models.retain(|model| {
        !matches!(
            model.id.as_str(),
            "qwen3.6:35b" | "qwen3.5:4b" | "bonsai:8b" | "bitnet-b1.58:2b"
        )
    });
    assert_eq!(
        serde_json::to_value(historical).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert!(first.ends_with('\n'));
    assert!(command(root.path()).output().unwrap().status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), first);
    assert!(!root.path().join("unused-home").exists());
}

#[test]
fn preview_never_writes_and_invalid_options_preserve_existing_data() {
    let root = tempfile::tempdir().unwrap();
    let output = command(root.path()).arg("--dry-run").output().unwrap();
    assert!(output.status.success());
    Catalog::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert!(!root.path().join("crates/rigspark-core/data").exists());
    let path = root.path().join("catalog.json");
    fs::write(&path, "original").unwrap();
    for args in [vec!["--unknown"], vec!["--now", "invalid"]] {
        let output = command(root.path())
            .args(["--out", "catalog.json"])
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    }
    let output = command(root.path())
        .args(["--out", "catalog.json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    Catalog::parse(&fs::read_to_string(path).unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_and_preserves_existing_permissions() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target.json");
    fs::write(&target, "untouched").unwrap();
    symlink(&target, root.path().join("link.json")).unwrap();
    assert!(
        !command(root.path())
            .args(["--out", "link.json"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(
        command(root.path())
            .args(["--out", "target.json"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    fs::create_dir(root.path().join("directory")).unwrap();
    assert!(
        !command(root.path())
            .args(["--out", "directory"])
            .output()
            .unwrap()
            .status
            .success()
    );
}
