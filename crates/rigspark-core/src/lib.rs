pub mod admission;
pub mod advice;
pub mod bootstrap;
pub mod catalog;
pub mod catalog_notice;
pub mod coverage;
pub mod enrich;
pub mod freshness;
pub mod generation;
pub mod generation_admission;
pub mod plan;
pub mod ranking;
pub mod registry_collector;
pub mod reports;
pub mod site_latest;
pub mod sizing;

/// Curated, cited model catalog bundled into every release.
pub const MODELS_JSON: &str = include_str!("../data/models.json");
/// Curated throughput dataset bundled into every release.
pub const PERF_JSON: &str = include_str!("../data/perf.json");
/// Curated, pinned image/video generation catalog (ComfyUI workflows).
pub const GENERATION_JSON: &str = include_str!("../data/generation.json");
/// Pinned registry snapshot that catalog bootstrap and refresh read.
pub const REGISTRY_SNAPSHOT_JSON: &str = include_str!("../fixtures/registry-snapshot.json");
