//! Curated, offline catalog of local voice models: text-to-speech ("speak") and
//! speech-to-text ("listen").
//!
//! Kept separate from the LLM and generation catalogs. Fit covers weight memory only;
//! voice speed is never estimated because no sourced throughput data exists.
use crate::{
    catalog::{
        LICENSES, coordinates, date, digest, model_file, parameter_label, parse_document, require,
        timestamp,
    },
    reports::strip_control,
    sizing::{CpuArch, HEADROOM, Hardware, Platform, ValidationError, memory_capacity},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const VOICE_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceKind {
    /// Text to speech: spoken replies and generated audio.
    Speak,
    /// Speech to text: dictation into the composer.
    Listen,
}
impl VoiceKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Speak => "speak",
            Self::Listen => "listen",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "speak" => Some(Self::Speak),
            "listen" => Some(Self::Listen),
            _ => None,
        }
    }
}

/// Local engines RigSpark starts for a voice model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VoiceEngine {
    /// whisper.cpp's `whisper-server`, loading a pinned, verified ggml file.
    WhisperCpp,
    /// `mlx_audio.server` on Apple Silicon; it fetches `engineModel` itself.
    MlxAudio,
}
impl VoiceEngine {
    pub fn name(self) -> &'static str {
        match self {
            Self::WhisperCpp => "whisper-cpp",
            Self::MlxAudio => "mlx-audio",
        }
    }
    pub fn kinds(self) -> &'static [VoiceKind] {
        match self {
            Self::WhisperCpp => &[VoiceKind::Listen],
            Self::MlxAudio => &[VoiceKind::Speak, VoiceKind::Listen],
        }
    }
    /// Whether the engine can run on this hardware at all.
    pub fn supports(self, hardware: &Hardware) -> bool {
        match self {
            Self::WhisperCpp => true,
            Self::MlxAudio => {
                hardware.platform == Platform::Darwin && hardware.arch == CpuArch::Arm64
            }
        }
    }
}

/// How `weightsBytes` was sourced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SizeBasis {
    /// The exact byte size of the pinned file.
    Pinned,
    /// An upper bound; the pinned SHA-256 still decides integrity.
    Ceiling,
    /// The rounded size shown on the model card; the engine downloads the weights.
    ModelCard,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoiceFile {
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    /// False when `bytes` is a ceiling rather than the exact size.
    pub exact_size: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Voice {
    pub id: String,
    pub label: String,
    /// The engine's language code for this voice.
    pub lang: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoiceModel {
    pub id: String,
    pub family: String,
    pub kind: VoiceKind,
    pub engine: VoiceEngine,
    pub params: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default)]
    pub default: bool,
    pub source: String,
    /// The pinned weight file RigSpark downloads and verifies (whisper.cpp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<VoiceFile>,
    /// The repository id the engine loads by itself (mlx-audio).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_model: Option<String>,
    pub weights_bytes: u64,
    pub size_basis: SizeBasis,
    pub languages: Vec<String>,
    #[serde(default)]
    pub voices: Vec<Voice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoiceCatalog {
    pub schema_version: u8,
    pub generated_at: String,
    pub models: Vec<VoiceModel>,
}

fn https(value: &str) -> Result<(), ValidationError> {
    require(
        value.len() <= 2048
            && value.starts_with("https://")
            && value.len() > "https://".len()
            && !value
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace()),
        "source links must be HTTPS",
    )
}
fn model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-:".contains(&byte)
        })
        && VoiceKind::parse(value).is_none()
}
fn short_token(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

impl VoiceModel {
    pub fn voice(&self, id: &str) -> Option<&Voice> {
        self.voices.iter().find(|voice| voice.id == id)
    }
    fn validate(&self) -> Result<(), ValidationError> {
        require(model_id(&self.id), "invalid voice model id")?;
        require(short_token(&self.family, 128), "invalid voice model family")?;
        require(parameter_label(&self.params), "invalid parameter label")?;
        require(
            LICENSES.contains(&self.license.as_str()),
            "voice catalog requires an allowlisted open-weight license",
        )?;
        if let Some(released) = &self.release_date {
            date(released)?;
        }
        https(&self.source)?;
        require(
            self.engine.kinds().contains(&self.kind),
            "engine cannot run this voice model kind",
        )?;
        require(
            self.weights_bytes > 0 && self.weights_bytes <= MAX_SAFE_INTEGER,
            "invalid weight size",
        )?;
        require(
            !self.languages.is_empty()
                && self.languages.len() <= 64
                && self
                    .languages
                    .iter()
                    .all(|language| short_token(language, 32)),
            "invalid voice model languages",
        )?;
        require(self.voices.len() <= 256, "too many voices")?;
        let mut voices = HashSet::new();
        for voice in &self.voices {
            require(
                short_token(&voice.id, 64)
                    && short_token(&voice.lang, 8)
                    && !voice.label.is_empty()
                    && voice.label.len() <= 128
                    && voices.insert(&voice.id),
                "invalid voice",
            )?;
        }
        match self.engine {
            VoiceEngine::WhisperCpp => {
                let file = self.file.as_ref().ok_or_else(|| {
                    ValidationError("whisper.cpp models need a pinned file".into())
                })?;
                require(
                    self.engine_model.is_none(),
                    "whisper.cpp models load a pinned file, not an engine model",
                )?;
                coordinates(&file.repo, &file.revision)?;
                model_file(&file.file, 256)?;
                require(
                    file.file.ends_with(".bin") && !file.file.contains('/'),
                    "whisper.cpp weights must be a top-level ggml .bin file",
                )?;
                digest(&file.sha256)?;
                require(
                    file.bytes > 0 && file.bytes <= MAX_SAFE_INTEGER,
                    "invalid weight file size",
                )?;
                require(
                    self.weights_bytes == file.bytes
                        && self.size_basis
                            == if file.exact_size {
                                SizeBasis::Pinned
                            } else {
                                SizeBasis::Ceiling
                            },
                    "weight size must match the pinned file",
                )?;
            }
            VoiceEngine::MlxAudio => {
                let repository = self.engine_model.as_deref().ok_or_else(|| {
                    ValidationError("mlx-audio models need an engine model".into())
                })?;
                require(self.file.is_none(), "mlx-audio downloads its own weights")?;
                require(
                    repository.len() <= 200
                        && repository.split('/').count() == 2
                        && repository.split('/').all(|part| short_token(part, 100)),
                    "invalid engine model repository",
                )?;
                require(
                    self.size_basis == SizeBasis::ModelCard,
                    "engine-downloaded weights are sized from the model card",
                )?;
            }
        }
        Ok(())
    }
}

impl VoiceCatalog {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let mut result: Self = parse_document(raw)?;
        require(
            result.schema_version == VOICE_SCHEMA_VERSION
                && !result.models.is_empty()
                && result.models.len() <= 256,
            "unsupported or empty voice catalog",
        )?;
        timestamp(&result.generated_at)?;
        let mut ids = HashSet::new();
        let mut defaults = HashSet::new();
        let mut kinds = HashSet::new();
        for model in &result.models {
            model.validate()?;
            require(ids.insert(&model.id), "duplicate voice model id")?;
            kinds.insert(model.kind);
            if model.default {
                require(
                    defaults.insert(model.kind),
                    "each voice kind needs exactly one default model",
                )?;
            }
        }
        require(
            kinds == defaults,
            "each voice kind needs exactly one default model",
        )?;
        for model in &mut result.models {
            for voice in &mut model.voices {
                voice.label = strip_control(&voice.label);
            }
        }
        Ok(result)
    }
    pub fn bundled() -> Result<Self, ValidationError> {
        Self::parse(crate::VOICE_JSON)
    }
    /// Resolves `speak`/`listen` to that kind's default model, otherwise an exact id.
    pub fn resolve(&self, query: &str) -> Result<&VoiceModel, ValidationError> {
        let found = match VoiceKind::parse(query) {
            Some(kind) => self
                .models
                .iter()
                .find(|model| model.kind == kind && model.default),
            None => self.models.iter().find(|model| model.id == query),
        };
        found.ok_or_else(|| ValidationError("unknown voice model".into()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceVerdict {
    Yes,
    No,
}

/// Memory-only fit of a voice model; speed is never estimated.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFit {
    pub verdict: VoiceVerdict,
    pub reason: &'static str,
    pub memory_kind: &'static str,
    pub budget_bytes: f64,
    pub weights_bytes: u64,
    /// Always `"unknown"`: no sourced voice throughput dataset exists.
    pub speed: &'static str,
}

pub fn fit(model: &VoiceModel, hardware: &Hardware) -> VoiceFit {
    let (memory_kind, usable_bytes) = memory_capacity(hardware);
    let budget_bytes = usable_bytes * (1.0 - HEADROOM);
    let (verdict, reason) = if !model.engine.supports(hardware) {
        (
            VoiceVerdict::No,
            "mlx-audio runs only on Apple Silicon Macs",
        )
    } else if model.weights_bytes as f64 <= budget_bytes {
        (VoiceVerdict::Yes, "weights fit in memory")
    } else {
        (VoiceVerdict::No, "weights exceed usable memory")
    };
    VoiceFit {
        verdict,
        reason,
        memory_kind,
        budget_bytes,
        weights_bytes: model.weights_bytes,
        speed: "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sizing::{CpuArch, Platform};

    fn hardware(platform: Platform, arch: CpuArch, ram_gib: f64) -> Hardware {
        Hardware {
            arch,
            platform,
            total_ram_bytes: ram_gib * 1073741824.0,
            free_ram_bytes: ram_gib * 1073741824.0,
            free_disk_bytes: 1e12,
            gpu: Vec::new(),
            unified_memory: None,
        }
    }
    fn catalog() -> serde_json::Value {
        serde_json::from_str(crate::VOICE_JSON).unwrap()
    }
    fn parse(value: &serde_json::Value) -> Result<VoiceCatalog, ValidationError> {
        VoiceCatalog::parse(&value.to_string())
    }

    #[test]
    fn the_bundled_catalog_has_one_default_per_kind() {
        let catalog = VoiceCatalog::bundled().unwrap();
        assert_eq!(catalog.resolve("speak").unwrap().id, "kokoro-82m");
        assert_eq!(
            catalog.resolve("listen").unwrap().id,
            "whisper-large-v3-turbo:q8_0"
        );
        assert_eq!(
            catalog.resolve("chatterbox").unwrap().kind,
            VoiceKind::Speak
        );
        assert!(catalog.resolve("missing").is_err());
        let kokoro = catalog.resolve("kokoro-82m").unwrap();
        assert_eq!(kokoro.voice("af_heart").unwrap().lang, "a");
        assert!(kokoro.voice("nobody").is_none());
    }

    #[test]
    fn engines_fit_only_where_they_run() {
        let catalog = VoiceCatalog::bundled().unwrap();
        let kokoro = catalog.resolve("speak").unwrap();
        let whisper = catalog.resolve("listen").unwrap();
        let mac = hardware(Platform::Darwin, CpuArch::Arm64, 16.0);
        let linux = hardware(Platform::Linux, CpuArch::X64, 16.0);
        assert_eq!(fit(kokoro, &mac).verdict, VoiceVerdict::Yes);
        assert_eq!(fit(kokoro, &linux).verdict, VoiceVerdict::No);
        assert_eq!(fit(whisper, &linux).verdict, VoiceVerdict::Yes);
        assert_eq!(fit(whisper, &linux).speed, "unknown");
        // 0.5 GiB of RAM cannot hold an 874 MB model.
        let tiny = hardware(Platform::Linux, CpuArch::X64, 0.5);
        assert_eq!(fit(whisper, &tiny).verdict, VoiceVerdict::No);
    }

    #[test]
    fn invalid_entries_are_refused() {
        let base = catalog();
        type Change = Box<dyn Fn(&mut serde_json::Value)>;
        let broken: Vec<(&str, Change)> = vec![
            (
                "license",
                Box::new(|c| c["models"][0]["license"] = "proprietary".into()),
            ),
            (
                "kind",
                Box::new(|c| c["models"][0]["engine"] = "whisper-cpp".into()),
            ),
            (
                "listen engine",
                Box::new(|c| c["models"][0]["kind"] = "listen".into()),
            ),
            (
                "two defaults",
                Box::new(|c| c["models"][1]["default"] = true.into()),
            ),
            (
                "no default",
                Box::new(|c| c["models"][0]["default"] = false.into()),
            ),
            (
                "duplicate",
                Box::new(|c| c["models"][1]["id"] = "kokoro-82m".into()),
            ),
            ("id", Box::new(|c| c["models"][1]["id"] = "Speak".into())),
            (
                "kind id",
                Box::new(|c| c["models"][1]["id"] = "speak".into()),
            ),
            (
                "source",
                Box::new(|c| c["models"][0]["source"] = "http://example.com".into()),
            ),
            (
                "sha",
                Box::new(|c| c["models"][2]["file"]["sha256"] = "00".into()),
            ),
            (
                "revision",
                Box::new(|c| c["models"][2]["file"]["revision"] = "main".into()),
            ),
            (
                "nested file",
                Box::new(|c| c["models"][2]["file"]["file"] = "a/b.bin".into()),
            ),
            (
                "extension",
                Box::new(|c| c["models"][2]["file"]["file"] = "model.gguf".into()),
            ),
            (
                "size mismatch",
                Box::new(|c| c["models"][2]["weightsBytes"] = 1.into()),
            ),
            (
                "basis",
                Box::new(|c| c["models"][2]["sizeBasis"] = "ceiling".into()),
            ),
            (
                "mlx file",
                Box::new(|c| c["models"][0]["file"] = c["models"][2]["file"].clone()),
            ),
            (
                "mlx basis",
                Box::new(|c| c["models"][0]["sizeBasis"] = "pinned".into()),
            ),
            (
                "mlx repo",
                Box::new(|c| c["models"][0]["engineModel"] = "one/two/three".into()),
            ),
            (
                "no repo",
                Box::new(|c| {
                    c["models"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("engineModel");
                }),
            ),
            (
                "no file",
                Box::new(|c| {
                    c["models"][2].as_object_mut().unwrap().remove("file");
                }),
            ),
            (
                "zero size",
                Box::new(|c| c["models"][0]["weightsBytes"] = 0.into()),
            ),
            (
                "languages",
                Box::new(|c| c["models"][0]["languages"] = serde_json::json!([])),
            ),
            (
                "voice",
                Box::new(|c| c["models"][0]["voices"][0]["id"] = "bad voice".into()),
            ),
            (
                "duplicate voice",
                Box::new(|c| c["models"][0]["voices"][1]["id"] = "af_heart".into()),
            ),
            (
                "params",
                Box::new(|c| c["models"][0]["params"] = "big".into()),
            ),
            (
                "date",
                Box::new(|c| c["models"][2]["releaseDate"] = "2024-13-01".into()),
            ),
            ("schema", Box::new(|c| c["schemaVersion"] = 2.into())),
            (
                "timestamp",
                Box::new(|c| c["generatedAt"] = "yesterday".into()),
            ),
            (
                "extra field",
                Box::new(|c| c["models"][0]["speed"] = "fast".into()),
            ),
        ];
        assert!(parse(&base).is_ok());
        for (label, change) in broken {
            let mut value = base.clone();
            change(&mut value);
            assert!(parse(&value).is_err(), "{label}");
        }
    }

    #[test]
    fn voice_labels_lose_control_characters() {
        let mut value = catalog();
        value["models"][0]["voices"][0]["label"] = "Heart\u{1b}[31m".into();
        let parsed = parse(&value).unwrap();
        assert_eq!(parsed.models[0].voices[0].label, "Heart");
    }
}
