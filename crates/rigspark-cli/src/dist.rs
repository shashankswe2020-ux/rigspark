use clap::{Parser, Subcommand};
use rigspark_cli::distribution::{
    archive_path, checksum, homebrew_formula, package_directory, parse_checksums,
    release_archive_stem, verify_directory,
};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

#[derive(Parser)]
#[command(about = "Build and verify unsigned native CLI/GUI archives and the Homebrew formula")]
struct Args {
    #[command(subcommand)]
    command: Operation,
}
#[derive(Subcommand)]
enum Operation {
    Package {
        /// Rust target triple; defaults to the host
        #[arg(long)]
        target: Option<String>,
    },
    Verify {
        directory: PathBuf,
    },
    /// Print a Homebrew formula for a published release's SHA256SUMS file
    Formula {
        #[arg(long)]
        version: String,
        #[arg(long)]
        checksums: PathBuf,
    },
}

fn checked(command: &mut Command) -> Result<(), Box<dyn std::error::Error>> {
    if !command.status()?.success() {
        return Err("native build or archive command failed".into());
    }
    Ok(())
}
fn native_build_command(cargo: &OsStr, root: &Path, target: &str, build_target: &Path) -> Command {
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args([
            "build",
            "--release",
            "--locked",
            "--target",
            target,
            "-p",
            "rigspark-cli",
            "--bin",
            "llmup",
            "--bin",
            "rigspark",
            "-p",
            "rigspark-gui",
            "--bin",
            "rigspark-gui",
        ])
        .arg("--target-dir")
        .arg(build_target);
    command
}
fn native_package_files(root: &Path, release: &Path, extension: &str) -> Vec<(String, PathBuf)> {
    vec![
        (
            format!("llmup{extension}"),
            release.join(format!("llmup{extension}")),
        ),
        (
            format!("rigspark{extension}"),
            release.join(format!("rigspark{extension}")),
        ),
        (
            format!("rigspark-gui{extension}"),
            release.join(format!("rigspark-gui{extension}")),
        ),
        ("LICENSE".into(), root.join("LICENSE")),
        (
            "marked.LICENSE.md".into(),
            root.join("crates/rigspark-gui/vendor/marked.LICENSE.md"),
        ),
        (
            "dompurify.LICENSE".into(),
            root.join("crates/rigspark-gui/vendor/dompurify.LICENSE"),
        ),
        (
            "katex.LICENSE".into(),
            root.join("crates/rigspark-gui/vendor/katex.LICENSE"),
        ),
        (
            "THIRD-PARTY.md".into(),
            root.join("crates/rigspark-gui/vendor/README.md"),
        ),
        (
            "crossterm.LICENSE".into(),
            root.join("vendor/crossterm/LICENSE"),
        ),
        (
            "fonts.OFL.txt".into(),
            root.join("crates/rigspark-gui/static/fonts/LICENSE-OFL.txt"),
        ),
        (
            "CROSSTERM-PATCH.md".into(),
            root.join("vendor/crossterm/RIGSPARK-PATCH.md"),
        ),
    ]
}
fn verify_native_versions(
    release: &Path,
    extension: &str,
    mut probe: impl FnMut(&mut Command) -> std::io::Result<std::process::Output>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (name, product) in [
        ("llmup", "rigspark"),
        ("rigspark", "rigspark"),
        ("rigspark-gui", "rigspark-gui"),
    ] {
        let mut command = Command::new(release.join(format!("{name}{extension}")));
        command.arg("--version");
        let observed = probe(&mut command)
            .map_err(|_| format!("cannot query native binary version: {name}"))?;
        if !observed.status.success()
            || std::str::from_utf8(&observed.stdout).map(str::trim).ok()
                != Some(format!("{product} {}", env!("CARGO_PKG_VERSION")).as_str())
        {
            return Err(format!("native binary version mismatch: {name}").into());
        }
    }
    Ok(())
}
fn execute(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let requested = match args.command {
        Operation::Verify { directory } => {
            let manifest = verify_directory(&directory)?;
            println!(
                "Verified {} {} (unsigned; checksums do not authenticate publishers)",
                manifest.version, manifest.target
            );
            return Ok(());
        }
        Operation::Formula { version, checksums } => {
            let text = fs::read_to_string(checksums)?;
            print!("{}", homebrew_formula(&version, &parse_checksums(&text)?)?);
            return Ok(());
        }
        Operation::Package { target } => target,
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = Command::new(rustc).arg("-vV").output()?;
    if !version.status.success() {
        return Err("rustc version query failed".into());
    }
    let version = String::from_utf8(version.stdout)?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("missing Rust host target")?;
    let target = requested.as_deref().unwrap_or(host);
    let stem = release_archive_stem(target)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let build_target = root.join("target/native-build");
    checked(&mut native_build_command(
        &cargo,
        &root,
        target,
        &build_target,
    ))?;
    let extension = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let release = build_target.join(target).join("release");
    // Cross-built binaries (Intel macOS on Apple Silicon) cannot always be executed on the host.
    if target == host {
        verify_native_versions(&release, extension, Command::output)?;
    } else {
        println!("Cross-built {target}: version probe skipped on {host}");
    }
    let parent = root.join("target/native-dist");
    fs::create_dir_all(&parent)?;
    let output = parent.join(&stem);
    if output.exists() {
        fs::remove_dir_all(&output)?;
    }
    let files = native_package_files(&root, &release, extension);
    package_directory(
        &output,
        env!("CARGO_PKG_VERSION"),
        target,
        &files
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_path()))
            .collect::<Vec<_>>(),
    )?;
    let archive = archive_path(&output)?;
    // Relative names: GNU tar on Windows runners parses `C:\...` as a remote host.
    checked(
        Command::new("tar")
            .current_dir(&parent)
            .arg("-czf")
            .arg(archive.file_name().ok_or("missing archive name")?)
            .arg(output.file_name().ok_or("missing package name")?),
    )?;
    let (_, sha256) = checksum(&archive)?;
    fs::write(
        archive.with_extension("gz.sha256"),
        format!(
            "{sha256}  {}\n",
            archive
                .file_name()
                .ok_or("missing archive name")?
                .to_string_lossy()
        ),
    )?;
    println!("Unsigned native archive: {}", archive.display());
    println!("SHA-256: {sha256}");
    Ok(())
}
fn main() -> ExitCode {
    match execute(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native-dist: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version_output(success: bool, stdout: Vec<u8>) -> std::process::Output {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(if success { 0 } else { 256 }),
            stdout,
            stderr: Vec::new(),
        }
    }

    #[test]
    fn version_gate_checks_both_aliases_and_companion_on_each_platform() {
        for extension in ["", ".exe"] {
            let release = PathBuf::from("release");
            let mut visited = Vec::new();
            verify_native_versions(&release, extension, |command| {
                let path = PathBuf::from(command.get_program());
                assert_eq!(command.get_args().collect::<Vec<_>>(), ["--version"]);
                let product = if path.file_stem().unwrap() == "rigspark-gui" {
                    "rigspark-gui"
                } else {
                    "rigspark"
                };
                visited.push(path);
                Ok(version_output(
                    true,
                    format!("{product} {}\n", env!("CARGO_PKG_VERSION")).into_bytes(),
                ))
            })
            .unwrap();
            assert_eq!(
                visited,
                ["llmup", "rigspark", "rigspark-gui"]
                    .map(|name| release.join(format!("{name}{extension}")))
            );
        }
    }

    #[test]
    fn version_gate_rejects_mismatch_failed_exit_and_invalid_output_for_each_binary() {
        for broken in ["llmup", "rigspark", "rigspark-gui"] {
            for fault in ["version", "product", "exit", "encoding", "missing"] {
                let error = verify_native_versions(Path::new("release"), "", |command| {
                    let path = Path::new(command.get_program());
                    let is_broken = path.file_name().unwrap() == broken;
                    if is_broken && fault == "missing" {
                        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
                    }
                    let product = if is_broken && fault == "product" {
                        "wrong-product"
                    } else if path.file_name().unwrap() == "rigspark-gui" {
                        "rigspark-gui"
                    } else {
                        "rigspark"
                    };
                    let version = if is_broken && fault == "version" {
                        "0.0.0"
                    } else {
                        env!("CARGO_PKG_VERSION")
                    };
                    let stdout = if is_broken && fault == "encoding" {
                        vec![0xff]
                    } else {
                        format!("{product} {version}\n").into_bytes()
                    };
                    Ok(version_output(!(is_broken && fault == "exit"), stdout))
                })
                .unwrap_err();
                assert!(error.to_string().contains(broken), "{error}");
            }
        }
    }

    #[test]
    fn archive_build_requests_both_public_binaries_and_gui() {
        let root = PathBuf::from("workspace");
        let target_dir = root.join("target/native-build");
        let command =
            native_build_command("cargo".as_ref(), &root, "aarch64-apple-darwin", &target_dir);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(command.get_current_dir(), Some(root.as_path()));
        assert_eq!(
            args,
            [
                "build",
                "--release",
                "--locked",
                "--target",
                "aarch64-apple-darwin",
                "-p",
                "rigspark-cli",
                "--bin",
                "llmup",
                "--bin",
                "rigspark",
                "-p",
                "rigspark-gui",
                "--bin",
                "rigspark-gui",
                "--target-dir",
                target_dir.to_str().unwrap(),
            ]
        );
    }

    #[test]
    fn archive_uses_public_builds_and_required_gui_on_each_platform() {
        let root = PathBuf::from("workspace");
        let release = root.join("release");
        for extension in ["", ".exe"] {
            let files = native_package_files(&root, &release, extension);
            assert_eq!(files.len(), 11);
            for (index, name) in ["llmup", "rigspark", "rigspark-gui"].iter().enumerate() {
                let name = format!("{name}{extension}");
                assert_eq!(files[index], (name.clone(), release.join(name)));
            }
            assert_eq!(files[3], ("LICENSE".into(), root.join("LICENSE")));
            assert!(files.contains(&(
                "katex.LICENSE".into(),
                root.join("crates/rigspark-gui/vendor/katex.LICENSE")
            )));
            assert!(files.contains(&(
                "crossterm.LICENSE".into(),
                root.join("vendor/crossterm/LICENSE")
            )));
            // rigspark-gui embeds Inter and JetBrains Mono; OFL-1.1 must travel with them.
            assert!(files.contains(&(
                "fonts.OFL.txt".into(),
                root.join("crates/rigspark-gui/static/fonts/LICENSE-OFL.txt")
            )));
            assert!(files.contains(&(
                "CROSSTERM-PATCH.md".into(),
                root.join("vendor/crossterm/RIGSPARK-PATCH.md")
            )));
            assert!(
                files
                    .iter()
                    .all(|(_, path)| !path.to_string_lossy().contains("llmup-native"))
            );
        }
    }
}
