use crate::Host;
use rigspark_runtime::{
    harness::{LocalHarness, NativeRemoteTransport},
    harness_registry::HarnessRegistry,
    native_runtime::NativeRuntime,
    opencode::NativeOpenCodeRunner,
    state::{Config, StateStore},
};
use std::{collections::BTreeMap, fmt, path::Path, sync::Arc};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StartupOptions {
    port: u16,
    harness: Option<String>,
    startup_json: bool,
    version: bool,
    advice_json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupError {
    Arguments,
    Harness,
    Configuration,
}

impl fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Arguments => {
                "invalid GUI options (expected --port PORT, --harness NAME, --startup-json, standalone --version, or standalone --advice-json)"
            }
            Self::Harness => "invalid GUI harness",
            Self::Configuration => "cannot initialize GUI configuration",
        })
    }
}

impl std::error::Error for StartupError {}

impl StartupOptions {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, StartupError> {
        let mut options = Self::default();
        let mut seen_port = false;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(flag, value)| (flag, Some(value)));
            match flag {
                "--port" if !seen_port => {
                    let value = inline
                        .map(str::to_owned)
                        .or_else(|| args.next())
                        .ok_or(StartupError::Arguments)?;
                    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err(StartupError::Arguments);
                    }
                    options.port = value.parse().map_err(|_| StartupError::Arguments)?;
                    seen_port = true;
                }
                "--harness" if options.harness.is_none() => {
                    options.harness = Some(
                        inline
                            .map(str::to_owned)
                            .or_else(|| args.next())
                            .ok_or(StartupError::Arguments)?,
                    );
                }
                "--startup-json" if !options.startup_json && inline.is_none() => {
                    options.startup_json = true
                }
                "--version" if !options.version && inline.is_none() => options.version = true,
                "--advice-json" if !options.advice_json && inline.is_none() => {
                    options.advice_json = true
                }
                _ => return Err(StartupError::Arguments),
            }
        }
        if options.version && (seen_port || options.harness.is_some() || options.startup_json) {
            return Err(StartupError::Arguments);
        }
        if options.advice_json
            && (seen_port || options.harness.is_some() || options.startup_json || options.version)
        {
            return Err(StartupError::Arguments);
        }
        Ok(options)
    }

    pub fn advice_json(&self) -> bool {
        self.advice_json
    }

    pub fn version(&self) -> bool {
        self.version
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn needs_environment_harness(&self) -> bool {
        self.startup_json && self.harness.is_none()
    }

    pub async fn create_host(
        &self,
        home: &Path,
        port: u16,
        environment_harness: Option<&str>,
    ) -> Result<Arc<Host>, StartupError> {
        if !self.startup_json && self.harness.is_none() {
            return Host::new(home, port).map_err(|_| StartupError::Configuration);
        }
        let requested = self
            .harness
            .as_deref()
            .or_else(|| {
                self.needs_environment_harness()
                    .then_some(environment_harness)
                    .flatten()
            })
            .unwrap_or("local")
            .trim();
        if requested.is_empty()
            || requested.len() > 64
            || !requested
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        {
            return Err(StartupError::Harness);
        }
        let config = Config::from_home(home).map_err(|_| StartupError::Configuration)?;
        let runtime =
            NativeRuntime::new(config.clone()).map_err(|_| StartupError::Configuration)?;
        let adapters = runtime.adapters();
        let backends = adapters.registry();
        let state = StateStore::new(config);
        let remote = NativeRemoteTransport::new().map_err(|_| StartupError::Configuration)?;
        let mut env: BTreeMap<String, String> = rigspark_runtime::process_control::minimal_env()
            .into_iter()
            .collect();
        for name in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_COMPAT_BASE_URL",
            "OPENAI_COMPAT_API_KEY",
            "RIGSPARK_OPENCODE_UNRESTRICTED",
        ] {
            if let Ok(value) = std::env::var(name) {
                env.insert(name.into(), value);
            }
        }
        let runner = NativeOpenCodeRunner {
            binary: runtime.binary("opencode"),
            env: env.clone(),
            commit_home: Some(home.to_path_buf()),
        };
        let registry = HarnessRegistry::builtins(
            LocalHarness {
                state: &state,
                registry: &backends,
                probe: &runtime.probe,
            },
            &remote,
            &runner,
            &env,
        )
        .map_err(|_| StartupError::Configuration)?;
        let harness = registry
            .get(requested)
            .map_err(|_| StartupError::Harness)?
            .name();
        let host = Host::new(home, port).map_err(|_| StartupError::Configuration)?;
        host.ui.lock().await.harness = harness.into();
        Ok(host)
    }

    pub async fn readiness(&self, host: &Host) -> Result<String, serde_json::Error> {
        if self.startup_json {
            serde_json::to_string(&serde_json::json!({
                "url": host.origin(),
                "harness": host.ui.lock().await.harness,
                "port": host.port,
            }))
        } else {
            Ok(host.origin())
        }
    }
}
