use rigspark_core::{
    advice::{hardware_score, throughput, verdict},
    catalog::{Catalog, PerfDataset},
    sizing::Hardware,
};
use serde_json::json;

#[test]
fn scores_hardware_and_preserves_unknown_backend_evidence() {
    let hardware: Hardware = serde_json::from_value(json!({"arch":"x64","platform":"linux","totalRamBytes":68719476736_u64,"freeRamBytes":60000000000_u64,"freeDiskBytes":214748364800_u64,"gpu":[{"vendor":"nvidia","vramBytes":25769803776_u64}]})).unwrap();
    assert_eq!(hardware_score(&hardware)["total"], 100);
    let catalog = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let perf = PerfDataset::parse(include_str!("../../rigspark-core/data/perf.json")).unwrap();
    let model = catalog
        .models
        .iter()
        .find(|model| model.id == "llama3.1:8b")
        .unwrap();
    assert!(
        throughput(model, &model.quantizations[0], &hardware, &perf, "ollama")
            .unwrap()
            .known
    );
    assert!(
        !throughput(model, &model.quantizations[0], &hardware, &perf, "mlx")
            .unwrap()
            .known
    );
    assert_eq!(
        verdict(
            model,
            &hardware,
            &perf,
            Some(model.context_length.unwrap() + 1.0),
            "ollama"
        )
        .unwrap()["reason"],
        "context-bound"
    );
}
