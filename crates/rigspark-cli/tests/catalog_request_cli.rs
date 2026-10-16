use rigspark_core::catalog::Catalog;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

const NOW: &str = "2026-10-13T00:00:00Z";
const REPOSITORY: &str = "owner/Requested-1B-GGUF";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FILE: &str = "requested-1b-q4_k_m.gguf";
const SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn gguf() -> Vec<u8> {
    let mut out = b"GGUF".to_vec();
    out.extend(3u32.to_le_bytes());
    out.extend(1u64.to_le_bytes());
    let kv: [(&str, Result<&str, u32>); 3] = [
        ("general.architecture", Ok("requested")),
        ("general.license", Ok("apache-2.0")),
        ("requested.context_length", Err(32768)),
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
    string(&mut out, "output.weight");
    out.extend(1u32.to_le_bytes());
    out.extend(1_000_000_000u64.to_le_bytes());
    out.extend(12u32.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out
}

fn setup(root: &Path, runnable: bool) {
    fs::write(root.join("generation.json"), rigspark_core::GENERATION_JSON).unwrap();
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
    fs::write(
        root.join("event.json"),
        json!({
            "issue": {
                "number": 321,
                "html_url": "https://github.com/shashankswe2020-ux/rigspark/issues/321",
                "body": format!("### Hugging Face model URL\n\nhttps://huggingface.co/{REPOSITORY}\n\n### Reason or use case\n\nTest")
            }
        })
        .to_string(),
    )
    .unwrap();
    let siblings = if runnable {
        json!([{"rfilename":FILE,"lfs":{"sha256":SHA256,"size":700_000_000}}])
    } else {
        json!([{"rfilename":"model.safetensors"}])
    };
    let metadata = json!({
        "id": REPOSITORY,
        "sha": REVISION,
        "tags": ["gguf","text-generation","base_model:owner/Requested-1B","license:apache-2.0"],
        "gated": false,
        "private": false,
        "siblings": siblings,
        "pipeline_tag": "text-generation"
    });
    let mut fixture = serde_json::Map::new();
    fixture.insert(
        format!("https://huggingface.co/api/models/{REPOSITORY}?blobs=true"),
        json!({"status":200,"json":metadata}),
    );
    if runnable {
        fixture.insert(
            format!("https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{FILE}"),
            json!({"status":206,"hex":hex(&gguf())}),
        );
    }
    fs::write(
        root.join("upstream.json"),
        Value::Object(fixture).to_string(),
    )
    .unwrap();
}

fn command(root: &Path, dry_run: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-request"));
    command.current_dir(root).env("PATH", "").args([
        "--catalog-path",
        "models.json",
        "--evidence-path",
        "evidence.json",
        "--generation-path",
        "generation.json",
        "--event-path",
        "event.json",
        "--fixture",
        "upstream.json",
        "--now",
        NOW,
    ]);
    if dry_run {
        command.arg("--dry-run");
    }
    command.output().unwrap()
}

#[test]
fn eligible_request_writes_catalog_and_one_evidence_observation() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path(), true);
    let output = command(root.path(), false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["disposition"], "eligible");
    assert_eq!(report["issueNumber"], 321);
    let catalog =
        Catalog::parse(&fs::read_to_string(root.path().join("models.json")).unwrap()).unwrap();
    assert!(
        catalog
            .models
            .iter()
            .any(|model| model.id == "owner/requested-1b:q4_k_m")
    );
    let evidence: Value =
        serde_json::from_str(&fs::read_to_string(root.path().join("evidence.json")).unwrap())
            .unwrap();
    assert_eq!(evidence["observations"].as_array().unwrap().len(), 1);
    assert_eq!(
        evidence["observations"][0]["id"],
        "owner/requested-1b:q4_k_m"
    );
}

#[test]
fn blocked_and_dry_run_requests_leave_inputs_byte_stable() {
    for (runnable, dry_run, disposition) in [
        (false, false, "needsRunnableArtifact"),
        (true, true, "eligible"),
    ] {
        let root = tempfile::tempdir().unwrap();
        setup(root.path(), runnable);
        let before =
            ["models.json", "evidence.json"].map(|name| fs::read(root.path().join(name)).unwrap());
        let output = command(root.path(), dry_run);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["disposition"], disposition);
        assert_eq!(
            ["models.json", "evidence.json"].map(|name| fs::read(root.path().join(name)).unwrap()),
            before
        );
    }
}

#[test]
fn image_request_reports_pinned_lineage_without_mutating_any_catalog_file() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path(), false);
    let mut catalog =
        rigspark_core::generation::GenerationCatalog::parse(rigspark_core::GENERATION_JSON)
            .unwrap();
    catalog
        .models
        .retain(|model| model.family == "ming-image" || model.default);
    for model in catalog
        .models
        .iter_mut()
        .filter(|model| model.family == "ming-image")
    {
        model.workflow = None;
        model.workflow_source = None;
    }
    fs::write(
        root.path().join("generation.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    fs::write(root.path().join("event.json"), json!({"issue":{
            "number":317,
            "html_url":"https://github.com/shashankswe2020-ux/rigspark/issues/317",
            "body":"### Hugging Face model URL\n\nhttps://huggingface.co/inclusionAI/Ming-Image-0.1-Design\n\n### Reason or use case\n\nneed for UI/UX design"
        }}).to_string()).unwrap();
    let recorded: Value = serde_json::from_str(include_str!(
        "../../rigspark-core/fixtures/admission/hf-models.json"
    ))
    .unwrap();
    let package = &recorded["Comfy-Org/Ming-Image"];
    let revision = package["sha"].as_str().unwrap();
    let mut upstream = json!({
        "https://huggingface.co/api/models/inclusionAI/Ming-Image-0.1-Design?blobs=true": {
            "status":200, "json":{
                "id":"inclusionAI/Ming-Image-0.1-Design", "sha":REVISION,
                "private":false, "gated":false, "pipeline_tag":"text-to-image",
                "tags":[], "siblings":[]
            }
        },
        format!("https://huggingface.co/api/models/Comfy-Org/Ming-Image/revision/{revision}?blobs=true"): {
            "status":200, "json":package
        }
    });
    for model in catalog.models.iter().filter(|model| model.default) {
        for file in &model.files {
            let url = format!(
                "https://huggingface.co/api/models/{}/revision/{}?blobs=true",
                file.repo, file.revision,
            );
            upstream[url] = json!({"status":200, "json":{
                "id":file.repo, "sha":file.revision, "private":false, "gated":false,
                "tags":[], "siblings":[]
            }});
        }
    }
    fs::write(root.path().join("upstream.json"), upstream.to_string()).unwrap();
    let names = ["models.json", "evidence.json", "generation.json"];
    let before = names.map(|name| fs::read(root.path().join(name)).unwrap());
    for dry_run in [false, true] {
        let output = command(root.path(), dry_run);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["issueNumber"], 317);
        assert_eq!(report["disposition"], "needsWorkflow");
        assert_eq!(report["reasonCode"], "GENERATION_WORKFLOW_MISSING");
        assert_eq!(
            report["modelIds"].as_array().unwrap().len(),
            catalog
                .models
                .iter()
                .filter(|model| model.family == "ming-image")
                .count()
        );
        assert_eq!(report["changedFiles"], json!([]));
        assert_eq!(
            names.map(|name| fs::read(root.path().join(name)).unwrap()),
            before
        );
    }
}
