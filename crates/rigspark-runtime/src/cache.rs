use rigspark_core::sizing::KvCacheType;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlashAttention {
    #[default]
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptReuse {
    #[default]
    Off,
    Reuse,
}

impl FlashAttention {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Auto, Self::On, Self::Off]
            .into_iter()
            .find(|kind| kind.name() == value)
    }
}

impl PromptReuse {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Reuse => "reuse",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Off, Self::Reuse]
            .into_iter()
            .find(|kind| kind.name() == value)
    }
}

/// Speculative decoding: `draft` uses the catalog drafter, `ngram` drafts from the
/// context itself, and `auto` resolves to `draft` or `off` before launch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speculative {
    #[default]
    Off,
    Auto,
    Draft,
    Ngram,
}

impl Speculative {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Draft => "draft",
            Self::Ngram => "ngram",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Off, Self::Auto, Self::Draft, Self::Ngram]
            .into_iter()
            .find(|kind| kind.name() == value)
    }
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }
}

pub const MAX_PARALLEL: u8 = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheProfile {
    pub kv_k: KvCacheType,
    pub kv_v: KvCacheType,
    pub flash_attention: FlashAttention,
    pub prompt_reuse: PromptReuse,
    #[serde(skip_serializing_if = "Speculative::is_off")]
    pub speculative: Speculative,
    /// Concurrent request slots; `None` keeps the backend default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<u8>,
}

impl CacheProfile {
    /// Rules that hold on every backend.
    pub fn validate(&self) -> Result<(), CacheError> {
        if self.kv_v != KvCacheType::F16 && self.flash_attention == FlashAttention::Off {
            return Err(CacheError::Invalid(
                "a quantized V cache requires flash attention",
            ));
        }
        if self
            .parallel
            .is_some_and(|slots| !(1..=MAX_PARALLEL).contains(&slots))
        {
            return Err(CacheError::Invalid("parallel slots must be 1 to 16"));
        }
        Ok(())
    }
    pub fn summary(&self) -> String {
        let kv = if self.kv_k == self.kv_v {
            self.kv_k.name().to_owned()
        } else {
            format!("K {} / V {}", self.kv_k.name(), self.kv_v.name())
        };
        let mut text = format!(
            "KV {kv}, flash attention {}, prompt reuse {}",
            self.flash_attention.name(),
            self.prompt_reuse.name()
        );
        if !self.speculative.is_off() {
            text.push_str(&format!(", speculative {}", self.speculative.name()));
        }
        if let Some(slots) = self.parallel {
            text.push_str(&format!(", {slots} slots"));
        }
        text
    }
}

/// Cache options a user asked for; unset fields inherit from the base profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheFlags {
    pub kv: Option<KvCacheType>,
    pub flash_attention: Option<FlashAttention>,
    pub prompt_reuse: Option<PromptReuse>,
    pub speculative: Option<Speculative>,
    pub parallel: Option<u8>,
}

impl CacheFlags {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
    /// `base` with every given flag applied on top.
    pub fn over(self, base: Option<CacheProfile>) -> Option<CacheProfile> {
        if self.is_empty() {
            return base;
        }
        let mut profile = base.unwrap_or_default();
        if let Some(kind) = self.kv {
            profile.kv_k = kind;
            profile.kv_v = kind;
        }
        if let Some(flash) = self.flash_attention {
            profile.flash_attention = flash;
        }
        if let Some(reuse) = self.prompt_reuse {
            profile.prompt_reuse = reuse;
        }
        if let Some(speculative) = self.speculative {
            profile.speculative = speculative;
        }
        if let Some(slots) = self.parallel {
            profile.parallel = Some(slots);
        }
        Some(profile)
    }
}

/// Arguments and environment a cache provider adds to a backend launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnDelta {
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CacheError {
    #[error("invalid cache profile: {0}")]
    Invalid(&'static str),
    #[error("{backend} cannot apply this cache profile: {reason}")]
    Unsupported {
        backend: &'static str,
        reason: &'static str,
    },
    #[error("{backend} cache settings are not verified yet; use the default cache profile")]
    Unverified { backend: &'static str },
}

pub trait CacheProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(
        &self,
        profile: &CacheProfile,
        backend: &str,
        owned: bool,
    ) -> Result<SpawnDelta, CacheError>;
}

/// Providers are compiled in; names never load code or pass raw arguments through.
pub fn provider(name: &str) -> Option<&'static dyn CacheProvider> {
    match name {
        "native" => Some(&NativeCacheProvider),
        _ => None,
    }
}

/// The profile worth recording and applying: `None` when absent or equal to the default.
pub fn effective(profile: Option<&CacheProfile>) -> Option<CacheProfile> {
    profile
        .copied()
        .filter(|profile| *profile != CacheProfile::default())
}

/// The profile a launch applies: flags win, then the running profile for `switch`
/// or the user default for `up`, then the backend default (`None`).
pub fn requested(
    flags: CacheFlags,
    switch: bool,
    running: Option<CacheProfile>,
    user_default: Option<CacheProfile>,
) -> Option<CacheProfile> {
    effective(
        flags
            .over(if switch { running } else { user_default })
            .as_ref(),
    )
}

/// Launch settings for `backend`, refusing profiles it cannot apply before anything starts.
pub fn launch_delta(
    profile: Option<&CacheProfile>,
    backend: &str,
    owned: bool,
) -> Result<SpawnDelta, CacheError> {
    match effective(profile) {
        None => Ok(SpawnDelta::default()),
        Some(profile) => NativeCacheProvider.apply(&profile, backend, owned),
    }
}

/// Cache settings each backend already exposes (verified: llama.cpp b10090, Ollama 0.32.5).
pub struct NativeCacheProvider;

impl CacheProvider for NativeCacheProvider {
    fn name(&self) -> &'static str {
        "native"
    }
    fn apply(
        &self,
        profile: &CacheProfile,
        backend: &str,
        owned: bool,
    ) -> Result<SpawnDelta, CacheError> {
        if !rigspark_core::catalog::BACKENDS.contains(&backend) {
            return Err(CacheError::Invalid("unknown backend"));
        }
        profile.validate()?;
        // An unresolved `auto` asks for nothing by itself.
        let profile = &CacheProfile {
            speculative: match profile.speculative {
                Speculative::Auto => Speculative::Off,
                mode => mode,
            },
            ..*profile
        };
        if *profile == CacheProfile::default() {
            return Ok(SpawnDelta::default());
        }
        match backend {
            "llamacpp" => Ok(llama_cpp(profile)),
            "ollama" => ollama(profile, owned),
            "mlx" => Err(CacheError::Unverified { backend: "mlx" }),
            "bitnet" => Err(CacheError::Unverified { backend: "bitnet" }),
            _ => Err(CacheError::Unsupported {
                backend: "lmstudio",
                reason: "LM Studio is attach-only; change its cache settings in LM Studio",
            }),
        }
    }
}

fn flash_enabled(profile: &CacheProfile) -> Option<bool> {
    match profile.flash_attention {
        FlashAttention::On => Some(true),
        FlashAttention::Off => Some(false),
        FlashAttention::Auto => (profile.kv_v != KvCacheType::F16).then_some(true),
    }
}

fn llama_cpp(profile: &CacheProfile) -> SpawnDelta {
    let mut args = Vec::new();
    for (flag, kind) in [
        ("--cache-type-k", profile.kv_k),
        ("--cache-type-v", profile.kv_v),
    ] {
        if kind != KvCacheType::F16 {
            args.extend([flag.to_string(), kind.name().to_string()]);
        }
    }
    if let Some(enabled) = flash_enabled(profile) {
        args.extend([
            "--flash-attn".into(),
            if enabled { "on" } else { "off" }.into(),
        ]);
    }
    if profile.prompt_reuse == PromptReuse::Reuse {
        args.extend(["--cache-reuse".into(), "256".into()]);
    }
    match profile.speculative {
        Speculative::Draft => args.extend(["--spec-type".into(), "draft-simple".into()]),
        Speculative::Ngram => args.extend(["--spec-type".into(), "ngram-mod".into()]),
        Speculative::Off | Speculative::Auto => {}
    }
    if let Some(slots) = profile.parallel {
        args.extend([
            "--parallel".into(),
            slots.to_string(),
            "--kv-unified".into(),
        ]);
    }
    SpawnDelta {
        args,
        env: BTreeMap::new(),
    }
}

fn ollama(profile: &CacheProfile, owned: bool) -> Result<SpawnDelta, CacheError> {
    let unsupported = |reason| CacheError::Unsupported {
        backend: "ollama",
        reason,
    };
    if profile.kv_k != profile.kv_v {
        return Err(unsupported("Ollama uses one cache type for both K and V"));
    }
    let mut env = BTreeMap::new();
    if let Some(enabled) = flash_enabled(profile) {
        env.insert(
            "OLLAMA_FLASH_ATTENTION".into(),
            if enabled { "1" } else { "0" }.into(),
        );
    }
    if profile.kv_k != KvCacheType::F16 {
        env.insert("OLLAMA_KV_CACHE_TYPE".into(), profile.kv_k.name().into());
    }
    if matches!(profile.speculative, Speculative::Draft | Speculative::Ngram) {
        return Err(unsupported(
            "Ollama ships speculative decoding inside its model tags; use llama.cpp to choose a drafter",
        ));
    }
    if let Some(slots) = profile.parallel {
        env.insert("OLLAMA_NUM_PARALLEL".into(), slots.to_string());
    }
    // Prompt-prefix reuse is Ollama's default behaviour and needs no setting.
    if !env.is_empty() && !owned {
        return Err(unsupported(
            "cache settings are daemon-wide and apply only to an Ollama daemon rigspark starts",
        ));
    }
    Ok(SpawnDelta {
        args: Vec::new(),
        env,
    })
}
