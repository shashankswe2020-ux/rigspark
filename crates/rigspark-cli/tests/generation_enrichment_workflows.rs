#[test]
fn image_and_video_enrichment_are_independent_review_gated_pipelines() {
    for (kind, workflow) in [
        (
            "image",
            include_str!("../../../.github/workflows/generation-image-enrichment.yml"),
        ),
        (
            "video",
            include_str!("../../../.github/workflows/generation-video-enrichment.yml"),
        ),
    ] {
        assert!(workflow.contains("workflow_dispatch:"));
        assert!(workflow.contains(&format!("ruby scripts/generation-hf-enrich.rb {kind} ")));
        assert!(workflow.contains(&format!("cargo generation-enrich {kind}")));
        assert!(workflow.contains("cargo native-retirement"));
        assert!(workflow.contains("gh pr create --base main"));
        assert!(workflow.contains("Unknown candidates are report-only"));
        assert!(!workflow.contains("actions/checkout"));
        assert!(!workflow.contains("push origin main"));
    }
}
