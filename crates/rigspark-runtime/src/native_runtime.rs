use crate::{
    adapters::{BackendKind, RuntimeAdapter},
    command::NativeCommandRunner,
    http::NativeTransport,
    identity::NativeProcessProbe,
    lifecycle::Registry,
    process_control::{NativeProcessControl, resolve_binary, resolve_bitnet_server},
    special_adapters::{LmStudioAdapter, MlxAdapter},
    state::Config,
};
use std::path::PathBuf;

pub struct NativeRuntime {
    http: NativeTransport,
    pub probe: NativeProcessProbe,
    control: NativeProcessControl,
    commands: NativeCommandRunner,
    home: PathBuf,
    config: Config,
    ollama_models: PathBuf,
}
pub struct NativeAdapters<'runtime> {
    ollama: RuntimeAdapter<'runtime>,
    llama: RuntimeAdapter<'runtime>,
    bitnet: RuntimeAdapter<'runtime>,
    mlx: MlxAdapter<'runtime>,
    studio: LmStudioAdapter<'runtime>,
}
impl NativeRuntime {
    pub fn new(config: Config) -> Result<Self, crate::harness::HarnessError> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .ok_or(crate::harness::HarnessError::Invalid)?;
        let ollama_models = std::env::var_os("OLLAMA_MODELS")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".ollama/models"));
        crate::command::OllamaCommandContext::new("http://127.0.0.1:11434", &ollama_models)
            .map_err(|_| crate::harness::HarnessError::Invalid)?;
        Ok(Self {
            http: NativeTransport::new().map_err(|_| crate::harness::HarnessError::Transport)?,
            probe: NativeProcessProbe,
            control: NativeProcessControl,
            commands: NativeCommandRunner,
            home,
            config,
            ollama_models,
        })
    }
    pub fn binary(&self, name: &str) -> PathBuf {
        let resolved = if name == "bitnet" {
            resolve_bitnet_server()
        } else {
            resolve_binary(name)
        };
        resolved.unwrap_or_else(|_| self.config.home.join(".unavailable").join(name))
    }
    pub fn adapters(&self) -> NativeAdapters<'_> {
        let mut ollama = RuntimeAdapter::new(
            BackendKind::Ollama,
            self.binary("ollama"),
            &self.http,
            &self.probe,
            &self.control,
        );
        ollama.ollama_models = Some(self.ollama_models.clone());
        NativeAdapters {
            ollama,
            llama: RuntimeAdapter::new(
                BackendKind::LlamaCpp,
                self.binary("llama-server"),
                &self.http,
                &self.probe,
                &self.control,
            ),
            bitnet: RuntimeAdapter::new(
                BackendKind::BitNet,
                self.binary("bitnet"),
                &self.http,
                &self.probe,
                &self.control,
            ),
            mlx: MlxAdapter {
                binary: self.binary("python3"),
                http: &self.http,
                probe: &self.probe,
                control: &self.control,
                commands: &self.commands,
                token: None,
            },
            studio: LmStudioAdapter {
                binary: self.binary("lms"),
                trusted_executables: crate::application::studio_trust(&self.home),
                http: &self.http,
                probe: &self.probe,
                commands: &self.commands,
                token: std::env::var("LM_API_TOKEN").ok(),
            },
        }
    }
}
impl NativeAdapters<'_> {
    pub fn registry(&self) -> Registry<'_> {
        Registry::new(vec![
            &self.ollama,
            &self.llama,
            &self.mlx,
            &self.studio,
            &self.bitnet,
        ])
    }
}
