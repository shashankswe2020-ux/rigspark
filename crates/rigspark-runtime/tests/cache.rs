use rigspark_core::sizing::KvCacheType::{self, F16, Q4_0, Q8_0};
use rigspark_runtime::cache::{
    CacheError, CacheProfile, FlashAttention, PromptReuse, SpawnDelta, Speculative, provider,
};
use std::collections::BTreeMap;

fn profile(kv_k: KvCacheType, kv_v: KvCacheType, flash: FlashAttention) -> CacheProfile {
    CacheProfile {
        kv_k,
        kv_v,
        flash_attention: flash,
        prompt_reuse: PromptReuse::Off,
        ..CacheProfile::default()
    }
}

fn apply(profile: &CacheProfile, backend: &str, owned: bool) -> Result<SpawnDelta, CacheError> {
    provider("native").unwrap().apply(profile, backend, owned)
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
fn the_default_profile_changes_nothing_on_any_backend() {
    for backend in ["ollama", "llamacpp", "mlx", "lmstudio"] {
        for owned in [true, false] {
            assert_eq!(
                apply(&CacheProfile::default(), backend, owned).unwrap(),
                SpawnDelta::default(),
                "{backend} owned={owned}"
            );
        }
    }
}

#[test]
fn llama_cpp_receives_exact_cache_flags() {
    let cases = [
        (
            profile(Q8_0, Q8_0, FlashAttention::Auto),
            args(&[
                "--cache-type-k",
                "q8_0",
                "--cache-type-v",
                "q8_0",
                "--flash-attn",
                "on",
            ]),
        ),
        (
            profile(Q4_0, F16, FlashAttention::Auto),
            args(&["--cache-type-k", "q4_0"]),
        ),
        (
            profile(F16, Q4_0, FlashAttention::On),
            args(&["--cache-type-v", "q4_0", "--flash-attn", "on"]),
        ),
        (
            profile(F16, F16, FlashAttention::Off),
            args(&["--flash-attn", "off"]),
        ),
    ];
    for (profile, expected) in cases {
        let delta = apply(&profile, "llamacpp", true).unwrap();
        assert_eq!(delta.args, expected, "{profile:?}");
        assert!(delta.env.is_empty());
    }
    let reuse = CacheProfile {
        prompt_reuse: PromptReuse::Reuse,
        ..CacheProfile::default()
    };
    assert_eq!(
        apply(&reuse, "llamacpp", true).unwrap().args,
        args(&["--cache-reuse", "256"])
    );
}

#[test]
fn a_quantized_v_cache_without_flash_attention_is_rejected() {
    for backend in ["llamacpp", "ollama"] {
        for kind in [Q8_0, Q4_0] {
            assert!(
                matches!(
                    apply(&profile(kind, kind, FlashAttention::Off), backend, true),
                    Err(CacheError::Invalid(_))
                ),
                "{backend} {kind:?}"
            );
        }
    }
    assert!(apply(&profile(Q8_0, F16, FlashAttention::Off), "llamacpp", true).is_ok());
}

#[test]
fn owned_ollama_daemons_receive_cache_environment() {
    assert_eq!(
        apply(&profile(Q8_0, Q8_0, FlashAttention::Auto), "ollama", true).unwrap(),
        SpawnDelta {
            args: vec![],
            env: env(&[
                ("OLLAMA_FLASH_ATTENTION", "1"),
                ("OLLAMA_KV_CACHE_TYPE", "q8_0")
            ]),
        }
    );
    assert_eq!(
        apply(&profile(F16, F16, FlashAttention::Off), "ollama", true)
            .unwrap()
            .env,
        env(&[("OLLAMA_FLASH_ATTENTION", "0")])
    );
    let reuse = CacheProfile {
        prompt_reuse: PromptReuse::Reuse,
        ..CacheProfile::default()
    };
    assert_eq!(
        apply(&reuse, "ollama", false).unwrap(),
        SpawnDelta::default(),
        "Ollama reuses cached prompt prefixes by default"
    );
}

#[test]
fn ollama_refuses_settings_it_cannot_apply() {
    assert!(matches!(
        apply(&profile(Q8_0, Q8_0, FlashAttention::Auto), "ollama", false),
        Err(CacheError::Unsupported {
            backend: "ollama",
            ..
        })
    ));
    assert!(matches!(
        apply(&profile(F16, F16, FlashAttention::On), "ollama", false),
        Err(CacheError::Unsupported {
            backend: "ollama",
            ..
        })
    ));
    assert!(matches!(
        apply(&profile(Q8_0, F16, FlashAttention::Auto), "ollama", true),
        Err(CacheError::Unsupported {
            backend: "ollama",
            ..
        })
    ));
}

#[test]
fn unverified_and_attach_only_backends_fail_closed() {
    let quantized = profile(Q8_0, Q8_0, FlashAttention::Auto);
    assert!(matches!(
        apply(&quantized, "mlx", true),
        Err(CacheError::Unverified { backend: "mlx" })
    ));
    assert!(matches!(
        apply(&quantized, "lmstudio", false),
        Err(CacheError::Unsupported {
            backend: "lmstudio",
            ..
        })
    ));
    assert!(matches!(
        apply(&CacheProfile::default(), "vllm", true),
        Err(CacheError::Invalid(_))
    ));
}

#[test]
fn providers_are_selected_by_name_from_a_fixed_registry() {
    assert_eq!(provider("native").unwrap().name(), "native");
    for name in ["", "Native", "lmcache", "custom", "../native"] {
        assert!(provider(name).is_none(), "{name:?}");
    }
}

#[test]
fn profiles_serialize_with_stable_names_and_reject_unknown_fields() {
    let value = serde_json::to_value(profile(Q8_0, Q4_0, FlashAttention::On)).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"kvK":"q8_0","kvV":"q4_0","flashAttention":"on","promptReuse":"off"})
    );
    let parsed: CacheProfile = serde_json::from_value(value).unwrap();
    assert_eq!(parsed, profile(Q8_0, Q4_0, FlashAttention::On));
    assert_eq!(
        serde_json::from_value::<CacheProfile>(serde_json::json!({})).unwrap(),
        CacheProfile::default()
    );
    assert!(
        serde_json::from_value::<CacheProfile>(serde_json::json!({"kvK":"q8_0","args":"--x"}))
            .is_err()
    );
}

#[test]
fn flags_override_the_running_profile_on_switch_and_the_user_default_on_up() {
    use rigspark_runtime::cache::{CacheFlags, requested};
    let q8 = profile(Q8_0, Q8_0, FlashAttention::Auto);
    let q4_on = profile(Q4_0, Q4_0, FlashAttention::On);
    let none = CacheFlags::default();
    let kv = |kind| CacheFlags {
        kv: Some(kind),
        ..CacheFlags::default()
    };
    assert_eq!(none.over(None), None);
    assert_eq!(none.over(Some(q8)), Some(q8));
    assert_eq!(kv(Q8_0).over(None), Some(q8));
    let flash = CacheFlags {
        flash_attention: Some(FlashAttention::On),
        ..CacheFlags::default()
    };
    assert_eq!(
        flash.over(Some(profile(Q4_0, Q4_0, FlashAttention::Auto))),
        Some(q4_on)
    );
    // up: flag > user default > backend default; the running profile is ignored.
    assert_eq!(requested(none, false, Some(q4_on), Some(q8)), Some(q8));
    assert_eq!(
        requested(kv(Q4_0), false, None, Some(q8)),
        Some(profile(Q4_0, Q4_0, FlashAttention::Auto))
    );
    assert_eq!(requested(none, false, Some(q8), None), None);
    // switch: flag > running profile; the user default never replaces what is running.
    assert_eq!(requested(none, true, Some(q8), Some(q4_on)), Some(q8));
    assert_eq!(requested(none, true, None, Some(q4_on)), None);
    assert_eq!(
        requested(kv(F16), true, Some(q8), None),
        None,
        "f16 clears the profile"
    );
    assert_eq!(requested(kv(Q4_0), true, Some(q4_on), None), Some(q4_on));
}

#[test]
fn names_parse_round_trip_and_summaries_are_plain_text() {
    for flash in [
        FlashAttention::Auto,
        FlashAttention::On,
        FlashAttention::Off,
    ] {
        assert_eq!(FlashAttention::parse(flash.name()), Some(flash));
    }
    for reuse in [PromptReuse::Off, PromptReuse::Reuse] {
        assert_eq!(PromptReuse::parse(reuse.name()), Some(reuse));
    }
    assert_eq!(FlashAttention::parse("yes"), None);
    assert_eq!(PromptReuse::parse(""), None);
    assert_eq!(
        profile(Q8_0, Q8_0, FlashAttention::Auto).summary(),
        "KV q8_0, flash attention auto, prompt reuse off"
    );
    assert_eq!(
        profile(Q8_0, F16, FlashAttention::Off).summary(),
        "KV K q8_0 / V f16, flash attention off, prompt reuse off"
    );
    assert!(profile(F16, Q4_0, FlashAttention::Off).validate().is_err());
    profile(Q4_0, F16, FlashAttention::Off).validate().unwrap();
}

fn launch(speculative: Speculative, parallel: Option<u8>) -> CacheProfile {
    CacheProfile {
        speculative,
        parallel,
        ..CacheProfile::default()
    }
}

#[test]
fn llama_cpp_receives_speculative_and_slot_flags() {
    assert_eq!(
        apply(&launch(Speculative::Draft, None), "llamacpp", true)
            .unwrap()
            .args,
        args(&["--spec-type", "draft-simple"]),
        "the adapter adds the verified --model-draft path"
    );
    assert_eq!(
        apply(&launch(Speculative::Ngram, None), "llamacpp", true)
            .unwrap()
            .args,
        args(&["--spec-type", "ngram-mod"])
    );
    assert_eq!(
        apply(&launch(Speculative::Auto, None), "llamacpp", true).unwrap(),
        SpawnDelta::default(),
        "auto is resolved before launch and never passes flags by itself"
    );
    assert_eq!(
        apply(&launch(Speculative::Off, Some(4)), "llamacpp", true)
            .unwrap()
            .args,
        args(&["--parallel", "4", "--kv-unified"]),
        "slots share one KV pool"
    );
}

#[test]
fn ollama_takes_slots_from_the_environment_and_owns_its_drafters() {
    assert_eq!(
        apply(&launch(Speculative::Off, Some(4)), "ollama", true).unwrap(),
        SpawnDelta {
            args: vec![],
            env: env(&[("OLLAMA_NUM_PARALLEL", "4")]),
        }
    );
    assert!(matches!(
        apply(&launch(Speculative::Off, Some(4)), "ollama", false),
        Err(CacheError::Unsupported {
            backend: "ollama",
            ..
        })
    ));
    for mode in [Speculative::Draft, Speculative::Ngram] {
        assert!(
            matches!(
                apply(&launch(mode, None), "ollama", true),
                Err(CacheError::Unsupported {
                    backend: "ollama",
                    ..
                })
            ),
            "{mode:?}"
        );
    }
    assert_eq!(
        apply(&launch(Speculative::Auto, None), "ollama", false).unwrap(),
        SpawnDelta::default()
    );
}

#[test]
fn unverified_backends_refuse_speculation_and_slots() {
    for profile in [
        launch(Speculative::Draft, None),
        launch(Speculative::Off, Some(2)),
    ] {
        assert!(matches!(
            apply(&profile, "mlx", true),
            Err(CacheError::Unverified { backend: "mlx" })
        ));
        assert!(matches!(
            apply(&profile, "bitnet", true),
            Err(CacheError::Unverified { backend: "bitnet" })
        ));
    }
}

#[test]
fn slot_counts_are_bounded_and_launch_fields_serialize_only_when_set() {
    for bad in [0, 17] {
        assert!(launch(Speculative::Off, Some(bad)).validate().is_err());
    }
    launch(Speculative::Off, Some(1)).validate().unwrap();
    launch(Speculative::Off, Some(16)).validate().unwrap();
    assert_eq!(
        serde_json::to_value(CacheProfile::default()).unwrap(),
        serde_json::json!({"kvK":"f16","kvV":"f16","flashAttention":"auto","promptReuse":"off"}),
        "existing state files stay byte-compatible"
    );
    let value = serde_json::to_value(launch(Speculative::Draft, Some(4))).unwrap();
    assert_eq!(value["speculative"], "draft");
    assert_eq!(value["parallel"], 4);
    assert_eq!(
        serde_json::from_value::<CacheProfile>(value).unwrap(),
        launch(Speculative::Draft, Some(4))
    );
    assert_eq!(
        launch(Speculative::Draft, Some(4)).summary(),
        "KV f16, flash attention auto, prompt reuse off, speculative draft, 4 slots"
    );
    for mode in [
        Speculative::Off,
        Speculative::Auto,
        Speculative::Draft,
        Speculative::Ngram,
    ] {
        assert_eq!(Speculative::parse(mode.name()), Some(mode));
    }
    assert_eq!(Speculative::parse("eagle"), None);
}

#[test]
fn speculative_and_slot_flags_layer_over_a_base_profile() {
    use rigspark_runtime::cache::CacheFlags;
    let flags = CacheFlags {
        speculative: Some(Speculative::Ngram),
        parallel: Some(2),
        ..CacheFlags::default()
    };
    assert!(!flags.is_empty());
    assert_eq!(
        flags.over(Some(profile(Q8_0, Q8_0, FlashAttention::Auto))),
        Some(CacheProfile {
            speculative: Speculative::Ngram,
            parallel: Some(2),
            ..profile(Q8_0, Q8_0, FlashAttention::Auto)
        })
    );
}
