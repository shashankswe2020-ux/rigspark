use rigspark_cli::distribution::{homebrew_formula, parse_checksums, release_archive_stem};

const SHA: [&str; 4] = [
    "1111111111111111111111111111111111111111111111111111111111111111",
    "2222222222222222222222222222222222222222222222222222222222222222",
    "3333333333333333333333333333333333333333333333333333333333333333",
    "4444444444444444444444444444444444444444444444444444444444444444",
];

fn checksums() -> String {
    [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu",
    ]
    .iter()
    .zip(SHA)
    .map(|(target, sha)| format!("{sha}  rigspark-{target}.tar.gz\n"))
    .collect::<String>()
        + &format!(
            "{}  rigspark-x86_64-pc-windows-msvc.tar.gz\n",
            "5".repeat(64)
        )
}

#[test]
fn release_archives_match_the_cargo_binstall_template() {
    assert_eq!(
        release_archive_stem("aarch64-apple-darwin").unwrap(),
        "rigspark-aarch64-apple-darwin"
    );
    for manifest in [
        include_str!("../Cargo.toml"),
        include_str!("../../rigspark-gui/Cargo.toml"),
    ] {
        assert!(manifest.contains(
            "pkg-url = \"{ repo }/releases/download/v{ version }/rigspark-{ target }{ archive-suffix }\""
        ));
        assert!(manifest.contains("bin-dir = \"rigspark-{ target }/{ bin }{ binary-ext }\""));
        assert!(manifest.contains("pkg-fmt = \"tgz\""));
    }
    for target in ["wasm32-unknown-unknown", "../aarch64-apple-darwin", ""] {
        assert!(release_archive_stem(target).is_err(), "{target}");
    }
}

#[test]
fn checksum_files_are_strict_and_bounded() {
    let parsed = parse_checksums(&checksums()).unwrap();
    assert_eq!(parsed.len(), 5);
    assert_eq!(parsed["rigspark-aarch64-apple-darwin.tar.gz"], SHA[0]);
    for invalid in [
        String::new(),
        format!("{}  a.tar.gz\n{}  a.tar.gz\n", SHA[0], SHA[1]),
        format!("{}  ../a.tar.gz\n", SHA[0]),
        format!("{}  dir/a.tar.gz\n", SHA[0]),
        format!("{} a.tar.gz\n", SHA[0]),
        format!("{}  a.tar.gz\n", "g".repeat(64)),
        format!("{}  a.tar.gz\n", &SHA[0][..63]),
        format!("{}  a.tar.gz\n", "ab".repeat(32).to_uppercase()),
        "x\n".repeat(70_000),
    ] {
        assert!(parse_checksums(&invalid).is_err(), "{invalid:.80}");
    }
}

#[test]
fn homebrew_formula_pins_every_unix_archive_and_installs_the_trio() {
    let formula = homebrew_formula("1.0.0", &parse_checksums(&checksums()).unwrap()).unwrap();
    assert!(formula.starts_with("class Rigspark < Formula\n"));
    let description = formula
        .lines()
        .find_map(|line| line.strip_prefix("  desc \"")?.strip_suffix('"'))
        .unwrap();
    assert!(description.len() < 80, "{description}");
    assert!(!formula.contains("  version \""));
    assert!(formula.contains("  license \"MIT\"\n"));
    for (target, sha) in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu",
    ]
    .iter()
    .zip(SHA)
    {
        assert!(formula.contains(&format!(
            "url \"https://github.com/shashankswe2020-ux/rigspark/releases/download/v1.0.0/rigspark-{target}.tar.gz\"\n      sha256 \"{sha}\"\n"
        )));
    }
    for block in ["on_macos do", "on_linux do", "on_arm do", "on_intel do"] {
        assert!(formula.contains(block), "{block}");
    }
    assert!(formula.contains("bin.install \"llmup\", \"rigspark\", \"rigspark-gui\"\n"));
    assert!(formula.contains("shell_output(\"#{bin}/llmup --version\")"));
    assert!(formula.contains(
        "  def caveats\n    <<~EOS\n      If Sparky helps you, please consider sponsoring the project:\n      https://buymeacoffee.com/shashanksw9\n    EOS\n  end\n"
    ));
    assert!(!formula.contains("windows"));
}

#[test]
fn homebrew_formula_refuses_missing_archives_and_unsafe_versions() {
    let mut partial = parse_checksums(&checksums()).unwrap();
    partial.remove("rigspark-x86_64-apple-darwin.tar.gz");
    assert!(homebrew_formula("1.0.0", &partial).is_err());
    let complete = parse_checksums(&checksums()).unwrap();
    for version in ["1.0", "1.0.0-rc.1", "v1.0.0", "1.0.0\"; system \"x"] {
        assert!(homebrew_formula(version, &complete).is_err(), "{version}");
    }
}

#[test]
fn sponsor_help_matches_the_formula_caveat() {
    use rigspark_cli::distribution::{SPONSOR_HELP, SPONSOR_MESSAGE, SPONSOR_URL};
    assert_eq!(SPONSOR_HELP, format!("{SPONSOR_MESSAGE}\n{SPONSOR_URL}"));
}
