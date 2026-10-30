use rigspark_core::catalog::{Catalog, CatalogModel, EntryProvenance};
use rigspark_runtime::{catalog_update::CatalogStore, local_catalog::LocalCatalog};

fn local(id: &str) -> CatalogModel {
    let mut model = Catalog::parse(rigspark_core::MODELS_JSON)
        .unwrap()
        .models
        .into_iter()
        .find(|model| model.id == "qwen2.5:0.5b")
        .unwrap();
    model.id = id.into();
    model.provenance = EntryProvenance::Local;
    model.release_date = None;
    model.benchmark_proxy = None;
    model.added_at = Some("2026-10-30".into());
    model
}

#[test]
fn added_models_persist_replace_and_remove() {
    let home = tempfile::tempdir().unwrap();
    let store = LocalCatalog::new(home.path());
    assert!(store.models().unwrap().is_empty(), "no overlay yet");
    store.add(local("hf.co/a/b-gguf:q4_k_m")).unwrap();
    store.add(local("hf.co/a/b-gguf:q8_0")).unwrap();
    let mut replaced = local("hf.co/a/b-gguf:q4_k_m");
    replaced.context_length = 4096.0;
    store.add(replaced).unwrap();
    let models = store.models().unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].context_length, 4096.0, "re-adding replaces");
    assert!(store.remove("hf.co/a/b-gguf:q8_0").unwrap());
    assert!(!store.remove("hf.co/a/b-gguf:q8_0").unwrap());
    assert_eq!(store.models().unwrap().len(), 1);
}

#[test]
fn only_pinned_local_entries_that_do_not_shadow_the_catalog_are_accepted() {
    let home = tempfile::tempdir().unwrap();
    let store = LocalCatalog::new(home.path());
    assert!(store.add(local("qwen2.5:0.5b")).is_err(), "catalog ids win");
    let mut curated = local("hf.co/a/b-gguf:q4_k_m");
    curated.provenance = EntryProvenance::Curated;
    assert!(store.add(curated).is_err());
    let mut unpinned = local("hf.co/a/b-gguf:q4_k_m");
    unpinned.quantizations[0].sha256 = None;
    assert!(store.add(unpinned).is_err());
    assert!(store.models().unwrap().is_empty());
}

#[test]
fn a_corrupt_overlay_is_refused_not_overwritten() {
    let home = tempfile::tempdir().unwrap();
    let store = LocalCatalog::new(home.path());
    store.add(local("hf.co/a/b-gguf:q4_k_m")).unwrap();
    std::fs::write(home.path().join("catalog/local.json"), b"{broken").unwrap();
    assert!(store.models().is_err());
    assert!(store.add(local("hf.co/a/c-gguf:q4_k_m")).is_err());
    assert_eq!(
        std::fs::read(home.path().join("catalog/local.json")).unwrap(),
        b"{broken"
    );
}

#[test]
fn the_loaded_catalog_includes_added_models_and_reports_a_broken_overlay() {
    let home = tempfile::tempdir().unwrap();
    let store = CatalogStore::new(home.path(), None);
    let base = store.load().unwrap();
    LocalCatalog::new(home.path())
        .add(local("hf.co/a/b-gguf:q4_k_m"))
        .unwrap();
    let merged = store.load().unwrap();
    assert_eq!(merged.catalog.models.len(), base.catalog.models.len() + 1);
    assert_eq!(merged.status.local_count, 1);
    let reparsed = Catalog::parse(&merged.raw).unwrap();
    assert!(
        reparsed
            .models
            .iter()
            .any(|model| model.id == "hf.co/a/b-gguf:q4_k_m"),
        "advice re-parses raw and must see added models"
    );
    std::fs::write(home.path().join("catalog/local.json"), b"{broken").unwrap();
    let fallback = store.load().unwrap();
    assert_eq!(fallback.catalog.models.len(), base.catalog.models.len());
    assert!(
        fallback
            .status
            .warnings
            .iter()
            .any(|warning| warning.contains("added")),
        "{:?}",
        fallback.status.warnings
    );
}
