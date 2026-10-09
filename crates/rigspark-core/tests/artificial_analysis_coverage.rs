use rigspark_core::artificial_analysis::{MAX_INDEX_HTML_BYTES, parse_index_html};

const INVENTORY: &str = include_str!("../fixtures/artificial-analysis-inventory.html");

#[test]
fn parses_public_index_inventory_and_preserves_benchmark_configurations() {
    let inventory = parse_index_html(INVENTORY).unwrap();

    assert_eq!(inventory.len(), 3);
    assert_eq!(
        inventory
            .iter()
            .filter(|model| model.is_open_weights)
            .count(),
        2
    );
    assert_eq!(inventory[0].release.slug, "qwen-example-32b");
    assert_eq!(inventory[1].release.slug, "qwen-example-32b");
    assert_ne!(inventory[0].id, inventory[1].id);
    assert_eq!(inventory[0].creator.slug, "qwen");
    assert!(!inventory[2].is_open_weights);
}

#[test]
fn rejects_empty_or_missing_inventory_instead_of_reporting_complete_coverage() {
    let empty = INVENTORY.replace(
        "\\\"initialModels\\\":[{\\\"id\\\":\\\"model-config-1\\\"",
        "\\\"initialModels\\\":[],\\\"ignored\\\":[{\\\"id\\\":\\\"model-config-1\\\"",
    );
    assert_eq!(
        parse_index_html(&empty).unwrap_err().0,
        "Artificial Analysis inventory is empty"
    );
    assert_eq!(
        parse_index_html("<!doctype html><title>changed</title>")
            .unwrap_err()
            .0,
        "Artificial Analysis inventory missing"
    );
}

#[test]
fn rejects_oversized_or_structurally_invalid_inventory() {
    assert_eq!(
        parse_index_html(&"x".repeat(MAX_INDEX_HTML_BYTES + 1))
            .unwrap_err()
            .0,
        "Artificial Analysis index exceeds 4 MiB"
    );
    let invalid = INVENTORY.replacen("\\\"isOpenWeights\\\":true,", "", 1);
    assert!(parse_index_html(&invalid).is_err());
}
