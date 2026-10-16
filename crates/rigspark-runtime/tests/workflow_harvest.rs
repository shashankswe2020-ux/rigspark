//! Harvesting official ComfyUI templates into pinned workflow candidates.
//! Recorded fixtures: Comfy-Org/workflow_templates@99e3d437 and Hugging Face metadata.
use rigspark_core::{
    generation::{FileRole, GenerationCatalog, GenerationKind},
    workflow::{DeviceClass, OutputFormat, Registry, Status, WORKFLOWS_JSON, WorkflowFile},
};
use rigspark_runtime::{
    admission::{RecordedAdmissionTransport, allowed_request},
    workflow_compile::ObjectInfo,
    workflow_harvest::{Harvest, HarvestInput, harvest},
};
use serde_json::Value;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;
use url::Url;

const COMMIT: &str = "99e3d43745926b78466d99f937c0cd2bb622423a";
const MAIN: &str = "https://api.github.com/repos/Comfy-Org/workflow_templates/commits/main";

fn raw(path: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/Comfy-Org/workflow_templates/{COMMIT}/templates/{path}"
    )
}
fn hf(repo: &str) -> String {
    format!("https://huggingface.co/api/models/{repo}?blobs=true")
}
fn recorded() -> BTreeMap<String, (u16, Vec<u8>)> {
    let hub: Value = serde_json::from_str(include_str!(
        "../../rigspark-core/fixtures/admission/hf-models.json"
    ))
    .unwrap();
    let json = |value: &Value| (200, serde_json::to_vec(value).unwrap());
    let text = |body: &str| (200, body.as_bytes().to_vec());
    BTreeMap::from([
        (MAIN.into(), text(COMMIT)),
        (
            raw("index.json"),
            text(include_str!("fixtures/harvest/index.json")),
        ),
        (
            raw("text_to_video_wan.json"),
            text(include_str!("fixtures/templates/text_to_video_wan.json")),
        ),
        (
            raw("image_qwen_image.json"),
            text(include_str!("fixtures/templates/image_qwen_image.json")),
        ),
        (
            raw("image_ming_image_01_design_t2i.json"),
            text(include_str!(
                "fixtures/templates/image_ming_image_01_design_t2i.json"
            )),
        ),
        (
            raw("video_wan2_2_14B_t2v.json"),
            text(include_str!("fixtures/templates/video_wan2_2_14B_t2v.json")),
        ),
        (
            hf("Comfy-Org/Wan_2.1_ComfyUI_repackaged"),
            text(include_str!("fixtures/harvest/hf-wan21.json")),
        ),
        (
            hf("Comfy-Org/Qwen-Image_ComfyUI"),
            json(&hub["Comfy-Org/Qwen-Image_ComfyUI"]),
        ),
        (
            hf("Comfy-Org/Wan_2.2_ComfyUI_Repackaged"),
            json(&hub["Comfy-Org/Wan_2.2_ComfyUI_Repackaged"]),
        ),
        (
            hf("Comfy-Org/Ming-Image"),
            json(&hub["Comfy-Org/Ming-Image"]),
        ),
    ])
}
fn info() -> ObjectInfo {
    ObjectInfo::parse(include_str!("fixtures/comfyui-0.38.0-object-info.json")).unwrap()
}
async fn run(
    responses: BTreeMap<String, (u16, Vec<u8>)>,
    registry: &Registry,
    commit: Option<&str>,
    limit: usize,
) -> Harvest {
    let catalog = GenerationCatalog::bundled().unwrap();
    let object_info = [(DeviceClass::Cpu, info())];
    harvest(
        &HarvestInput {
            commit,
            object_info: &object_info,
            registry,
            catalog: &catalog,
            limit,
        },
        &RecordedAdmissionTransport::new(responses).unwrap(),
        &CancellationToken::new(),
    )
    .await
    .unwrap()
}
fn bundled() -> Registry {
    Registry::parse(WORKFLOWS_JSON).unwrap()
}
fn candidate<'a>(harvest: &'a Harvest, template: &str) -> &'a WorkflowFile {
    harvest
        .candidates
        .iter()
        .find(|file| file.provenance.as_ref().unwrap().template == template)
        .unwrap_or_else(|| panic!("no candidate for {template}: {:#?}", harvest.report))
}

#[tokio::test]
async fn open_templates_become_pinned_candidates_and_every_rejection_has_a_reason() {
    let harvest = run(recorded(), &bundled(), None, 10).await;
    let report = &harvest.report;
    assert_eq!(report.commit, COMMIT);
    assert!(report.complete, "{report:#?}");
    let mut ids: Vec<&str> = harvest
        .candidates
        .iter()
        .map(|file| file.id.as_str())
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "image-qwen-image-99e3d43",
            "text-to-video-wan-99e3d43",
            "video-wan2-2-14b-t2v-99e3d43"
        ]
    );
    assert!(report.rejected["api_wan_text_to_image"].contains("Partner"));
    assert!(report.rejected["audio_ace_step_1_5_split_llm"].contains("Partner"));
    assert!(
        report.rejected["image_ming_image_01_design_t2i"].contains("comfy kitchen attention"),
        "{report:#?}"
    );
    for file in &harvest.candidates {
        assert_eq!(file.status, Status::Candidate);
        assert_eq!(file.device_classes, [DeviceClass::Cpu]);
        let round_trip = WorkflowFile::parse(&serde_json::to_string(file).unwrap()).unwrap();
        assert_eq!(&round_trip, file);
    }
}

#[tokio::test]
async fn wan_candidate_pins_exact_weights_and_links_the_matching_catalog_entry() {
    let harvest = run(recorded(), &bundled(), None, 10).await;
    let wan = candidate(&harvest, "text_to_video_wan");
    assert_eq!(wan.kind, GenerationKind::Video);
    assert_eq!(wan.bindings.output.format, OutputFormat::Mp4);
    let provenance = wan.provenance.as_ref().unwrap();
    assert_eq!(provenance.commit, COMMIT);
    assert_eq!(provenance.min_comfyui_version.as_deref(), Some("0.3.31"));
    assert_eq!(
        wan.source,
        format!(
            "https://github.com/Comfy-Org/workflow_templates/blob/{COMMIT}/templates/text_to_video_wan.json"
        )
    );
    let diffusion = wan
        .models
        .iter()
        .find(|slot| slot.role == FileRole::Diffusion)
        .and_then(|slot| slot.weight.as_ref())
        .unwrap();
    assert_eq!(diffusion.repo, "Comfy-Org/Wan_2.1_ComfyUI_repackaged");
    assert_eq!(
        diffusion.revision,
        "123acf1cc74bccbb9bfff8ac1ee72edc08c2341d"
    );
    assert_eq!(
        diffusion.file,
        "split_files/diffusion_models/wan2.1_t2v_1.3B_fp16.safetensors"
    );
    assert!(diffusion.sha256.starts_with("be531024cd90"));
    assert_eq!(diffusion.bytes, 2_838_303_560);
    let entry = &harvest.report.candidates["text-to-video-wan-99e3d43"];
    assert_eq!(entry.matches, ["wan2.1-t2v:1.3b"]);
    assert_eq!(
        entry.weights_bytes,
        2_838_303_560 + 6_735_906_897 + 253_815_318
    );
}

#[tokio::test]
async fn two_stage_wan22_candidate_links_both_fit_only_14b_entries() {
    let harvest = run(recorded(), &bundled(), None, 10).await;
    let wan22 = candidate(&harvest, "video_wan2_2_14B_t2v");
    let diffusion = wan22
        .models
        .iter()
        .filter(|slot| slot.role == FileRole::Diffusion)
        .count();
    assert_eq!(diffusion, 2);
    assert_eq!(wan22.bindings.seed.len(), 2);
    assert_eq!(
        harvest.report.candidates["video-wan2-2-14b-t2v-99e3d43"].matches,
        [
            "wan_2.2:wan2.2_t2v_high_noise_14b_fp8_scaled",
            "wan_2.2:wan2.2_t2v_low_noise_14b_fp8_scaled"
        ]
    );
}

#[tokio::test]
async fn unchanged_templates_already_in_the_registry_are_skipped() {
    let first = run(recorded(), &bundled(), None, 10).await;
    let mut document: Value = serde_json::from_str(WORKFLOWS_JSON).unwrap();
    let wan = candidate(&first, "text_to_video_wan");
    document["workflows"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(wan).unwrap());
    let registry = Registry::parse(&document.to_string()).unwrap();
    let second = run(recorded(), &registry, None, 10).await;
    assert!(second.report.skipped["text_to_video_wan"].contains("unchanged"));
    assert_eq!(second.candidates.len(), 2);
}

#[tokio::test]
async fn the_candidate_limit_caps_one_pull_request() {
    let harvest = run(recorded(), &bundled(), None, 1).await;
    assert_eq!(harvest.candidates.len(), 1);
    assert!(
        harvest
            .report
            .skipped
            .values()
            .any(|reason| reason.contains("limit"))
    );
}

#[tokio::test]
async fn unreadable_weight_metadata_marks_the_run_incomplete_without_a_candidate() {
    let mut responses = recorded();
    responses.remove(&hf("Comfy-Org/Wan_2.1_ComfyUI_repackaged"));
    let harvest = run(responses, &bundled(), None, 10).await;
    assert!(!harvest.report.complete);
    assert!(
        harvest
            .report
            .failures
            .iter()
            .any(|failure| failure.contains("Wan_2.1_ComfyUI_repackaged"))
    );
    assert!(harvest.candidates.iter().all(|file| {
        let template = &file.provenance.as_ref().unwrap().template;
        template != "text_to_video_wan" && template != "video_wan2_2_14B_t2v"
    }));
}

#[tokio::test]
async fn an_explicit_commit_skips_resolving_main() {
    let mut responses = recorded();
    responses.remove(MAIN);
    let harvest = run(responses, &bundled(), Some(COMMIT), 10).await;
    assert_eq!(harvest.candidates.len(), 3);
}

#[test]
fn only_pinned_template_urls_are_allow_listed() {
    let allowed = |url: &str| allowed_request(&Url::parse(url).unwrap());
    assert!(allowed(MAIN));
    assert!(allowed(&raw("index.json")));
    assert!(allowed(&raw("text_to_video_wan.json")));
    for refused in [
        "https://raw.githubusercontent.com/Comfy-Org/workflow_templates/main/templates/index.json",
        "https://raw.githubusercontent.com/Comfy-Org/other/99e3d43745926b78466d99f937c0cd2bb622423a/templates/index.json",
        "https://raw.githubusercontent.com/Comfy-Org/workflow_templates/99e3d43745926b78466d99f937c0cd2bb622423a/templates/../secrets.json",
        "https://raw.githubusercontent.com/Comfy-Org/workflow_templates/99e3d43745926b78466d99f937c0cd2bb622423a/templates/thumb.webp",
        "https://api.github.com/repos/Comfy-Org/workflow_templates/commits/dev",
        "https://api.github.com/repos/Comfy-Org/other/commits/main",
    ] {
        assert!(!allowed(refused), "{refused}");
    }
}
