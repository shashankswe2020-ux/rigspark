use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_FILE: u64 = 256 * 1024 * 1024;
const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
];
const FILES: &[&str] = &[
    "llmup",
    "llmup.exe",
    "rigspark",
    "rigspark.exe",
    "rigspark-gui",
    "rigspark-gui.exe",
    "LICENSE",
    "marked.LICENSE.md",
    "dompurify.LICENSE",
    "katex.LICENSE",
    "crossterm.LICENSE",
    "fonts.OFL.txt",
    "CROSSTERM-PATCH.md",
    "THIRD-PARTY.md",
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u8,
    pub version: String,
    pub target: String,
    pub signing: String,
    pub files: Vec<Artifact>,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub fn archive_path(directory: &Path) -> io::Result<PathBuf> {
    let mut name = directory
        .file_name()
        .ok_or_else(|| invalid("missing archive directory name"))?
        .to_os_string();
    name.push(".tar.gz");
    Ok(directory.with_file_name(name))
}
fn validate(version: &str, target: &str, names: &[&str]) -> io::Result<()> {
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty() || part.len() > 8 || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
        || !TARGETS.contains(&target)
    {
        return Err(invalid("invalid native package version or target"));
    }
    let mut seen = BTreeSet::new();
    if names.is_empty()
        || names.len() > FILES.len()
        || names
            .iter()
            .any(|name| !FILES.contains(name) || !seen.insert(*name))
    {
        return Err(invalid("invalid or duplicate native artifact name"));
    }
    let (required, foreign) = if target.contains("windows") {
        (
            ["llmup.exe", "rigspark.exe", "rigspark-gui.exe"],
            ["llmup", "rigspark", "rigspark-gui"],
        )
    } else {
        (
            ["llmup", "rigspark", "rigspark-gui"],
            ["llmup.exe", "rigspark.exe", "rigspark-gui.exe"],
        )
    };
    if required.iter().any(|name| !seen.contains(name))
        || foreign.iter().any(|name| seen.contains(name))
    {
        return Err(invalid(
            "native package requires both public aliases and the GUI companion with target-correct names",
        ));
    }
    Ok(())
}
pub fn checksum(path: &Path) -> io::Result<(u64, String)> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FILE {
        return Err(invalid("invalid native artifact file"));
    }
    let mut file = fs::File::open(path)?.take(MAX_FILE + 1);
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        hash.update(&buffer[..count]);
    }
    if total != metadata.len() || total > MAX_FILE {
        return Err(invalid("native artifact changed while reading"));
    }
    Ok((total, format!("{:x}", hash.finalize())))
}
fn verify_permissions(name: &str, path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if ["llmup", "rigspark", "rigspark-gui"].contains(&name)
            && fs::symlink_metadata(path)?.permissions().mode() & 0o100 == 0
        {
            return Err(invalid("native binary must have owner execute permission"));
        }
    }
    #[cfg(not(unix))]
    let _ = (name, path);
    Ok(())
}
pub fn package_directory(
    output: &Path,
    version: &str,
    target: &str,
    files: &[(&str, &Path)],
) -> io::Result<Manifest> {
    validate(
        version,
        target,
        &files.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
    )?;
    let mut manifest = Manifest {
        schema_version: 1,
        version: version.into(),
        target: target.into(),
        signing: "unsigned".into(),
        files: Vec::new(),
    };
    for (name, source) in files {
        let (bytes, sha256) = checksum(source)?;
        verify_permissions(name, source)?;
        manifest.files.push(Artifact {
            name: (*name).into(),
            bytes,
            sha256,
        });
    }
    fs::create_dir(output)?;
    for (name, source) in files {
        fs::copy(source, output.join(name))?;
    }
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    verify_directory(output)?;
    Ok(manifest)
}
pub fn verify_directory(directory: &Path) -> io::Result<Manifest> {
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err(invalid("native package must be a directory"));
    }
    let path = directory.join("manifest.json");
    if !fs::symlink_metadata(&path)?.is_file() {
        return Err(invalid("invalid manifest file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(invalid("manifest exceeds limit"));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    validate(
        &manifest.version,
        &manifest.target,
        &manifest
            .files
            .iter()
            .map(|file| file.name.as_str())
            .collect::<Vec<_>>(),
    )?;
    if manifest.schema_version != 1 || manifest.signing != "unsigned" {
        return Err(invalid("unsupported native manifest"));
    }
    let mut expected = BTreeSet::from(["manifest.json".to_owned()]);
    for file in &manifest.files {
        let (bytes, hash) = checksum(&directory.join(&file.name))?;
        verify_permissions(&file.name, &directory.join(&file.name))?;
        if bytes != file.bytes || hash != file.sha256 {
            return Err(invalid("native artifact checksum mismatch"));
        }
        expected.insert(file.name.clone());
    }
    for entry in fs::read_dir(directory)? {
        if !expected.remove(&entry?.file_name().to_string_lossy().into_owned()) {
            return Err(invalid("unexpected package file"));
        }
    }
    if !expected.is_empty() {
        return Err(invalid("missing package file"));
    }
    Ok(manifest)
}

const REPOSITORY: &str = "https://github.com/shashankswe2020-ux/rigspark";
const MAX_CHECKSUMS: usize = 64 * 1024;

fn release_version(version: &str) -> io::Result<()> {
    validate(version, TARGETS[0], &["llmup", "rigspark", "rigspark-gui"])
}

/// Archive and top-level directory name; `cargo binstall` metadata depends on this exact shape.
pub fn release_archive_stem(target: &str) -> io::Result<String> {
    if !TARGETS.contains(&target) {
        return Err(invalid("invalid native package version or target"));
    }
    Ok(format!("rigspark-{target}"))
}

pub fn parse_checksums(text: &str) -> io::Result<std::collections::BTreeMap<String, String>> {
    if text.is_empty() || text.len() > MAX_CHECKSUMS {
        return Err(invalid("checksum file is empty or too large"));
    }
    let mut entries = std::collections::BTreeMap::new();
    for line in text.lines() {
        let (hash, name) = line
            .split_once("  ")
            .ok_or_else(|| invalid("checksum lines must be `<sha256>  <file>`"))?;
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || name.is_empty()
            || name.contains(['/', '\\', ' '])
            || name.starts_with('.')
            || entries.insert(name.to_owned(), hash.to_owned()).is_some()
        {
            return Err(invalid("invalid or duplicate checksum entry"));
        }
    }
    Ok(entries)
}

pub fn homebrew_formula(
    version: &str,
    checksums: &std::collections::BTreeMap<String, String>,
) -> io::Result<String> {
    release_version(version)?;
    let asset = |target: &str| -> io::Result<String> {
        let file = format!("{}.tar.gz", release_archive_stem(target)?);
        let sha = checksums
            .get(&file)
            .ok_or_else(|| invalid("checksum file is missing a Homebrew archive"))?;
        Ok(format!(
            "      url \"{REPOSITORY}/releases/download/v{version}/{file}\"\n      sha256 \"{sha}\"\n"
        ))
    };
    Ok(format!(
        "class Rigspark < Formula\n  desc \"Hardware-aware CLI for choosing and running local LLMs\"\n  homepage \"{REPOSITORY}\"\n  license \"MIT\"\n\n  on_macos do\n    on_arm do\n{}    end\n    on_intel do\n{}    end\n  end\n\n  on_linux do\n    on_arm do\n{}    end\n    on_intel do\n{}    end\n  end\n\n  def install\n    bin.install \"llmup\", \"rigspark\", \"rigspark-gui\"\n  end\n\n  test do\n    assert_match version.to_s, shell_output(\"#{{bin}}/llmup --version\")\n  end\nend\n",
        asset("aarch64-apple-darwin")?,
        asset("x86_64-apple-darwin")?,
        asset("aarch64-unknown-linux-gnu")?,
        asset("x86_64-unknown-linux-gnu")?,
    ))
}
