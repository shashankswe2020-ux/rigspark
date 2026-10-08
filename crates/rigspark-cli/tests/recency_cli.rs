use serde_json::json;
use std::process::Command;

const HARDWARE: &str = r#"{"arch":"x64","platform":"linux","totalRamBytes":68719476736,"freeRamBytes":60000000000,"freeDiskBytes":500000000000,"gpu":[{"vendor":"nvidia","vramBytes":25769803776}]}"#;

fn catalog(root: &std::path::Path) -> std::path::PathBuf {
    let model = |id: &str, release: Option<&str>, added: Option<&str>| {
        let mut value = json!({
            "id":id,"family":"fixture","params":"8B","architecture":"dense",
            "license":"apache-2.0","openWeight":true,"contextLength":8192,
            "capabilities":["chat"],"source":{"ollama":id},
            "quantizations":[{"name":"Q4_K_M","diskBytes":5368709120_u64,"minRamBytes":6442450944_u64,
                "minVramBytes":6442450944_u64,"sha256":"a".repeat(64)}]
        });
        if let Some(day) = release {
            value["releaseDate"] = json!(day);
        }
        if let Some(day) = added {
            value["addedAt"] = json!(day);
            value["provenance"] = json!("auto");
        }
        value
    };
    let path = root.join("catalog.json");
    let catalog = json!({"schemaVersion":3,"generatedAt":"2026-10-07T00:00:00.000Z","models":[
        model("fresh:8b", Some("2026-09-20"), None),
        model("auto:8b", None, Some("2026-10-06")),
        model("old:8b", Some("2026-05-01"), None),
    ]});
    std::fs::write(&path, catalog.to_string()).unwrap();
    path
}

fn run(args: &[&str], path: &std::path::Path) -> String {
    let root = path.parent().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(args)
        .arg("--catalog-path")
        .arg(path)
        .args([
            "--hardware-json",
            HARDWARE,
            "--no-tui",
            "--today",
            "2026-10-07",
        ])
        .env("PATH", "")
        .env("RIGSPARK_HOME", root.join("home"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn month_window_filters_recommend_and_catalog_by_release_or_added_date() {
    let root = tempfile::tempdir().unwrap();
    let path = catalog(root.path());
    let text = run(&["catalog", "--all", "--month", "1"], &path);
    assert!(
        text.contains("fresh:8b") && text.contains("auto:8b"),
        "{text}"
    );
    assert!(!text.contains("old:8b"), "{text}");
    assert!(
        text.contains("added 2026-10-06"),
        "the basis is shown: {text}"
    );
    let all = run(&["catalog", "--all"], &path);
    assert!(all.contains("old:8b"));
    let ranked = run(&["recommend", "--month", "1"], &path);
    assert!(
        ranked.contains("fresh:8b") && !ranked.contains("old:8b"),
        "{ranked}"
    );
}

#[test]
fn an_empty_window_says_so_instead_of_showing_older_models() {
    let root = tempfile::tempdir().unwrap();
    let path = catalog(root.path());
    let output = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(["catalog", "--month", "1", "--catalog-path"])
        .arg(&path)
        .args([
            "--hardware-json",
            HARDWARE,
            "--no-tui",
            "--today",
            "2027-06-01",
        ])
        .env("PATH", "")
        .env("RIGSPARK_HOME", root.path().join("home"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.starts_with("No catalog models were released or added in the last 1 month."),
        "{text}"
    );
    let invalid = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(["recommend", "--month", "6", "--no-tui"])
        .env("PATH", "")
        .env("RIGSPARK_HOME", root.path().join("home"))
        .output()
        .unwrap();
    assert!(!invalid.status.success(), "only 1, 2 or 3 months");
}
