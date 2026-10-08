use rigspark_core::catalog::{Catalog, EntryProvenance};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

const NOW: &str = "2026-10-07T03:17:00Z";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A plain-attention GGUF header whose single tensor holds `total` elements.
fn gguf(total: u64) -> Vec<u8> {
    let mut out = b"GGUF".to_vec();
    out.extend(3u32.to_le_bytes());
    out.extend(1u64.to_le_bytes());
    let kv: [(&str, Result<&str, u32>); 7] = [
        ("general.architecture", Ok("granite")),
        ("general.license", Ok("apache-2.0")),
        ("granite.block_count", Err(32)),
        ("granite.context_length", Err(32768)),
        ("granite.embedding_length", Err(4096)),
        ("granite.attention.head_count", Err(32)),
        ("granite.attention.head_count_kv", Err(8)),
    ];
    out.extend((kv.len() as u64).to_le_bytes());
    let string = |out: &mut Vec<u8>, text: &str| {
        out.extend((text.len() as u64).to_le_bytes());
        out.extend(text.as_bytes());
    };
    for (key, value) in kv {
        string(&mut out, key);
        match value {
            Ok(text) => {
                out.extend(8u32.to_le_bytes());
                string(&mut out, text);
            }
            Err(number) => {
                out.extend(4u32.to_le_bytes());
                out.extend(number.to_le_bytes());
            }
        }
    }
    string(&mut out, "token_embd.weight");
    out.extend(1u32.to_le_bytes());
    out.extend(total.to_le_bytes());
    out.extend(12u32.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out
}

fn setup(root: &Path) {
    let mut catalog = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    catalog.models.retain(|model| model.id == "mistral:7b");
    fs::write(
        root.join("models.json"),
        serde_json::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("evidence.json"),
        json!({"policyVersion":1,"scopes":[],"observations":[]}).to_string(),
    )
    .unwrap();
    let model = "a".repeat(64);
    let config = "b".repeat(64);
    let registry = "https://registry.ollama.ai/v2/library/granite9";
    let fixture = json!({
        "https://ollama.com/library?sort=newest": {"status":200,"text":r#"<a href="/library/granite9" class="group w-full space-y-5"><span class="inline-flex items-center rounded-md x">tools</span></a>"#},
        "https://ollama.com/library/granite9/tags": {"status":200,"text":r#"<a href="/library/granite9:8b"></a><a href="/library/granite9:cloud"></a>"#},
        format!("{registry}/manifests/8b"): {"status":200,"json":{"config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":format!("sha256:{config}")},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":format!("sha256:{model}"),"size":4_000_000_000_u64}]}},
        format!("{registry}/blobs/sha256:{config}"): {"status":200,"json":{"model_family":"granite","model_type":"8.0B","file_type":"Q4_K_M"}},
        format!("{registry}/blobs/sha256:{model}"): {"status":206,"hex":hex(&gguf(8_000_000_000))},
    });
    fs::write(root.join("upstream.json"), fixture.to_string()).unwrap();
}

fn command(root: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_llmup-catalog-admit"))
        .current_dir(root)
        .env("PATH", "")
        .args([
            "--catalog-path",
            "models.json",
            "--evidence-path",
            "evidence.json",
            "--state-path",
            "state.json",
            "--fixture",
            "upstream.json",
            "--now",
            NOW,
        ])
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn first_run_admits_writes_evidence_and_state_then_reruns_are_byte_stable() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let output = command(root.path(), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["added"], json!(["granite9:8b"]));
    assert_eq!(report["complete"], true);

    let catalog =
        Catalog::parse(&fs::read_to_string(root.path().join("models.json")).unwrap()).unwrap();
    assert_eq!(catalog.schema_version, 3);
    let granite = catalog
        .models
        .iter()
        .find(|model| model.id == "granite9:8b")
        .unwrap();
    assert_eq!(granite.provenance, EntryProvenance::Auto);
    let evidence: Value =
        serde_json::from_str(&fs::read_to_string(root.path().join("evidence.json")).unwrap())
            .unwrap();
    assert_eq!(evidence["observations"][0]["id"], "granite9:8b");
    assert_eq!(evidence["scopes"][0]["name"], "ollama-local-variants");
    assert!(
        root.path().join("state.json").exists(),
        "state file is created on the first run"
    );

    let snapshot = |name: &str| fs::read(root.path().join(name)).unwrap();
    let before = ["models.json", "evidence.json", "state.json"].map(snapshot);
    let rerun = command(root.path(), &[]);
    assert!(
        rerun.status.success(),
        "{}",
        String::from_utf8_lossy(&rerun.stderr)
    );
    let rerun_report: Value = serde_json::from_slice(&rerun.stdout).unwrap();
    assert_eq!(rerun_report["reverified"], json!(["granite9:8b"]));
    assert_eq!(
        ["models.json", "evidence.json", "state.json"].map(snapshot),
        before
    );
}

#[test]
fn dry_run_reports_without_writing() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let models = fs::read(root.path().join("models.json")).unwrap();
    let output = command(root.path(), &["--dry-run"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("catalog-admit dry-run: added=1"));
    assert_eq!(fs::read(root.path().join("models.json")).unwrap(), models);
    assert!(!root.path().join("state.json").exists());
}
