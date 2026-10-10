use rigspark_core::{
    artificial_analysis::{PublisherConfig, PublisherMapping, parse_publisher_artifact},
    generation_admission::MAX_METADATA_BYTES,
};
use serde_json::{Value, json};

fn mapping() -> PublisherMapping {
    PublisherMapping {
        creator_slug: "publisher".into(),
        release_slug: "example".into(),
        repo: "Publisher/Example".into(),
    }
}

fn file(name: &str, digest: char) -> Value {
    json!({"rfilename":name,"size":1000,"lfs":{"sha256":digest.to_string().repeat(64),"size":1000}})
}

fn metadata() -> Value {
    json!({
        "id":"Publisher/Example","sha":"a".repeat(40),"private":false,"gated":false,
        "tags":["license:apache-2.0"],"cardData":{"license":"apache-2.0"},
        "siblings":[{"rfilename":"config.json"},file("model.safetensors", 'b')]
    })
}

#[test]
fn publisher_metadata_requires_explicit_access_flags_and_consistent_license() {
    for field in ["private", "gated"] {
        let mut value = metadata();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            parse_publisher_artifact(
                &mapping(),
                &["model.safetensors".into()],
                &value.to_string()
            )
            .is_err()
        );
    }
    let mut value = metadata();
    value["cardData"]["license"] = json!("mit");
    assert!(
        parse_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &value.to_string()
        )
        .is_err()
    );
}

#[test]
fn publisher_weight_sizes_must_be_positive_exact_and_consistent() {
    for size in [0, 9_007_199_254_740_992, u64::MAX] {
        let mut value = metadata();
        value["siblings"][1]["lfs"]["size"] = json!(size);
        assert!(
            parse_publisher_artifact(
                &mapping(),
                &["model.safetensors".into()],
                &value.to_string()
            )
            .is_err()
        );
    }
    let mut value = metadata();
    value["siblings"][1]["size"] = json!(999);
    assert!(
        parse_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &value.to_string()
        )
        .is_err()
    );
}

#[test]
fn publisher_shards_require_every_number_once_and_sort_deterministically() {
    let names: Vec<String> = (1..=2)
        .map(|n| format!("model-{n:05}-of-00002.safetensors"))
        .collect();
    let mut value = metadata();
    value["siblings"] =
        json!([{"rfilename":"config.json"},file(&names[1], 'c'),file(&names[0], 'b')]);
    let artifact = parse_publisher_artifact(&mapping(), &names, &value.to_string()).unwrap();
    assert_eq!(artifact.source.files[0].file, names[0]);
    assert_eq!(artifact.source.files.len(), 2);
    assert!(parse_publisher_artifact(&mapping(), &names[..1], &value.to_string()).is_err());
    let wrong = "model-00001-of-00003.safetensors";
    value["siblings"][1] = file(wrong, 'c');
    assert!(
        parse_publisher_artifact(
            &mapping(),
            &[names[0].clone(), wrong.into()],
            &value.to_string()
        )
        .is_err()
    );
}

#[test]
fn publisher_resolves_gguf_without_claiming_a_runnable_backend() {
    let mut value = metadata();
    value["siblings"][1] = file("model.gguf", 'b');
    let artifact =
        parse_publisher_artifact(&mapping(), &["model.gguf".into()], &value.to_string()).unwrap();
    assert_eq!(artifact.source.files[0].file, "model.gguf");
    assert_eq!(serde_json::to_value(&artifact).unwrap()["format"], "gguf");
    value["siblings"][1] = file("model.py", 'b');
    assert!(
        parse_publisher_artifact(&mapping(), &["model.py".into()], &value.to_string()).is_err()
    );
}

#[test]
fn publisher_metadata_cannot_mix_formats_or_contradict_a_digest_size() {
    let mut value = metadata();
    value["siblings"]
        .as_array_mut()
        .unwrap()
        .push(file("model.gguf", 'c'));
    assert!(
        parse_publisher_artifact(
            &mapping(),
            &["model.gguf".into(), "model.safetensors".into()],
            &value.to_string()
        )
        .is_err()
    );
    value["siblings"][2] = file("second.safetensors", 'b');
    value["siblings"][2]["size"] = json!(2000);
    value["siblings"][2]["lfs"]["size"] = json!(2000);
    assert!(
        parse_publisher_artifact(
            &mapping(),
            &["model.safetensors".into(), "second.safetensors".into()],
            &value.to_string()
        )
        .is_err()
    );
}

#[test]
fn publisher_document_limits_are_checked_at_the_exact_boundary() {
    let mut raw = metadata().to_string();
    raw.push_str(&" ".repeat(MAX_METADATA_BYTES - raw.len()));
    assert!(parse_publisher_artifact(&mapping(), &["model.safetensors".into()], &raw).is_ok());
    raw.push(' ');
    assert!(parse_publisher_artifact(&mapping(), &["model.safetensors".into()], &raw).is_err());
    let mut config = "{}".to_string();
    config.push_str(&" ".repeat(PublisherConfig::MAX_BYTES - config.len()));
    assert!(PublisherConfig::parse(config.as_bytes()).is_ok());
    config.push(' ');
    assert!(PublisherConfig::parse(config.as_bytes()).is_err());
    assert!(PublisherConfig::parse(b"invalid").is_err());
    assert!(
        parse_publisher_artifact(&mapping(), &["model.safetensors".into()], "invalid").is_err()
    );
}
