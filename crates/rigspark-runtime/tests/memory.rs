#[cfg(unix)]
#[test]
fn retained_memory_handle_refuses_replaced_home() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("home");
    let store = MemoryStore::open(&home, "model", "original").unwrap();
    std::fs::rename(&home, parent.path().join("original")).unwrap();
    let replacement = MemoryStore::open(&home, "model", "replacement").unwrap();
    assert!(store.load().is_err());
    assert_eq!(replacement.read_meta().unwrap().created_at, "replacement");
    assert_eq!(
        MemoryStore::existing(&parent.path().join("original"), "model")
            .unwrap()
            .meta
            .created_at,
        "original"
    );
}
#[tokio::test]
async fn prepared_migration_commits_exact_summary_once_and_rejects_later_drift() {
    use rigspark_runtime::memory::{
        CaptureOptions, MemoryError, MigrationOptions, Summarizer, Turn,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Summary(AtomicUsize);
    #[async_trait::async_trait]
    impl Summarizer for Summary {
        async fn summarize(
            &self,
            _: &[Turn],
            _: &tokio_util::sync::CancellationToken,
        ) -> Result<String, MemoryError> {
            Ok(format!("summary {}", self.0.fetch_add(1, Ordering::SeqCst)))
        }
    }
    let home = tempfile::tempdir().unwrap();
    let source = MemoryStore::open(home.path(), "source", "now").unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    source
        .capture(
            &"history ".repeat(500),
            "reply",
            CaptureOptions {
                timestamp: "now",
                embedder: None,
                embedding_unsupported: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    let summarizer = Summary(AtomicUsize::new(0));
    let options = || MigrationOptions {
        context: 300,
        embedder: None,
        target_dimension: None,
        summarizer: Some(&summarizer),
        embedding_unsupported: false,
    };
    let prepared = source
        .prepare_migration("target", options(), &cancel)
        .await
        .unwrap();
    assert_eq!(prepared.summary().strategy, "summarize");
    let target = source
        .commit_migration(prepared, "now", false, &cancel)
        .unwrap();
    assert_eq!(summarizer.0.load(Ordering::SeqCst), 1);
    assert!(
        target.load().unwrap().turns[0]
            .content
            .ends_with("summary 0")
    );
    let prepared = source
        .prepare_migration("target", options(), &cancel)
        .await
        .unwrap();
    target
        .capture(
            "concurrent",
            "reply",
            CaptureOptions {
                timestamp: "now",
                embedder: None,
                embedding_unsupported: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(
        source
            .commit_migration(prepared, "now", false, &cancel)
            .is_err()
    );
    assert!(
        target
            .load()
            .unwrap()
            .turns
            .iter()
            .any(|turn| turn.content == "concurrent")
    );
}
#[test]
fn memory_scan_bounds_empty_directory_depth() {
    let root = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(root.path(), "model", "now").unwrap();
    let mut nested = store.dir.clone();
    for _ in 0..42 {
        nested = nested.join("nested");
    }
    std::fs::create_dir_all(nested).unwrap();
    assert!(store.load().is_err());
}
#[test]
fn foreign_home_lock_cannot_authorize_metadata_write() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let first_store = MemoryStore::open(first.path(), "model", "now").unwrap();
    let second_store = MemoryStore::open(second.path(), "model", "now").unwrap();
    let guard = first_store.lock().unwrap();
    let mut meta = second_store.meta.clone();
    meta.embedding_unsupported = Some(true);
    assert!(second_store.write_meta(&guard, &meta).is_err());
    assert_eq!(
        second_store.read_meta().unwrap().embedding_unsupported,
        None
    );
}

struct EmbeddingFixture {
    model: &'static str,
    dimension: usize,
}
#[async_trait::async_trait]
impl rigspark_runtime::memory::Embedder for EmbeddingFixture {
    fn model(&self) -> &str {
        self.model
    }
    async fn embed(
        &self,
        texts: &[String],
        _: &tokio_util::sync::CancellationToken,
    ) -> Result<rigspark_runtime::memory::EmbeddingOutput, rigspark_runtime::memory::MemoryError>
    {
        Ok(rigspark_runtime::memory::EmbeddingOutput {
            dimension: self.dimension,
            vectors: texts.iter().map(|_| vec![0.5; self.dimension]).collect(),
        })
    }
}
#[tokio::test]
async fn embedding_capture_pins_space_and_migration_reembeds_exact_chunks() {
    use rigspark_runtime::memory::{CaptureOptions, MigrationOptions};
    let root = tempfile::tempdir().unwrap();
    let source = MemoryStore::open(root.path(), "source", "now").unwrap();
    let first = EmbeddingFixture {
        model: "embed-first",
        dimension: 2,
    };
    let second = EmbeddingFixture {
        model: "embed-second",
        dimension: 3,
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let options = |embedder| CaptureOptions {
        timestamp: "now",
        embedder: Some(embedder),
        embedding_unsupported: false,
    };
    source
        .capture("question", "answer", options(&first), &cancel)
        .await
        .unwrap();
    let before = source.load().unwrap();
    assert!(
        source
            .capture("other", "reply", options(&second), &cancel)
            .await
            .is_err()
    );
    assert_eq!(source.load().unwrap(), before);
    let target = source
        .migrate(
            "target",
            "now",
            MigrationOptions {
                context: 8192,
                embedder: Some(&second),
                target_dimension: Some(3),
                summarizer: None,
                embedding_unsupported: false,
            },
            false,
            &cancel,
        )
        .await
        .unwrap();
    let migrated = target.load().unwrap().embedding.unwrap();
    assert_eq!(migrated.meta.model, "embed-second");
    assert_eq!(migrated.meta.dimension, 3);
    assert_eq!(migrated.chunks, before.embedding.unwrap().chunks);
    assert_eq!(migrated.vectors.len(), 2);
}
use rigspark_runtime::memory::{MemoryStore, memory_slug};
#[test]
fn slugs_preserve_existing_layout_and_bound_long_names() {
    assert_eq!(
        memory_slug(" Owner/Model:Q4_K_M ", false).unwrap(),
        "owner-model-q4_k_m"
    );
    assert_eq!(memory_slug("../evil", false).unwrap(), "evil");
    assert_eq!(memory_slug("CON.json", true).unwrap(), "x-con.json");
    assert!(memory_slug("...///", false).is_err());
    let long = memory_slug(&"model".repeat(100), false).unwrap();
    assert_eq!(long.len(), 128);
}
#[test]
fn metadata_roundtrips_and_slug_collisions_never_replace_ownership() {
    let root = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(root.path(), "owner/model", "2026-09-17T00:00:00Z").unwrap();
    assert_eq!(store.meta.model_id, "owner/model");
    assert_eq!(
        MemoryStore::open(root.path(), "owner/model", "ignored")
            .unwrap()
            .meta,
        store.meta
    );
    assert!(MemoryStore::open(root.path(), "owner:model", "2026-09-17T00:00:00Z").is_err());
    assert_eq!(store.read_meta().unwrap().model_id, "owner/model");
}

#[test]
fn opening_recovers_crash_between_publication_renames() {
    let root = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(root.path(), "model", "original").unwrap();
    let transaction = root.path().join(".staging/memory-model");
    std::fs::create_dir_all(transaction.join("next")).unwrap();
    std::fs::rename(&store.dir, transaction.join("previous")).unwrap();
    std::fs::write(transaction.join("phase"), "prepared\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            transaction.join("phase"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let reopened = MemoryStore::open(root.path(), "model", "replacement").unwrap();
    assert_eq!(reopened.meta.created_at, "original");
    assert!(!transaction.exists());
}

#[tokio::test]
async fn capture_keeps_turns_and_facts_when_the_embedder_is_unavailable() {
    use rigspark_runtime::memory::{CaptureOptions, Embedder, EmbeddingOutput};
    struct Broken;
    #[async_trait::async_trait]
    impl Embedder for Broken {
        fn model(&self) -> &str {
            "embedding"
        }
        async fn embed(
            &self,
            _texts: &[String],
            _cancel: &tokio_util::sync::CancellationToken,
        ) -> Result<EmbeddingOutput, rigspark_runtime::memory::MemoryError> {
            Err(rigspark_runtime::memory::MemoryError("offline".into()))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(root.path(), "test:latest", "2026-09-17T00:00:00Z").unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let degraded = store
        .capture(
            "My name is Ada.",
            "Hello.",
            CaptureOptions {
                timestamp: "2026-09-17T00:00:00Z",
                embedder: Some(&Broken),
                embedding_unsupported: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        degraded.turns_appended, 2,
        "a missing embedding model never loses the reply"
    );
    assert_eq!(degraded.vectors_embedded, 0, "no vector is fabricated");
    assert!(
        degraded
            .embedding_skipped
            .as_deref()
            .is_some_and(|reason| reason.contains("offline"))
    );
    let meta = store.read_meta().unwrap();
    assert_eq!(
        (meta.embedding.clone(), meta.embedding_unsupported),
        (None, None),
        "the embedding space stays unset so a later embedder can still define it"
    );
    assert!(!store.dir.join("embeddings/vectors.jsonl").exists());
    let result = store
        .capture(
            "My name is Ada. I prefer Rust.",
            "Hello.",
            CaptureOptions {
                timestamp: "2026-09-17T00:00:00Z",
                embedder: None,
                embedding_unsupported: true,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(result.turns_appended, 2);
    assert_eq!(result.facts_extracted, 1, "name was already captured");
    assert_eq!(result.embedding_skipped, None);
    let memory = store.load().unwrap();
    assert_eq!(memory.turns.len(), 4);
    assert!(memory.facts_text.contains("name = Ada"));
    assert_eq!(store.read_meta().unwrap().embedding_unsupported, Some(true));
}

#[tokio::test]
async fn migration_preserves_facts_persona_and_remaps_old_turns() {
    use rigspark_runtime::memory::{MigrationOptions, SourceMemory, Turn, plan_migration};
    let source = SourceMemory {
        turns: vec![
            Turn {
                role: "user".into(),
                content: "old ".repeat(400),
                ts: "old".into(),
            },
            Turn {
                role: "assistant".into(),
                content: "recent".into(),
                ts: "new".into(),
            },
        ],
        system_prompt: Some("persona".into()),
        facts_text: "{ \"schemaVersion\": 1, \"facts\": [] }\n".into(),
        facts_present: true,
        embedding: None,
    };
    let plan = plan_migration(
        &source,
        MigrationOptions {
            context: 300,
            embedder: None,
            target_dimension: None,
            summarizer: None,
            embedding_unsupported: false,
        },
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(plan.source.facts_text, source.facts_text);
    assert_eq!(plan.source.system_prompt, source.system_prompt);
    assert_eq!(plan.summary.strategy, "truncate");
    assert_eq!(plan.source.turns.last().unwrap().content, "recent");
}

#[tokio::test]
async fn migration_copy_preserves_source_and_move_removes_only_after_verification() {
    use rigspark_runtime::memory::{CaptureOptions, MigrationOptions};
    let root = tempfile::tempdir().unwrap();
    let source = MemoryStore::open(root.path(), "source", "2026-09-17T00:00:00Z").unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    source
        .capture(
            "remember that durable fact",
            "reply",
            CaptureOptions {
                timestamp: "now",
                embedder: None,
                embedding_unsupported: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    let options = || MigrationOptions {
        context: 8192,
        embedder: None,
        target_dimension: None,
        summarizer: None,
        embedding_unsupported: false,
    };
    let target = source
        .migrate("target", "2026-09-17T00:00:00Z", options(), false, &cancel)
        .await
        .unwrap();
    assert_eq!(source.load().unwrap(), target.load().unwrap());
    let moved = source
        .migrate("moved", "2026-09-17T00:00:00Z", options(), true, &cancel)
        .await
        .unwrap();
    assert!(!source.dir.exists());
    assert_eq!(target.load().unwrap(), moved.load().unwrap());
}

fn private_fixture(path: &std::path::Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn logical_memory_files_reject_invalid_utf8_and_oversized_persona() {
    for (name, bytes) in [
        ("facts.json", vec![0xff]),
        ("system.md", vec![b'x'; 64 * 1024 + 1]),
    ] {
        let home = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(home.path(), "source", "now").unwrap();
        private_fixture(&store.dir.join(name), &bytes);
        assert!(store.load().is_err(), "{name}");
        assert_eq!(std::fs::read(store.dir.join(name)).unwrap(), bytes);
    }
}

#[cfg(unix)]
#[test]
fn logical_memory_files_and_embedding_directories_reject_symlink_escape() {
    use std::os::unix::fs::symlink;
    for name in ["facts.json", "conversation.jsonl", "embeddings"] {
        let home = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(home.path(), "source", "now").unwrap();
        let external = home.path().join("external");
        if name == "embeddings" {
            std::fs::create_dir(&external).unwrap();
            private_fixture(&external.join("vectors.jsonl"), b"outside");
        } else {
            private_fixture(&external, b"outside");
        }
        let path = store.dir.join(name);
        if path.is_file() {
            std::fs::remove_file(&path).unwrap();
        }
        symlink(&external, &path).unwrap();
        assert!(store.load().is_err(), "{name}");
        assert!(
            std::fs::symlink_metadata(path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let sentinel = if name == "embeddings" {
            external.join("vectors.jsonl")
        } else {
            external
        };
        assert_eq!(std::fs::read(sentinel).unwrap(), b"outside");
    }
}

#[test]
fn missing_source_memory_is_not_created_by_reading() {
    let home = tempfile::tempdir().unwrap();
    assert!(MemoryStore::existing(home.path(), "absent").is_err());
    assert!(MemoryStore::snapshot(home.path(), "absent").is_err());
    assert!(!home.path().join("memory/absent").exists());
}

#[tokio::test]
async fn migration_preserves_embedding_bytes_or_explicitly_drops_unsupported_vectors() {
    use rigspark_runtime::memory::{CaptureOptions, MigrationOptions};
    let home = tempfile::tempdir().unwrap();
    let source = MemoryStore::open(home.path(), "source", "now").unwrap();
    let embedder = EmbeddingFixture {
        model: "embedding",
        dimension: 2,
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    source
        .capture(
            "question",
            "answer",
            CaptureOptions {
                timestamp: "now",
                embedder: Some(&embedder),
                embedding_unsupported: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    let before = source.load().unwrap();
    for unsupported in [false, true] {
        let target = source
            .migrate(
                if unsupported { "vectorless" } else { "reuse" },
                "now",
                MigrationOptions {
                    context: 8192,
                    embedder: None,
                    target_dimension: None,
                    summarizer: None,
                    embedding_unsupported: unsupported,
                },
                false,
                &cancel,
            )
            .await
            .unwrap();
        if unsupported {
            assert!(target.load().unwrap().embedding.is_none());
            assert_eq!(
                target.read_meta().unwrap().embedding_unsupported,
                Some(true)
            );
            assert!(!target.dir.join("embeddings/vectors.jsonl").exists());
        } else {
            assert_eq!(
                serde_json::to_value(target.load().unwrap().embedding).unwrap(),
                serde_json::to_value(&before.embedding).unwrap()
            );
        }
        assert_eq!(source.load().unwrap(), before);
    }
}

#[tokio::test]
async fn source_drift_and_under_lock_failure_prevent_migration_commit() {
    use rigspark_runtime::memory::{CaptureOptions, MemoryError, MigrationOptions};
    let home = tempfile::tempdir().unwrap();
    let source = MemoryStore::open(home.path(), "source", "now").unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let options = || MigrationOptions {
        context: 8192,
        embedder: None,
        target_dimension: None,
        summarizer: None,
        embedding_unsupported: false,
    };
    let prepared = source
        .prepare_migration("target", options(), &cancel)
        .await
        .unwrap();
    source
        .capture(
            "concurrent",
            "reply",
            CaptureOptions {
                timestamp: "now",
                embedder: None,
                embedding_unsupported: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(
        source
            .commit_migration(prepared, "now", false, &cancel)
            .is_err()
    );
    assert!(!home.path().join("memory/target").exists());
    let before = source.load().unwrap();
    let prepared = source
        .prepare_migration("target", options(), &cancel)
        .await
        .unwrap();
    assert!(
        source
            .commit_migration_with(prepared, "now", false, &cancel, || Err(MemoryError(
                "runtime changed under lock".into()
            )))
            .is_err()
    );
    assert_eq!(source.load().unwrap(), before);
    assert!(!home.path().join("memory/target").exists());
}
