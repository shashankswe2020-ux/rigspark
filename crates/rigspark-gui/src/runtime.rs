use rigspark_runtime::{
    adapters::ServeRequest,
    native_runtime::NativeRuntime,
    state::{Config, ServerState},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;
#[derive(Default)]
pub struct RuntimeController {
    owned: BTreeMap<String, ServerState>,
}
fn view(name: &str, installed: bool, running: bool, owned: bool, endpoint: String) -> Value {
    let daemon = name == "ollama";
    json!({"name":name,"installed":installed,"running":running,"ownedByUs":owned,"endpoint":installed.then_some(endpoint),"canStart":installed&&daemon,"canStop":running&&daemon,"detail":if !installed && name == "bitnet" {"Set RIGSPARK_BITNET_SERVER to the absolute path of a compiled BitNet llama-server."}else if !installed{"Not installed."}else if !daemon{"Runs per model; start a model from Models."}else{""}})
}
fn request(name: &str) -> ServeRequest {
    ServeRequest {
        model_id: "llmup-daemon".into(),
        endpoint: format!(
            "http://127.0.0.1:{}",
            match name {
                "ollama" => 11434,
                "llamacpp" | "bitnet" => 8080,
                "mlx" => 8081,
                _ => 1234,
            }
        ),
        model_path: None,
        context: None,
        cache: None,
    }
}
impl RuntimeController {
    pub async fn list(
        &self,
        config: Config,
        cancel: &CancellationToken,
    ) -> Result<Vec<Value>, String> {
        let native = NativeRuntime::new(config).map_err(|_| "runtime initialization failed")?;
        let adapters = native.adapters();
        let registry = adapters.registry();
        let (hardware, _) = rigspark_runtime::hardware::detect()
            .await
            .map_err(|_| "hardware unavailable")?;
        let probes = rigspark_runtime::diagnostics::probe_backends(&hardware).await;
        let mut views = Vec::new();
        for probe in probes {
            let request = request(&probe.name);
            let running = if probe.installed {
                tokio::time::timeout(
                    std::time::Duration::from_millis(700),
                    registry
                        .get(&probe.name)
                        .map_err(|_| "unknown runtime")?
                        .ready(&request, cancel),
                )
                .await
                .is_ok_and(|result| result.is_ok())
            } else {
                false
            };
            views.push(view(
                &probe.name,
                probe.installed,
                running,
                self.owned.contains_key(&probe.name),
                request.endpoint,
            ));
        }
        Ok(views)
    }
    pub async fn change(
        &mut self,
        config: Config,
        name: &str,
        start: bool,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        if name != "ollama" {
            return Err("runtime requires model activation".into());
        }
        let native =
            NativeRuntime::new(config.clone()).map_err(|_| "runtime initialization failed")?;
        let adapters = native.adapters();
        let registry = adapters.registry();
        let adapter = registry.get(name).map_err(|_| "unknown runtime")?;
        if start {
            let handle = adapter
                .serve(&request(name), cancel)
                .await
                .map_err(|_| "runtime start failed")?;
            if handle.owned_by_us {
                self.owned.insert(name.into(), handle);
            }
        } else {
            let owned = self.owned.get(name).cloned();
            let mut handle = match owned {
                Some(handle) => handle,
                None => adapter
                    .attach_only(&request(name), cancel)
                    .await
                    .map_err(|_| "runtime attachment failed")?,
            };
            handle.owned_by_us = true;
            adapter
                .stop(&handle, cancel)
                .await
                .map_err(|_| "runtime stop failed")?;
            self.owned.remove(name);
            let state = rigspark_runtime::state::StateStore::new(config.clone());
            let guard = state
                .lock(std::time::Duration::from_secs(10))
                .map_err(|_| "state lock failed")?;
            let mut current = state.read().map_err(|_| "state read failed")?;
            if current.active.as_ref().is_some_and(|active| {
                active.backend == name
                    && active.pid == handle.pid
                    && active.process_started_at == handle.process_started_at
            }) {
                current.active = None;
                state
                    .write(&guard, &current)
                    .map_err(|_| "state update failed")?;
            }
            guard.release().map_err(|_| "state unlock failed")?;
        }
        self.list(config, cancel)
            .await?
            .into_iter()
            .find(|entry| entry["name"] == name)
            .ok_or_else(|| "runtime missing".into())
    }
    pub async fn shutdown(&mut self, config: Config) {
        let Ok(native) = NativeRuntime::new(config) else {
            return;
        };
        let adapters = native.adapters();
        let registry = adapters.registry();
        for (name, handle) in std::mem::take(&mut self.owned) {
            if let Ok(adapter) = registry.get(&name) {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    adapter.stop(&handle, &CancellationToken::new()),
                )
                .await;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_views_only_offer_daemon_controls_for_installed_ollama() {
        let endpoint = || "http://127.0.0.1:11434".to_owned();
        let stopped = view("ollama", true, false, false, endpoint());
        assert_eq!(
            (
                stopped["canStart"].as_bool(),
                stopped["running"].as_bool(),
                stopped["canStop"].as_bool()
            ),
            (Some(true), Some(false), Some(false))
        );
        let missing = view("ollama", false, false, false, endpoint());
        assert_eq!(
            (missing["canStart"].as_bool(), missing["endpoint"].is_null()),
            (Some(false), true)
        );
        let per_model = view(
            "llamacpp",
            true,
            true,
            false,
            "http://127.0.0.1:8080".into(),
        );
        assert_eq!(
            (
                per_model["canStart"].as_bool(),
                per_model["canStop"].as_bool()
            ),
            (Some(false), Some(false))
        );
        assert!(per_model["detail"].as_str().unwrap().contains("per model"));
        let external = view("ollama", true, true, false, endpoint());
        assert_eq!(
            (
                external["ownedByUs"].as_bool(),
                external["canStop"].as_bool()
            ),
            (Some(false), Some(true))
        );
        assert_eq!(
            view("ollama", true, true, true, endpoint())["ownedByUs"],
            true
        );
    }
    #[tokio::test]
    async fn per_model_runtimes_are_refused_before_any_runtime_access() {
        let home = tempfile::tempdir().unwrap();
        let config = Config::from_home(home.path()).unwrap();
        let mut controller = RuntimeController::default();
        for name in ["mlx", "llamacpp", "lmstudio", "bitnet", "bogus"] {
            for start in [true, false] {
                let error = controller
                    .change(config.clone(), name, start, &CancellationToken::new())
                    .await
                    .unwrap_err();
                assert_eq!(error, "runtime requires model activation", "{name}");
            }
        }
        assert!(controller.owned.is_empty());
    }
}
