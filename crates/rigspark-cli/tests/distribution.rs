use rigspark_cli::distribution::{package_directory, verify_directory};
use std::fs;

fn executable_fixture(path: &std::path::Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn unix_binary_execute_permissions_are_required_before_and_after_packaging() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::write(&source, b"native fixture").unwrap();
    let files = [
        ("llmup", source.as_path()),
        ("rigspark", source.as_path()),
        ("rigspark-gui", source.as_path()),
    ];
    let output = directory.path().join("package");
    assert!(
        package_directory(
            &output,
            env!("CARGO_PKG_VERSION"),
            "aarch64-apple-darwin",
            &files
        )
        .is_err()
    );
    assert!(!output.exists());
    executable_fixture(&source, b"native fixture");
    package_directory(
        &output,
        env!("CARGO_PKG_VERSION"),
        "aarch64-apple-darwin",
        &files,
    )
    .unwrap();
    for (name, _) in files {
        let path = output.join(name);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            verify_directory(&output).is_err(),
            "accepted non-executable {name}"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        verify_directory(&output).unwrap();
    }
}

#[test]
fn verification_rejects_manifest_size_hash_layout_and_unlisted_file_changes() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    executable_fixture(&source, b"native fixture");
    let output = directory.path().join("package");
    package_directory(
        &output,
        env!("CARGO_PKG_VERSION"),
        "aarch64-apple-darwin",
        &[
            ("llmup", source.as_path()),
            ("rigspark", source.as_path()),
            ("rigspark-gui", source.as_path()),
        ],
    )
    .unwrap();
    let path = output.join("manifest.json");
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for field in ["bytes", "sha256"] {
        let mut modified = original.clone();
        modified["files"][0][field] = if field == "bytes" {
            1.into()
        } else {
            "0".repeat(64).into()
        };
        fs::write(&path, serde_json::to_vec(&modified).unwrap()).unwrap();
        assert!(
            verify_directory(&output).is_err(),
            "accepted altered {field}"
        );
    }
    let mut modified = original.clone();
    modified["files"].as_array_mut().unwrap().pop();
    fs::write(&path, serde_json::to_vec(&modified).unwrap()).unwrap();
    fs::remove_file(output.join("rigspark-gui")).unwrap();
    assert!(verify_directory(&output).is_err());
    executable_fixture(&output.join("rigspark-gui"), b"native fixture");
    fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    verify_directory(&output).unwrap();
    fs::write(output.join("launcher.js"), b"unexpected").unwrap();
    assert!(verify_directory(&output).is_err());
}

#[test]
fn complete_distribution_requires_exactly_the_target_binary_trio() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::write(&source, b"native fixture").unwrap();
    for (target, required, foreign) in [
        (
            "aarch64-apple-darwin",
            ["llmup", "rigspark", "rigspark-gui"],
            ["llmup.exe", "rigspark.exe", "rigspark-gui.exe"],
        ),
        (
            "x86_64-pc-windows-msvc",
            ["llmup.exe", "rigspark.exe", "rigspark-gui.exe"],
            ["llmup", "rigspark", "rigspark-gui"],
        ),
    ] {
        let mut invalid_sets = vec![foreign.to_vec(), vec!["LICENSE"]];
        for missing in 0..required.len() {
            let mut names = required.to_vec();
            names.remove(missing);
            invalid_sets.push(names);
            let mut names = required.to_vec();
            names[missing] = foreign[missing];
            invalid_sets.push(names);
            let mut names = required.to_vec();
            names.push(foreign[missing]);
            invalid_sets.push(names);
        }
        for names in invalid_sets {
            let files: Vec<_> = names.iter().map(|name| (*name, source.as_path())).collect();
            let output = directory.path().join("package");
            assert!(
                package_directory(&output, env!("CARGO_PKG_VERSION"), target, &files).is_err(),
                "accepted incomplete or wrong-target package: {target} {names:?}"
            );
            assert!(!output.exists());
        }
    }
}

#[test]
fn patched_terminal_dependency_license_can_be_packaged_and_verified() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("binary");
    executable_fixture(&source, b"native fixture");
    let license =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/crossterm/LICENSE");
    let output = directory.path().join("package");
    package_directory(
        &output,
        env!("CARGO_PKG_VERSION"),
        "aarch64-apple-darwin",
        &[
            ("llmup", source.as_path()),
            ("rigspark", source.as_path()),
            ("rigspark-gui", source.as_path()),
            ("crossterm.LICENSE", license.as_path()),
        ],
    )
    .unwrap();
    verify_directory(&output).unwrap();
    assert_eq!(
        fs::read(output.join("crossterm.LICENSE")).unwrap(),
        fs::read(license).unwrap()
    );
}

#[test]
fn embedded_font_license_can_be_packaged_and_verified() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("binary");
    executable_fixture(&source, b"native fixture");
    let license = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rigspark-gui/static/fonts/LICENSE-OFL.txt");
    let output = directory.path().join("package");
    package_directory(
        &output,
        env!("CARGO_PKG_VERSION"),
        "aarch64-apple-darwin",
        &[
            ("llmup", source.as_path()),
            ("rigspark", source.as_path()),
            ("rigspark-gui", source.as_path()),
            ("fonts.OFL.txt", license.as_path()),
        ],
    )
    .unwrap();
    verify_directory(&output).unwrap();
    assert_eq!(
        fs::read(output.join("fonts.OFL.txt")).unwrap(),
        fs::read(license).unwrap()
    );
}

#[test]
fn public_binaries_and_gui_round_trip_unsigned_on_unix_and_windows() {
    for (target, names) in [
        (
            "aarch64-apple-darwin",
            ["llmup", "rigspark", "rigspark-gui"],
        ),
        (
            "x86_64-pc-windows-msvc",
            ["llmup.exe", "rigspark.exe", "rigspark-gui.exe"],
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let sources = names.map(|name| {
            let path = directory.path().join(name);
            executable_fixture(&path, format!("native fixture: {name}").as_bytes());
            path
        });
        let files: Vec<_> = names
            .iter()
            .zip(&sources)
            .map(|(name, path)| (*name, path.as_path()))
            .collect();
        let output = directory.path().join("package");
        package_directory(&output, env!("CARGO_PKG_VERSION"), target, &files).unwrap();
        let manifest = verify_directory(&output).unwrap();
        assert_eq!(manifest.signing, "unsigned");
        assert_eq!(
            manifest
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            names
        );
        fs::write(output.join(names[1]), b"tampered alias").unwrap();
        assert!(verify_directory(&output).is_err());
    }
}

#[test]
fn public_alias_allowlist_rejects_duplicates_launchers_and_traversal() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::write(&source, b"native fixture").unwrap();
    for names in [
        vec!["rigspark", "rigspark"],
        vec!["../rigspark"],
        vec!["rigspark.cmd"],
        vec!["rigspark.js"],
        vec!["llmup-native"],
    ] {
        let files: Vec<_> = names.iter().map(|name| (*name, source.as_path())).collect();
        let output = directory.path().join("package");
        assert!(
            package_directory(
                &output,
                env!("CARGO_PKG_VERSION"),
                "aarch64-apple-darwin",
                &files
            )
            .is_err()
        );
        assert!(!output.exists());
    }
}

#[test]
fn public_alias_manifest_cannot_claim_signing() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    executable_fixture(&source, b"native fixture");
    let output = directory.path().join("package");
    package_directory(
        &output,
        env!("CARGO_PKG_VERSION"),
        "aarch64-apple-darwin",
        &[
            ("llmup", source.as_path()),
            ("rigspark", source.as_path()),
            ("rigspark-gui", source.as_path()),
        ],
    )
    .unwrap();
    let path = output.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["signing"] = "signed".into();
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(verify_directory(&output).is_err());
}
