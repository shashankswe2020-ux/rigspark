use std::process::{Command, Stdio};

const HARDWARE: &str = r#"{"arch":"x64","platform":"linux","totalRamBytes":17179869184,"freeRamBytes":12000000000,"freeDiskBytes":500000000000,"gpu":[]}"#;

#[test]
fn public_aliases_report_bitnet_offline_and_explain_missing_server_before_download() {
    for binary in [env!("CARGO_BIN_EXE_llmup"), env!("CARGO_BIN_EXE_rigspark")] {
        let root = tempfile::tempdir().unwrap();
        let command = || {
            let mut command = Command::new(binary);
            command
                .env("RIGSPARK_HOME", root.path().join("home"))
                .env("PATH", "")
                .env("TERM", "dumb")
                .env_remove("RIGSPARK_TUI")
                .env_remove("RIGSPARK_BACKEND")
                .env_remove("RIGSPARK_BITNET_SERVER")
                .stdin(Stdio::null());
            command
        };
        let output = command()
            .args([
                "can-run",
                "bitnet-b1.58:2b",
                "--backend",
                "bitnet",
                "--json",
                "--hardware-json",
                HARDWARE,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["backends"], serde_json::json!(["bitnet"]));
        assert_eq!(value["throughput"]["known"], false);
        assert!(!root.path().join("home").exists());
        let output = command()
            .args([
                "plan",
                "bitnet-b1.58:2b",
                "--json",
                "--hardware-json",
                HARDWARE,
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(plan["backend"], "bitnet");
        assert_eq!(plan["paths"].as_array().unwrap().len(), 1);
        assert_eq!(plan["recommended"], "cpu");
        let output = command()
            .args([
                "up",
                "bitnet-b1.58:2b",
                "--backend",
                "bitnet",
                "--hardware-json",
                HARDWARE,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("RIGSPARK_BITNET_SERVER"));
        assert!(!root.path().join("home/cache").exists());
        assert!(!root.path().join("home/state.json").exists());
    }
}
