use rigspark_core::{
    artificial_analysis::{
        ArtifactObservation, PublisherMapping, artifact_groups, match_publishers, parse_index_html,
    },
    catalog::{PinnedFile, PinnedFileSource},
};

fn inventory() -> Vec<rigspark_core::artificial_analysis::IndexModel> {
    parse_index_html(include_str!(
        "../fixtures/artificial-analysis-inventory.html"
    ))
    .unwrap()
}

fn mapping() -> PublisherMapping {
    PublisherMapping {
        creator_slug: "qwen".into(),
        release_slug: "qwen-example-32b".into(),
        repo: "Publisher/Example-32B".into(),
    }
}

fn observation(id: &str) -> ArtifactObservation {
    ArtifactObservation {
        model_id: id.into(),
        source: PinnedFileSource {
            repo: mapping().repo,
            revision: "a".repeat(40),
            files: vec![PinnedFile {
                file: "model.safetensors".into(),
                sha256: "b".repeat(64),
                bytes: 1000.0,
            }],
        },
    }
}

#[test]
fn identity_matching_is_exact_and_excludes_proprietary_rows() {
    let matches = match_publishers(&inventory(), &[mapping()]).unwrap();
    assert_eq!(matches.matched.len(), 1);
    assert_eq!(
        matches.matched[0].model_ids,
        ["model-config-1", "model-config-2"]
    );
    assert_eq!(matches.matched[0].repo, "Publisher/Example-32B");
    assert_eq!(matches.excluded, ["model-config-3"]);
    assert!(matches.unmatched.is_empty());
    let mut wrong_publisher = mapping();
    wrong_publisher.creator_slug = "different-publisher".into();
    let matches = match_publishers(&inventory(), &[wrong_publisher]).unwrap();
    assert!(matches.matched.is_empty());
    assert_eq!(matches.unmatched, ["model-config-1", "model-config-2"]);
}

#[test]
fn identity_matching_rejects_ambiguous_aliases_and_duplicate_row_ids() {
    let mut conflicting = mapping();
    conflicting.repo = "AnotherPublisher/Example-32B".into();
    assert!(match_publishers(&inventory(), &[mapping(), conflicting]).is_err());
    assert_eq!(
        match_publishers(&inventory(), &[mapping(), mapping()])
            .unwrap()
            .matched
            .len(),
        1
    );
    let mut rows = inventory();
    rows.push(rows[0].clone());
    assert!(match_publishers(&rows, &[mapping()]).is_err());
}

#[test]
fn identity_matching_never_infers_repositories_from_similar_display_names() {
    let mut rows = inventory();
    rows[1].release.slug = "qwen-example-32b-new".into();
    let matches = match_publishers(&rows, &[mapping()]).unwrap();
    assert_eq!(matches.unmatched, ["model-config-2"]);
    assert_eq!(matches.matched[0].model_ids, ["model-config-1"]);
}

#[test]
fn identity_collapses_configurations_only_after_weights_are_resolved() {
    let rows = inventory();
    let observations = [observation("model-config-1"), observation("model-config-2")];
    let groups = artifact_groups(&rows, &[mapping()], &observations).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].model_ids, ["model-config-1", "model-config-2"]);
    assert_eq!(groups[0].identity.files[0].sha256, "b".repeat(64));
    assert_eq!(groups[0].identity.files[0].bytes, 1000);

    let mut distinct = observations.clone();
    distinct[1].source.files[0].sha256 = "c".repeat(64);
    assert_eq!(
        artifact_groups(&rows, &[mapping()], &distinct)
            .unwrap()
            .len(),
        2
    );
    distinct[1] = observations[1].clone();
    distinct[1].source.revision = "c".repeat(40);
    assert_eq!(
        artifact_groups(&rows, &[mapping()], &distinct)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn identity_is_deterministic_across_order_hex_case_and_file_aliases() {
    let mut first = observation("model-config-1");
    first.source.files.push(PinnedFile {
        file: "model-00002.safetensors".into(),
        sha256: "c".repeat(64),
        bytes: 2000.0,
    });
    let mut second = first.clone();
    second.model_id = "model-config-2".into();
    second.source.revision = second.source.revision.to_uppercase();
    second.source.files.reverse();
    for file in &mut second.source.files {
        file.sha256 = file.sha256.to_uppercase();
        file.file = format!("aliases/{}", file.file);
    }
    let expected =
        artifact_groups(&inventory(), &[mapping()], &[first.clone(), second.clone()]).unwrap();
    let mut reversed = inventory();
    reversed.reverse();
    let actual = artifact_groups(&reversed, &[mapping()], &[second, first]).unwrap();
    assert_eq!(expected, actual);
    assert_eq!(actual.len(), 1);
    assert_eq!(actual[0].identity.files.len(), 2);
}

#[test]
fn identity_rejects_unmatched_proprietary_or_wrong_publisher_artifacts() {
    for id in ["model-config-3", "unknown"] {
        assert!(artifact_groups(&inventory(), &[mapping()], &[observation(id)]).is_err());
    }
    let mut wrong = observation("model-config-1");
    wrong.source.repo = "Impersonator/Example-32B".into();
    assert!(artifact_groups(&inventory(), &[mapping()], &[wrong]).is_err());
    assert!(artifact_groups(&inventory(), &[], &[observation("model-config-1")]).is_err());
}

#[test]
fn identity_rejects_conflicting_sizes_for_the_same_digest() {
    let first = observation("model-config-1");
    let mut second = observation("model-config-2");
    second.source.files[0].bytes = 1001.0;
    assert!(artifact_groups(&inventory(), &[mapping()], &[first, second]).is_err());
}

#[test]
fn identity_validates_manifest_and_mapping_boundaries() {
    let mut invalid = observation("model-config-1");
    invalid.source.revision = "main".into();
    assert!(artifact_groups(&inventory(), &[mapping()], &[invalid]).is_err());
    let mut invalid = observation("model-config-1");
    invalid.source.files[0].file = "../model.safetensors".into();
    assert!(artifact_groups(&inventory(), &[mapping()], &[invalid]).is_err());
    let mut invalid = mapping();
    invalid.repo = "https://untrusted.test/model".into();
    assert!(match_publishers(&inventory(), &[invalid]).is_err());
    assert!(match_publishers(&[], &[]).is_err());
}

#[test]
fn identity_record_limits_are_enforced_at_the_exact_boundary() {
    use rigspark_core::artificial_analysis::MAX_IDENTITY_RECORDS;
    let row = inventory()[0].clone();
    let mut rows: Vec<_> = (0..MAX_IDENTITY_RECORDS)
        .map(|index| {
            let mut row = row.clone();
            row.id = format!("model-{index}");
            row
        })
        .collect();
    assert_eq!(
        match_publishers(&rows, &[mapping()]).unwrap().matched[0]
            .model_ids
            .len(),
        MAX_IDENTITY_RECORDS
    );
    let mut extra = row;
    extra.id = "extra".into();
    rows.push(extra);
    assert!(match_publishers(&rows, &[mapping()]).is_err());

    let mut mappings = vec![mapping(); MAX_IDENTITY_RECORDS];
    assert!(match_publishers(&inventory(), &mappings).is_ok());
    mappings.push(mapping());
    assert!(match_publishers(&inventory(), &mappings).is_err());

    let mut observations = vec![observation("model-config-1"); MAX_IDENTITY_RECORDS];
    assert_eq!(
        artifact_groups(&inventory(), &[mapping()], &observations)
            .unwrap()
            .len(),
        1
    );
    observations.push(observation("model-config-1"));
    assert!(artifact_groups(&inventory(), &[mapping()], &observations).is_err());
}

#[test]
fn identity_reports_unresolved_rows_without_creating_weight_identities() {
    let matches = match_publishers(&inventory(), &[]).unwrap();
    assert_eq!(matches.unmatched, ["model-config-1", "model-config-2"]);
    assert!(
        artifact_groups(&inventory(), &[mapping()], &[])
            .unwrap()
            .is_empty()
    );
    let groups =
        artifact_groups(&inventory(), &[mapping()], &[observation("model-config-1")]).unwrap();
    assert_eq!(groups[0].model_ids, ["model-config-1"]);
}

#[test]
fn identity_rejects_control_characters_without_echoing_untrusted_identifiers() {
    let error =
        artifact_groups(&inventory(), &[mapping()], &[observation("invalid\nid")]).unwrap_err();
    assert_eq!(error.0, "invalid artifact model id");
}

#[test]
fn identity_keeps_different_publishers_distinct_even_when_weight_digests_match() {
    let mut rows = inventory();
    rows[1].creator.slug = "another-publisher".into();
    rows[1].creator.id = "another-publisher-id".into();
    let mut other = mapping();
    other.creator_slug = rows[1].creator.slug.clone();
    other.repo = "AnotherPublisher/Example-32B".into();
    let mut second = observation("model-config-2");
    second.source.repo = other.repo.clone();
    let groups = artifact_groups(
        &rows,
        &[mapping(), other],
        &[observation("model-config-1"), second],
    )
    .unwrap();
    assert_eq!(groups.len(), 2);
}
