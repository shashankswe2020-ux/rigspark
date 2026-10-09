use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn workflows() -> Vec<(String, String)> {
    let mut entries: Vec<_> = fs::read_dir(root().join(".github/workflows"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "yml"))
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(path).unwrap())
        })
        .collect();
    entries.sort();
    entries
}

#[test]
fn node_toolchain_manifests_are_gone() {
    for path in [
        "package.json",
        "package-lock.json",
        "tsconfig.json",
        ".npmignore",
        "eslint.config.js",
        "vitest.config.ts",
        "playwright.config.ts",
        "playwright.rust.config.ts",
        "src",
        "data",
        "vendor/gui",
        ".github/workflows/npm-publish.yml",
        ".github/workflows/tui-compatibility.yml",
    ] {
        assert!(!root().join(path).exists(), "{path}");
    }
}

#[test]
fn workflows_declare_permissions_and_use_no_unpinned_actions() {
    let entries = workflows();
    assert!(entries.len() >= 6);
    for (name, text) in &entries {
        assert!(
            text.contains("\npermissions:\n"),
            "{name} must declare permissions"
        );
        for line in text.lines().filter(|line| {
            line.trim_start().starts_with("- uses:") || line.trim_start().starts_with("uses:")
        }) {
            let reference = line.rsplit('@').next().unwrap_or_default().trim();
            assert!(
                reference.len() == 40 && reference.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{name}: action must be pinned to a full commit SHA: {line}"
            );
        }
    }
}

#[test]
fn ci_runs_every_native_gate_read_only() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("permissions:\n  contents: read\n"));
    for step in [
        "test \"$(git rev-parse HEAD)\" = \"$GITHUB_SHA\"",
        "cargo native-retirement",
        "cargo catalog-site --check",
        "cargo fmt --all -- --check",
        "cargo clippy --workspace --all-targets --locked -- -D warnings",
        "cargo test --workspace --locked",
        "cargo package --workspace --locked",
        "cargo build --workspace --locked",
    ] {
        assert!(ci.contains(step), "{step}");
    }
}

#[test]
fn write_capable_workflows_never_push_to_main() {
    let catalog = read(".github/workflows/catalog-refresh.yml");
    assert!(catalog.contains("contents: write\n  pull-requests: write\n  issues: write\n"));
    // Only the admission job writes catalog data; the proposal job writes its report only.
    let proposals = catalog.split("\n  admit:\n").next().unwrap();
    assert!(!proposals.contains("git add crates/rigspark-core/data/models.json"));
    assert!(proposals.contains("git add docs/references/catalog-proposals.json"));
    assert!(catalog.contains("gh pr create --base main"));
    assert!(catalog.contains("cargo test --locked -p rigspark-core --test catalog"));
    let pages = read(".github/workflows/pages.yml");
    assert!(pages.contains("git push origin gh-pages"));
    for (name, text) in [("catalog-refresh.yml", &catalog), ("pages.yml", &pages)] {
        for forbidden in ["push origin main", "HEAD:main", "push --force", "push -f"] {
            assert!(!text.contains(forbidden), "{name}: {forbidden}");
        }
    }
    let backlog = read(".github/workflows/project-backlog.yml");
    let script = backlog.split("run: |").nth(1).unwrap();
    assert!(
        !script.contains("${{"),
        "event data must reach the shell through env"
    );
    assert!(backlog.contains("gh project item-add"));
}

#[test]
fn automatic_admission_merges_only_verified_data_only_changes() {
    let workflow = read(".github/workflows/catalog-refresh.yml");
    let admit = workflow
        .split("\n  admit:\n")
        .nth(1)
        .expect("admission job");
    for required in [
        "needs.pending-review.outputs.admit == 'true'",
        "cargo catalog-admit --correct-curated",
        "cargo generation-admit",
        "cargo catalog-site",
        "cargo test --workspace --locked",
        "cargo catalog-quality",
        "Admission touched files outside the data allow-list; refusing.",
        "gh pr checks \"$URL\" --watch --interval 60 --fail-fast --required",
        "PR diff left the data allow-list; refusing to merge.",
        "(cd \"$RUNNER_TEMP\" && gh pr merge \"$URL\" --repo \"$GITHUB_REPOSITORY\" --admin --squash)\n          echo \"merged=true\" >> \"$GITHUB_OUTPUT\"",
        "MERGE_TOKEN: ${{ secrets.CATALOG_MERGE_TOKEN }}",
        "steps.merge.outputs.merged == 'true' && steps.gate.outputs.publish == 'true'",
        "gh workflow run catalog-publish.yml --ref main",
    ] {
        assert!(admit.contains(required), "{required}");
    }
    // Inside the checkout, gh switches the local PR branch back to main after merging, which
    // fails in the shallow runner (run 37774747796) and skips the publish dispatch.
    assert!(!admit.contains("--delete-branch"));
    // The checks gate precedes the merge, and the publish dispatch follows it.
    assert!(admit.find("gh pr checks").unwrap() < admit.find("gh pr merge").unwrap());
    let allowed = admit
        .lines()
        .find(|line| line.trim_start().starts_with("ALLOWED="))
        .unwrap();
    for path in ["\\.rs", "Cargo", ".github", "main\\.js", "index\\.html"] {
        assert!(!allowed.contains(path), "{path} must never be auto-merged");
    }
    let script: String = admit
        .lines()
        .filter(|line| {
            !line.trim_start().starts_with("MERGE_TOKEN:")
                && !line.trim_start().starts_with("GH_TOKEN:")
                && !line.trim_start().starts_with("FALLBACK_TOKEN:")
        })
        .collect();
    assert!(
        !script.contains("${{ secrets."),
        "secrets reach the shell only through env"
    );
    for forbidden in ["push origin main", "HEAD:main", "push --force", "push -f"] {
        assert!(!admit.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn publishable_crates_carry_crates_io_metadata_and_exclude_tests() {
    let version = env!("CARGO_PKG_VERSION");
    let workspace = read("Cargo.toml");
    assert!(workspace.contains(&format!("version = \"{version}\"")));
    for forbidden in ["[patch", "publish = false"] {
        assert!(!workspace.contains(forbidden), "{forbidden}");
    }
    for name in [
        "rigspark-core",
        "rigspark-runtime",
        "rigspark-gui",
        "rigspark-cli",
    ] {
        let manifest = read(&format!("crates/{name}/Cargo.toml"));
        for field in [
            "license.workspace = true",
            "repository.workspace = true",
            "description = \"",
            "include = [\"src/**\"",
        ] {
            assert!(manifest.contains(field), "{name}: {field}");
        }
        let include = manifest
            .lines()
            .find(|line| line.starts_with("include = "))
            .unwrap();
        assert!(
            !include.contains("tests"),
            "{name} must not publish its test oracles"
        );
        for dependency in manifest
            .lines()
            .filter(|line| line.starts_with("rigspark-"))
        {
            assert!(
                dependency.contains(&format!("version = \"{version}\"")),
                "{name}: {dependency}"
            );
        }
    }
    let fork = read("vendor/crossterm/Cargo.toml");
    for required in [
        "name = \"rigspark-crossterm\"",
        "[lib]\nname = \"crossterm\"",
        "license = \"MIT\"",
        "\"Cargo.toml.orig\"",
    ] {
        assert!(fork.contains(required), "{required}");
    }
}

#[test]
fn vendored_browser_libraries_match_their_pinned_hashes_and_licenses() {
    let notice = read("crates/rigspark-gui/vendor/README.md");
    for (file, version, header, license) in [
        (
            "marked.min.js",
            "15.0.12",
            "marked v15.0.12",
            "marked.LICENSE.md",
        ),
        (
            "dompurify.min.js",
            "3.4.13",
            "DOMPurify 3.4.13",
            "dompurify.LICENSE",
        ),
        (
            "katex/katex.min.js",
            "0.19.0",
            "exports.katex",
            "katex.LICENSE",
        ),
    ] {
        let bytes = fs::read(root().join("crates/rigspark-gui/vendor").join(file)).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let row = notice
            .lines()
            .find(|line| line.contains(&format!("| {version} |")))
            .unwrap_or_else(|| panic!("{file} missing from vendor notice"));
        assert!(
            row.contains(&digest),
            "{file}: {digest} not pinned in vendor notice"
        );
        assert!(String::from_utf8_lossy(&bytes[..400.min(bytes.len())]).contains(header));
        assert!(
            !read(&format!("crates/rigspark-gui/vendor/{license}"))
                .trim()
                .is_empty()
        );
    }
}

#[test]
fn guide_media_references_exist() {
    let guide = read("docs/references/guide.md");
    let mut found = 0;
    for (index, _) in guide.match_indices("src=\"../../assets/") {
        let start = index + "src=\"../../".len();
        let end = start + guide[start..].find('"').unwrap();
        let path = &guide[start..end];
        assert!(root().join(path).is_file(), "{path}");
        found += 1;
    }
    assert!(found > 0);
}

#[test]
fn readme_documents_native_install_and_primary_workflows() {
    let readme = read("README.md");
    for required in [
        "brew install shashankswe2020-ux/tap/rigspark",
        "(docs/references/guide.md#install)",
        "(docs/references/guide.md#docker)",
        "assets/rigspark.mp4",
        "assets/rigspark-preview.gif",
        "rigspark recommend",
        "rigspark up",
        "rigspark chat",
        "rigspark catalog",
        "rigspark catalog --status",
        "rigspark catalog --update",
        "recommendations and normal startup remain\noffline",
        "rigspark catalog --refresh",
        "docs/references/guide.md#independent-catalog-updates",
        "rigspark gui",
        "`llmup` remains a compatibility alias",
    ] {
        assert!(readme.contains(required), "{required}");
    }
    let reference = read("docs/references/guide.md");
    for required in [
        "## Install",
        "### Docker",
        "cargo binstall rigspark-cli rigspark-gui",
        "cargo install rigspark-cli --locked",
        "cargo install rigspark-gui --locked",
        "docker pull ghcr.io/shashankswe2020-ux/rigspark",
    ] {
        assert!(reference.contains(required), "{required}");
    }
    for forbidden in ["npm install -g", "npx rigspark", "Node.js 18"] {
        assert!(!readme.contains(forbidden), "{forbidden}");
        assert!(!reference.contains(forbidden), "{forbidden}");
    }
    assert!(read("site/index.html").contains("docker pull ghcr.io/shashankswe2020-ux/rigspark"));
}

#[test]
fn site_downloads_every_release_archive_through_stable_latest_links() {
    let site = read("site/index.html");
    for target in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-pc-windows-msvc",
    ] {
        let url = format!(
            "https://github.com/shashankswe2020-ux/rigspark/releases/latest/download/{}.tar.gz",
            rigspark_cli::distribution::release_archive_stem(target).unwrap()
        );
        assert!(site.contains(&url), "{url}");
    }
    assert!(site.contains("releases/latest/download/SHA256SUMS"));
    for stale in [
        "releases/download/v0.",
        ".dmg",
        ".AppImage",
        "setup.exe",
        "Electron",
    ] {
        assert!(!site.contains(stale), "{stale}");
    }
}

#[test]
fn site_is_indexable_and_offers_only_native_installs() {
    let site = read("site/index.html");
    let canonical = "https://www.rigspark.si/";
    assert!(site.contains(&format!("<link rel=\"canonical\" href=\"{canonical}\" />")));
    assert!(site.contains(&format!(
        "<meta property=\"og:image\" content=\"{canonical}assets/"
    )));
    assert!(read("site/sitemap.xml").contains(&format!("<loc>{canonical}</loc>")));
    let blocks: Vec<serde_json::Value> = site
        .split("<script type=\"application/ld+json\">")
        .skip(1)
        .map(|block| serde_json::from_str(block.split("</script>").next().unwrap()).unwrap())
        .collect();
    let questions: Vec<&str> = blocks
        .iter()
        .find(|block| block["@type"] == "FAQPage")
        .unwrap()["mainEntity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|question| question["name"].as_str().unwrap())
        .collect();
    let visible: Vec<&str> = site
        .split("<summary>")
        .skip(1)
        .map(|item| item.split("</summary>").next().unwrap())
        .collect();
    assert_eq!(questions, visible, "structured FAQ must match visible FAQ");
    assert!(
        blocks
            .iter()
            .any(|block| block["@type"] == "SoftwareApplication")
    );
    for retired in ["npm install -g", "npx rigspark", "Ink 5"] {
        assert!(!site.contains(retired), "{retired}");
    }
}

#[test]
fn releases_publish_the_documented_container_image_from_verified_archives() {
    let container = read(".github/workflows/container.yml");
    let release = read(".github/workflows/release.yml");
    let dockerfile = read("Dockerfile");
    // GITHUB_TOKEN-published releases cannot start workflows, so release.yml dispatches.
    assert!(release.contains("gh workflow run container.yml"));
    assert!(container.contains("workflow_dispatch:"));
    assert!(
        !container.contains("uses:"),
        "workflows avoid action runtimes"
    );
    for required in [
        "ghcr.io/${GITHUB_REPOSITORY,,}",
        "packages: write",
        "sha256sum --check --strict",
        "--target release",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "imagetools create",
        "org.opencontainers.image.source",
    ] {
        assert!(container.contains(required), "{required}");
    }
    // The recipe comes from the workflow revision, so tags cut before the release target existed still package.
    assert!(container.contains("git fetch --no-tags --depth=1 origin \"$GITHUB_SHA\""));
    assert!(!container.contains("refs/tags/$TAG"));
    assert!(container.contains("org.opencontainers.image.revision=$SOURCE_REVISION"));
    // Release archives are built on Ubuntu 24.04 and need glibc 2.39 at runtime.
    assert!(dockerfile.contains("FROM debian:trixie-slim@sha256:"));
    assert!(dockerfile.contains("FROM base AS release"));
    assert!(dockerfile.contains("COPY release-bin/llmup"));
    let documented = "docker pull ghcr.io/shashankswe2020-ux/rigspark";
    assert!(read("site/index.html").contains(documented));
    assert!(read("docs/references/guide.md").contains(documented));
}
