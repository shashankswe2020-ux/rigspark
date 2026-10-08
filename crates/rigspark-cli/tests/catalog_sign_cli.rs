use std::{fs, process::Command};

const SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

#[test]
fn publication_is_protected_separate_from_binary_releases_and_not_latest() {
    let workflow = include_str!("../../../.github/workflows/catalog-publish.yml");
    for required in [
        "workflow_dispatch:",
        "github.ref == 'refs/heads/main'",
        "environment: catalog-signing",
        "cancel-in-progress: false",
        "umask 077",
        "--prerelease --latest=false",
        "catalog-r${GITHUB_RUN_NUMBER}",
        "gh release upload catalog-v1 catalog.json --clobber",
        "cargo catalog-quality",
        "--quality-evidence docs/references/catalog-quality-evidence.json",
        "--quality-output catalog-quality.json",
        "gh release upload catalog-v1 catalog-quality.json --clobber",
    ] {
        assert!(workflow.contains(required), "{required}");
    }
    assert!(!workflow.contains("pull_request"));
    assert!(!workflow.contains("set -x"));
    assert!(
        workflow.find("cargo catalog-quality").unwrap()
            < workflow.find("- name: Sign reviewed catalog").unwrap()
    );
}

#[test]
fn signer_validates_key_and_refuses_to_overwrite_output() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("seed"), SEED).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path().join("seed"), fs::Permissions::from_mode(0o600)).unwrap();
    }
    fs::write(root.path().join("public"), PUBLIC).unwrap();
    let mut catalog = rigspark_core::catalog::Catalog::parse(include_str!(
        "../../rigspark-core/fixtures/catalog-baseline.json"
    ))
    .unwrap();
    catalog.generated_at = "2026-09-30T00:00:00Z".into();
    catalog.models.truncate(1);
    let evidence = serde_json::json!({"policyVersion":1,"scopes":[{
        "name":"ollama-local-variants","checkedAt":"2026-09-30T00:00:00Z",
        "source":"https://ollama.com/library","complete":true,"variants":[catalog.models[0].id]
    }],"observations":[{"id":catalog.models[0].id,"checkedAt":"2026-09-30T00:00:00Z",
        "sources":["https://huggingface.co/Qwen/Qwen3.6-35B-A3B","https://registry.ollama.ai/v2/library/qwen3.6/manifests/35b"],"model":catalog.models[0]}]});
    fs::write(
        root.path().join("models.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    fs::write(root.path().join("evidence.json"), evidence.to_string()).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_llmup-catalog-sign"))
            .current_dir(root.path())
            .args([
                "--catalog-path",
                "models.json",
                "--key-file",
                "seed",
                "--public-key-file",
                "public",
                "--output",
                "catalog.json",
                "--quality-evidence",
                "evidence.json",
                "--quality-output",
                "catalog-quality.json",
                "--revision",
                "1",
                "--published-at",
                "2026-09-30T00:00:00Z",
            ])
            .output()
            .unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let artifact = fs::read(root.path().join("catalog.json")).unwrap();
    let public = rigspark_runtime::catalog_update::decode_hex(PUBLIC).unwrap();
    assert_eq!(
        rigspark_runtime::catalog_update::verify(&artifact, &public)
            .unwrap()
            .revision,
        1
    );
    assert!(!run().status.success());
    assert_eq!(
        fs::read(root.path().join("catalog.json")).unwrap(),
        artifact
    );
    fs::remove_file(root.path().join("catalog.json")).unwrap();
    fs::remove_file(root.path().join("catalog-quality.json")).unwrap();
    let mut stale = evidence.clone();
    stale["observations"][0]["checkedAt"] = serde_json::json!("2026-09-01T00:00:00Z");
    fs::write(root.path().join("evidence.json"), stale.to_string()).unwrap();
    assert!(!run().status.success());
    assert!(!root.path().join("catalog.json").exists());
    assert!(!root.path().join("catalog-quality.json").exists());
    fs::write(root.path().join("evidence.json"), evidence.to_string()).unwrap();
    fs::write(root.path().join("public"), "00".repeat(32)).unwrap();
    assert!(!run().status.success());
    assert!(!root.path().join("catalog.json").exists());
    assert!(!String::from_utf8_lossy(&first.stderr).contains(SEED));
}
