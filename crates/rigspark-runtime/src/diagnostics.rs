use crate::hardware::command_output;
use rigspark_core::{
    advice::{hardware_score, unified},
    catalog::{BACKENDS, Catalog},
    reports::{gib, strip_control, table},
    sizing::{Hardware, memory_capacity},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendInfo {
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub is_default: bool,
    pub install_hint: String,
}

pub async fn probe_backends(hardware: &Hardware) -> Vec<BackendInfo> {
    probe_available_backends(Some(hardware)).await
}

pub async fn probe_available_backends(hardware: Option<&Hardware>) -> Vec<BackendInfo> {
    let apple = hardware.is_some_and(unified);
    probe_with(apple, &SystemBackendProbe).await
}

#[async_trait::async_trait]
trait BackendProbe: Sync {
    async fn installed(&self, name: &str, program: &str) -> Result<bool, String>;
    async fn version(&self, name: &str, program: &str, args: &[&str]) -> Result<String, String>;
}

struct SystemBackendProbe;

#[async_trait::async_trait]
impl BackendProbe for SystemBackendProbe {
    async fn installed(&self, name: &str, program: &str) -> Result<bool, String> {
        if name == "bitnet" {
            return crate::process_control::resolve_bitnet_server().map(|_| true);
        }
        if name == "mlx" {
            return command_output(program, &["-c", "import mlx_lm"])
                .await
                .map(|_| true)
                .map_err(|error| error.to_string());
        }
        Ok(crate::process_control::resolve_binary(program).is_ok())
    }

    async fn version(&self, name: &str, program: &str, args: &[&str]) -> Result<String, String> {
        if name == "bitnet" {
            let path = crate::process_control::resolve_bitnet_server()?;
            return command_output(path.to_str().ok_or("invalid BitNet executable path")?, args)
                .await
                .map_err(|error| error.to_string());
        }
        command_output(program, args)
            .await
            .map_err(|error| error.to_string())
    }
}

async fn probe_with(apple: bool, probe: &impl BackendProbe) -> Vec<BackendInfo> {
    let mut result = Vec::new();
    for name in BACKENDS {
        let (program, args): (&str, &[&str]) = match name {
            "ollama" => ("ollama", &["--version"]),
            "llamacpp" => ("llama-server", &["--version"]),
            "bitnet" => ("bitnet", &["--version"]),
            "mlx" => (
                "python3",
                &[
                    "-c",
                    "import importlib.metadata; print(importlib.metadata.version('mlx-lm'))",
                ],
            ),
            _ => ("lms", &["--version"]),
        };
        let installed = if name == "mlx" && !apple {
            false
        } else {
            probe.installed(name, program).await.unwrap_or(false)
        };
        let version = if installed {
            probe
                .version(name, program, args)
                .await
                .ok()
                .map(|text| strip_control(text.trim()))
                .filter(|text| !text.is_empty())
        } else {
            None
        };
        result.push(BackendInfo {
            name: name.into(),
            installed,
            version,
            is_default: false,
            install_hint: match name {
                "ollama" => "https://ollama.com/download",
                "llamacpp" => "https://github.com/ggml-org/llama.cpp",
                "mlx" => "python3 -m pip install mlx-lm",
                "bitnet" => "Build https://github.com/microsoft/BitNet and set RIGSPARK_BITNET_SERVER to its absolute llama-server path",
                _ => "https://lmstudio.ai/download",
            }
            .into(),
        });
    }
    let priority: &[&str] = if apple {
        &["mlx", "ollama", "llamacpp"]
    } else {
        &["ollama", "llamacpp"]
    };
    let selected = priority
        .iter()
        .find(|name| {
            result
                .iter()
                .any(|entry| entry.name == **name && entry.installed)
        })
        .copied();
    for entry in &mut result {
        entry.is_default = selected == Some(entry.name.as_str());
    }
    result
}

pub fn report(
    catalog: &Catalog,
    hardware: &Hardware,
    backends: &[BackendInfo],
    state_exists: bool,
) -> Value {
    report_inputs(Ok(catalog), Ok(hardware), backends, state_exists)
}

fn hardware_check(hardware: &Hardware) -> Value {
    let (_, usable) = memory_capacity(hardware);
    let summary = format!(
        "{}/{}, {} usable memory, {} free disk",
        if hardware.arch == rigspark_core::sizing::CpuArch::Arm64 {
            "arm64"
        } else {
            "x64"
        },
        match hardware.platform {
            rigspark_core::sizing::Platform::Darwin => "darwin",
            rigspark_core::sizing::Platform::Win32 => "win32",
            _ => "linux",
        },
        gib(usable),
        gib(hardware.free_disk_bytes)
    );
    if usable < 1073741824.0 {
        json!({"name":"hardware","status":"fail","detail":format!("insufficient usable memory to run a model ({summary})")})
    } else {
        json!({"name":"hardware","status":"ok","detail":summary})
    }
}

fn catalog_check(catalog: &Catalog) -> Value {
    let unverified: Vec<_> = catalog
        .models
        .iter()
        .flat_map(|model| {
            model
                .quantizations
                .iter()
                .filter(|quant| quant.digest_verified == Some(false))
                .map(|quant| {
                    format!(
                        "{} ({})",
                        strip_control(&model.id),
                        strip_control(&quant.name)
                    )
                })
        })
        .collect();
    if catalog.models.is_empty() {
        json!({"name":"catalog", "status":"warn", "detail":"catalog contains no models"})
    } else {
        json!({"name":"catalog","status":if unverified.is_empty() { "ok" } else { "warn" },"detail":if unverified.is_empty() { format!("{} model(s), all digests verified",catalog.models.len()) } else { format!("{} quantization(s) have digestVerified:false — size-only verify: {}",unverified.len(),unverified.join(", ")) }})
    }
}

pub fn report_inputs(
    catalog: Result<&Catalog, &str>,
    hardware: Result<&Hardware, &str>,
    backends: &[BackendInfo],
    state_exists: bool,
) -> Value {
    let failed = |name: &str, prefix: &str, error: &str| json!({"name":name,"status":"fail","detail":format!("{prefix}: {}",strip_control(error))});
    let hardware_check = hardware
        .map(hardware_check)
        .unwrap_or_else(|error| failed("hardware", "hardware detection failed", error));
    let catalog_check = catalog
        .map(catalog_check)
        .unwrap_or_else(|error| failed("catalog", "catalog is unusable", error));
    let backend = backends.first().map_or(json!({"name":"backend","status":"fail","detail":"no backend registered"}), |backend| json!({"name":"backend","status":if backend.installed { "ok" } else { "fail" },"detail":if backend.installed { format!("{} is installed",backend.name) } else { format!("{} is not installed — run: {}",backend.name,backend.install_hint) }}));
    let state = if state_exists {
        json!({"name":"state","status":"warn","detail":"runtime state exists; active-server readiness not checked by the offline Rust doctor (Checkpoint 3)"})
    } else {
        json!({"name":"state","status":"ok","detail":"no active server recorded"})
    };
    let checks = vec![hardware_check, backend, catalog_check, state];
    json!({"ok":checks.iter().all(|check| check["status"] != "fail"),"checks":checks,"hardwareScore":hardware.ok().map(hardware_score),"backends":backends})
}

pub fn format_report(report: &Value) -> String {
    let checks = report["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|check| {
            vec![
                check["name"].as_str().unwrap_or("").into(),
                check["status"].as_str().unwrap_or("").to_uppercase(),
                check["detail"].as_str().unwrap_or("").into(),
            ]
        })
        .collect();
    let backends = report["backends"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|backend| {
            vec![
                backend["name"].as_str().unwrap_or("").into(),
                if backend["installed"] == true {
                    "yes".into()
                } else {
                    "no".into()
                },
                backend["version"].as_str().unwrap_or("unknown").into(),
                if backend["isDefault"] == true {
                    "yes".into()
                } else {
                    "".into()
                },
                if backend["installed"] == true {
                    "".into()
                } else {
                    format!(
                        "not installed — run: {}",
                        backend["installHint"].as_str().unwrap_or("")
                    )
                },
            ]
        })
        .collect();
    let bottleneck = match report["hardwareScore"]["bottleneck"].as_str().unwrap_or("") {
        "vram" => "VRAM",
        "ram" => "RAM",
        "compute" => "Compute",
        "storage" => "Storage",
        _ => "unknown",
    };
    let score = report["hardwareScore"]["total"]
        .as_f64()
        .map_or_else(|| "unknown".into(), |score| format!("{score}/100"));
    format!(
        "{}\n\nBackends\n{}\n\nAI Hardware Score: {}\nPrimary bottleneck: {bottleneck}\n\n{}\n",
        table(
            &[("Check", false), ("Status", false), ("Detail", false)],
            checks
        ),
        table(
            &[
                ("Backend", false),
                ("Installed", false),
                ("Version", false),
                ("Default", false),
                ("Detail", false)
            ],
            backends
        ),
        score,
        if report["ok"] == true {
            "All checks passed."
        } else {
            "Problems found — see FAIL rows above."
        }
    )
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    struct Probe {
        absent: bool,
    }

    #[async_trait::async_trait]
    impl BackendProbe for Probe {
        async fn installed(&self, name: &str, _program: &str) -> Result<bool, String> {
            if name == "llamacpp" {
                return Err("broken discovery".into());
            }
            Ok(!self.absent)
        }
        async fn version(
            &self,
            name: &str,
            _program: &str,
            _args: &[&str],
        ) -> Result<String, String> {
            if name == "ollama" {
                return Err("broken version".into());
            }
            Ok("  version\u{1b}[31m\nspoof  ".into())
        }
    }

    #[tokio::test]
    async fn backend_probes_isolate_failures_preserve_installation_and_choose_platform_default() {
        for apple in [false, true] {
            let rows = probe_with(apple, &Probe { absent: false }).await;
            assert_eq!(rows.len(), 5);
            let bitnet = rows.iter().find(|row| row.name == "bitnet").unwrap();
            assert!(bitnet.installed);
            assert!(!bitnet.is_default);
            assert!(bitnet.install_hint.contains("RIGSPARK_BITNET_SERVER"));
            let ollama = rows.iter().find(|row| row.name == "ollama").unwrap();
            assert!(ollama.installed);
            assert!(ollama.version.is_none());
            assert!(
                !rows
                    .iter()
                    .find(|row| row.name == "llamacpp")
                    .unwrap()
                    .installed
            );
            assert_eq!(
                rows.iter().find(|row| row.name == "mlx").unwrap().installed,
                apple
            );
            let defaults: Vec<_> = rows
                .iter()
                .filter(|row| row.is_default)
                .map(|row| row.name.as_str())
                .collect();
            assert_eq!(defaults, vec![if apple { "mlx" } else { "ollama" }]);
            for row in rows {
                assert!(!row.install_hint.is_empty());
                if let Some(version) = row.version {
                    assert!(!version.contains(['\u{1b}', '\n']));
                }
            }
        }
        let rows = probe_with(true, &Probe { absent: true }).await;
        assert!(
            rows.iter()
                .all(|row| !row.installed && !row.is_default && row.version.is_none())
        );
    }

    struct StudioOnly;
    #[async_trait::async_trait]
    impl BackendProbe for StudioOnly {
        async fn installed(&self, name: &str, _program: &str) -> Result<bool, String> {
            Ok(name == "lmstudio")
        }
        async fn version(&self, _: &str, _: &str, _: &[&str]) -> Result<String, String> {
            Ok("0.3.0".into())
        }
    }

    #[tokio::test]
    async fn attach_only_lm_studio_is_never_the_auto_selected_default() {
        for apple in [false, true] {
            let rows = probe_with(apple, &StudioOnly).await;
            assert!(
                rows.iter()
                    .any(|row| row.name == "lmstudio" && row.installed)
            );
            assert!(rows.iter().all(|row| !row.is_default));
        }
    }
}
