use rigspark_core::{
    catalog::{Catalog, PerfDataset},
    ranking::AdviceOptions,
    sizing::Hardware,
};
use rigspark_gui::models::recommended;
use serde_json::{Value, json};

#[test]
fn bitnet_gui_advice_retains_runtime_isolation_and_unknown_evidence() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let perf = PerfDataset::parse(rigspark_core::PERF_JSON).unwrap();
    let hardware: Hardware = serde_json::from_value(json!({
        "arch": "x64", "platform": "linux", "totalRamBytes": 17179869184_u64,
        "freeRamBytes": 12000000000_u64, "freeDiskBytes": 500000000000_u64, "gpu": []
    }))
    .unwrap();
    let models = recommended(
        &catalog,
        &hardware,
        &perf,
        &AdviceOptions {
            backend: Some("bitnet".into()),
            available_backends: Some(vec!["bitnet".into()]),
            context: Some(4096.0),
            ..Default::default()
        },
        100,
    )
    .unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "bitnet-b1.58:2b");
    assert_eq!(models[0]["backends"], json!(["bitnet"]));
    assert_eq!(models[0]["throughput"]["known"], false);
    assert_eq!(models[0]["contextFitKnown"], false);
    assert_eq!(models[0]["diskBytes"], 1187801280.0);
}

fn compare(actual: &Value, expected: &Value, path: &str) -> Result<(), String> {
    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) => {
            let actual = actual.as_f64().unwrap();
            let expected = expected.as_f64().unwrap();
            if (actual - expected).abs() <= 1e-12 * expected.abs().max(1.0) {
                return Ok(());
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            if actual.len() != expected.len() {
                return Err(format!(
                    "{path}: array length {} != {}",
                    actual.len(),
                    expected.len()
                ));
            }
            for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                compare(actual, expected, &format!("{path}[{index}]"))?;
            }
            return Ok(());
        }
        (Value::Object(actual), Value::Object(expected)) => {
            let mut actual_keys: Vec<_> = actual.keys().collect();
            let mut expected_keys: Vec<_> = expected.keys().collect();
            actual_keys.sort();
            expected_keys.sort();
            if actual_keys != expected_keys {
                return Err(format!(
                    "{path}: object keys {actual_keys:?} != {expected_keys:?}"
                ));
            }
            for (key, expected) in expected {
                compare(&actual[key], expected, &format!("{path}.{key}"))?;
            }
            return Ok(());
        }
        _ if actual == expected => return Ok(()),
        _ => {}
    }
    Err(format!("{path}: actual {actual} != expected {expected}"))
}

#[test]
fn all_24_gui_recommendation_contracts_match_frozen_typescript() {
    let fixture: Value = serde_json::from_str(include_str!("recommendation-oracle.json")).unwrap();
    let catalog = Catalog::parse(&fixture["catalog"].to_string()).unwrap();
    let perf = PerfDataset::parse(&fixture["perf"].to_string()).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 24);

    let gib = 1024_u64.pow(3);
    let profiles = [
        (
            "apple-32g",
            json!({"arch":"arm64","platform":"darwin","totalRamBytes":32*gib,"freeRamBytes":24*gib,"freeDiskBytes":500*gib,"gpu":[{"vendor":"apple","vramBytes":0}]}),
        ),
        (
            "nvidia-8g",
            json!({"arch":"x64","platform":"linux","totalRamBytes":32*gib,"freeRamBytes":24*gib,"freeDiskBytes":500*gib,"gpu":[{"vendor":"nvidia","vramBytes":8*gib}]}),
        ),
        (
            "cpu-windows-16g",
            json!({"arch":"x64","platform":"win32","totalRamBytes":16*gib,"freeRamBytes":8*gib,"freeDiskBytes":100*gib,"gpu":[]}),
        ),
    ];
    let modes = [
        ("default", json!({}), json!({})),
        (
            "context-low",
            json!({"contextPreset":"low"}),
            json!({"contextPercent":25}),
        ),
        (
            "context-mid",
            json!({"contextPreset":"mid"}),
            json!({"contextPercent":50}),
        ),
        (
            "context-high",
            json!({"contextPreset":"high"}),
            json!({"contextPercent":75}),
        ),
        (
            "context-max",
            json!({"contextPreset":"max"}),
            json!({"contextPercent":100}),
        ),
        (
            "context-65536",
            json!({"context":65536}),
            json!({"context":65536}),
        ),
        (
            "runtime-mlx",
            json!({"runtime":"mlx"}),
            json!({"backend":"mlx"}),
        ),
        (
            "runtime-llamacpp",
            json!({"runtime":"llamacpp"}),
            json!({"backend":"llamacpp"}),
        ),
    ];

    for (index, case) in cases.iter().enumerate() {
        let (profile_name, hardware) = &profiles[index / modes.len()];
        let (mode_name, mode, options) = &modes[index % modes.len()];
        let name = format!("{profile_name}/{mode_name}");
        assert_eq!(case["name"], name);
        assert_eq!(&case["mode"], mode, "{name}: TS mode");
        assert_eq!(
            case["request"],
            json!({"hardware":hardware,"options":options}),
            "{name}: native request"
        );
        let hardware: Hardware =
            serde_json::from_value(case["request"]["hardware"].clone()).unwrap();
        let options: AdviceOptions =
            serde_json::from_value(case["request"]["options"].clone()).unwrap();
        let actual = recommended(&catalog, &hardware, &perf, &options, 8)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(case["expected"].as_array().unwrap().len(), 8, "{name}");
        compare(&json!(actual), &case["expected"], &name).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn comparator_preserves_original_numeric_tolerance() {
    for expected in [0.0_f64, 0.5, 2.0, -2.0, 1e12] {
        let tolerance = 1e-12 * expected.abs().max(1.0);
        for direction in [-1.0, 1.0] {
            assert!(
                compare(
                    &json!(expected + direction * tolerance * 0.5),
                    &json!(expected),
                    "number"
                )
                .is_ok()
            );
            assert!(
                compare(
                    &json!(expected + direction * tolerance * 2.0),
                    &json!(expected),
                    "number"
                )
                .is_err()
            );
        }
    }
    assert!(compare(&json!(1e-12), &json!(0), "boundary").is_ok());
    assert!(compare(&json!(1), &json!(1.0), "integer").is_ok());
}

#[test]
fn comparator_rejects_structural_type_and_order_drift() {
    for (actual, expected) in [
        (json!({"extra":null}), json!({})),
        (json!({}), json!({"missing":null})),
        (json!({"wrong":1}), json!({"key":1})),
        (json!([1]), json!([1, 2])),
        (json!([2, 1]), json!([1, 2])),
        (json!([]), json!({})),
        (json!("1"), json!(1)),
        (json!(false), json!(true)),
        (json!("slow"), json!("yes")),
        (json!(0), Value::Null),
        (
            json!({"models":[{"score":0}]}),
            json!({"models":[{"score":1}]}),
        ),
    ] {
        assert!(compare(&actual, &expected, "contract").is_err());
    }
    assert!(compare(&json!({"nullable":null}), &json!({"nullable":null}), "null").is_ok());
    assert!(
        compare(
            &json!({"first":1,"second":2}),
            &json!({"second":2,"first":1}),
            "keys"
        )
        .is_ok()
    );
    let error = compare(&json!([{"score":0}]), &json!([{"score":1}]), "case").unwrap_err();
    assert!(error.starts_with("case[0].score:"));
}

#[test]
fn bundled_cpu_only_llama_cpp_is_judged_on_system_memory_alone() {
    let hardware: Hardware = serde_json::from_value(json!({
        "arch": "x64", "platform": "win32", "totalRamBytes": 17179869184_u64,
        "freeRamBytes": 12000000000_u64, "freeDiskBytes": 500000000000_u64,
        "gpu": [{"vendor": "nvidia", "vramBytes": 8589934592_u64}]
    }))
    .unwrap();
    let cpu = rigspark_gui::models::advice_hardware(hardware.clone(), Some("llamacpp"), true);
    assert!(cpu.gpu.is_empty());
    assert_eq!(cpu.unified_memory, Some(false));
    assert_eq!(cpu.total_ram_bytes, hardware.total_ram_bytes);
    for (runtime, bundled_cpu_only) in [
        (Some("llamacpp"), false),
        (Some("ollama"), true),
        (None, true),
    ] {
        assert_eq!(
            rigspark_gui::models::advice_hardware(hardware.clone(), runtime, bundled_cpu_only)
                .gpu
                .len(),
            1,
            "{runtime:?} {bundled_cpu_only}"
        );
    }
}
