use rigspark_runtime::acquire::{Acquisition, Artifact, DownloadResponse, DownloadTransport};
use sha2::{Digest, Sha256};
use std::{io::Cursor, pin::Pin};

struct FakeTransport {
    bytes: Vec<u8>,
}
#[async_trait::async_trait]
impl DownloadTransport for FakeTransport {
    async fn get(&self, _url: &str) -> Result<DownloadResponse, String> {
        Ok(DownloadResponse {
            status: 200,
            commit: Some("a".repeat(40)),
            length: Some(self.bytes.len() as u64),
            body: Box::pin(Cursor::new(self.bytes.clone()))
                as Pin<Box<dyn tokio::io::AsyncRead + Send>>,
        })
    }
}
fn artifact(bytes: &[u8]) -> Artifact {
    Artifact {
        backend: "llamacpp".into(),
        repo: "owner/model".into(),
        revision: "a".repeat(40),
        file: "weights.gguf".into(),
        sha256: format!("{:x}", Sha256::digest(bytes)),
        bytes: bytes.len() as u64,
    }
}

#[test]
fn download_dns_rejects_private_mixed_mapped_and_special_addresses() {
    use rigspark_runtime::acquire::public_addresses;
    for address in [
        "127.0.0.1:443",
        "10.0.0.1:443",
        "169.254.169.254:443",
        "100.64.0.1:443",
        "0.0.0.0:443",
        "192.0.2.1:443",
        "[::1]:443",
        "[::ffff:127.0.0.1]:443",
        "[fc00::1]:443",
        "[2001:db8::1]:443",
    ] {
        assert!(
            public_addresses(vec![address.parse().unwrap()]).is_err(),
            "{address}"
        );
        assert!(
            public_addresses(vec![
                "8.8.8.8:443".parse().unwrap(),
                address.parse().unwrap()
            ])
            .is_err()
        );
    }
    assert!(public_addresses(Vec::new()).is_err());
    let socket = |address: &str| -> std::net::SocketAddr {
        std::net::SocketAddr::new(address.parse().unwrap(), 443)
    };
    for address in [
        "0.0.0.1",
        "10.0.0.1",
        "127.0.0.1",
        "100.64.0.1",
        "169.254.0.1",
        "172.16.0.1",
        "192.168.0.1",
        "192.0.0.8",
        "192.0.2.1",
        "198.18.0.1",
        "198.19.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "::",
        "4000::1",
        "2001:1::1",
        "2001:db8::1",
        "::ffff:192.168.0.1",
    ] {
        assert!(
            public_addresses(vec![socket(address)]).is_err(),
            "{address}"
        );
    }
    for address in [
        "8.8.8.8",
        "172.32.0.1",
        "169.253.0.1",
        "198.51.101.1",
        "203.0.114.1",
        "2001:4860::1",
        "2606:4700::1",
        "::ffff:8.8.8.8",
    ] {
        assert!(public_addresses(vec![socket(address)]).is_ok(), "{address}");
    }
    assert!(
        public_addresses(vec![
            "8.8.8.8:443".parse().unwrap(),
            "[2606:4700:4700::1111]:443".parse().unwrap()
        ])
        .is_ok()
    );
}

#[test]
fn filenames_reject_windows_aliases_on_every_platform() {
    for file in [
        "CON",
        "aux.json",
        "LPT1.safetensors",
        "weights.gguf.",
        "weights.gguf ",
        "COM9/config.json",
        "nested/../weights.gguf",
    ] {
        assert!(!rigspark_runtime::acquire::safe_file(file), "{file}");
    }
    assert!(rigspark_runtime::acquire::safe_file(
        "nested/model-00001.safetensors"
    ));
}

#[tokio::test]
async fn repository_rejects_case_aliases_and_file_directory_collisions_before_writing() {
    for names in [
        ["Weights.gguf", "weights.gguf"],
        ["weights", "weights/part.gguf"],
    ] {
        let root = tempfile::tempdir().unwrap();
        let acquire = Acquisition::new(root.path()).unwrap();
        let artifacts: Vec<_> = names
            .into_iter()
            .map(|name| {
                let mut request = artifact(b"verified");
                request.file = name.into();
                request
            })
            .collect();
        assert!(
            acquire
                .repository(
                    &artifacts,
                    &FakeTransport {
                        bytes: b"verified".to_vec()
                    },
                    &tokio_util::sync::CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn abandoned_partial_cleanup_preserves_live_unknown_and_unrelated_files() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        ".llmup-download.123.abc.part",
        ".llmup-download.456.abc.part",
        ".llmup-download.unknown.abc.part",
        "user.part",
    ] {
        std::fs::write(root.path().join(name), b"partial").unwrap();
    }
    rigspark_runtime::acquire::cleanup_partials(root.path(), |pid| pid != 123).unwrap();
    assert!(!root.path().join(".llmup-download.123.abc.part").exists());
    assert!(root.path().join(".llmup-download.456.abc.part").exists());
    assert!(
        root.path()
            .join(".llmup-download.unknown.abc.part")
            .exists()
    );
    assert!(root.path().join("user.part").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_cache_ancestor_cannot_create_directories_outside_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let acquire = Acquisition::new(root.path()).unwrap();
    std::fs::create_dir(outside.path().join("owner")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("llamacpp")).unwrap();
    assert!(
        acquire
            .acquire(
                &artifact(b"verified"),
                &FakeTransport {
                    bytes: b"verified".to_vec()
                },
                &tokio_util::sync::CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(outside.path().join("owner"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn reports_progress_without_changing_verified_bytes() {
    let root = tempfile::tempdir().unwrap();
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = events.clone();
    let acquire =
        Acquisition::new(root.path())
            .unwrap()
            .with_progress(move |completed, total, file| {
                recorded
                    .lock()
                    .unwrap()
                    .push((completed, total, file.to_owned()))
            });
    let request = artifact(b"verified");
    acquire
        .acquire(
            &request,
            &FakeTransport {
                bytes: b"verified".to_vec(),
            },
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        events.lock().unwrap().last(),
        Some(&(8, 8, "weights.gguf".into()))
    );
}

#[tokio::test]
async fn approximate_gguf_sizes_are_ceilings_without_weakening_digest_checks() {
    use rigspark_runtime::acquire::SizePolicy;
    let root = tempfile::tempdir().unwrap();
    let acquire = Acquisition::new(root.path()).unwrap();
    let mut request = artifact(b"verified");
    request.bytes = 100;
    let transport = FakeTransport {
        bytes: b"verified".to_vec(),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    assert!(
        acquire
            .acquire(&request, &transport, &cancel)
            .await
            .is_err()
    );
    let actual = acquire
        .acquire_sized(&request, SizePolicy::Ceiling, &transport, &cancel)
        .await
        .unwrap();
    assert_eq!(actual.bytes, 8);
    assert!(
        acquire
            .acquire_sized(&request, SizePolicy::Ceiling, &transport, &cancel)
            .await
            .unwrap()
            .cached
    );
    request.sha256 = "b".repeat(64);
    assert!(
        acquire
            .acquire_sized(&request, SizePolicy::Ceiling, &transport, &cancel)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn streams_verified_bytes_and_reuses_only_verified_cache() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"verified weights";
    let acquire = Acquisition::new(root.path()).unwrap();
    let transport = FakeTransport {
        bytes: bytes.to_vec(),
    };
    let result = acquire
        .acquire(
            &artifact(bytes),
            &transport,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&result.path).unwrap(), bytes);
    assert!(!result.cached);
    assert!(
        acquire
            .acquire(
                &artifact(bytes),
                &transport,
                &tokio_util::sync::CancellationToken::new()
            )
            .await
            .unwrap()
            .cached
    );
}
#[tokio::test]
async fn rejects_digest_mismatch_limits_traversal_and_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let acquire = Acquisition::new(root.path()).unwrap();
    let transport = FakeTransport {
        bytes: b"wrong weights".to_vec(),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    assert!(
        acquire
            .acquire(&artifact(b"good weights"), &transport, &cancel)
            .await
            .is_err()
    );
    let mut bad = artifact(b"wrong weights");
    bad.file = "../escape".into();
    assert!(acquire.acquire(&bad, &transport, &cancel).await.is_err());
    cancel.cancel();
    assert!(
        acquire
            .acquire(&artifact(b"wrong weights"), &transport, &cancel)
            .await
            .is_err()
    );
}

struct ResponseTransport {
    bytes: Vec<u8>,
    commit: Option<String>,
    length: Option<u64>,
}

struct StalledTransport {
    started: std::sync::Arc<tokio::sync::Notify>,
}
struct StalledReader(std::sync::Arc<tokio::sync::Notify>);
impl tokio::io::AsyncRead for StalledReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        _buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.0.notify_one();
        std::task::Poll::Pending
    }
}
#[async_trait::async_trait]
impl DownloadTransport for StalledTransport {
    async fn get(&self, _url: &str) -> Result<DownloadResponse, String> {
        Ok(DownloadResponse {
            status: 200,
            commit: Some("a".repeat(40)),
            length: None,
            body: Box::pin(StalledReader(self.started.clone())),
        })
    }
}

#[tokio::test]
async fn cancellation_during_a_stalled_body_removes_partial_and_lock() {
    let root = tempfile::tempdir().unwrap();
    let acquire = Acquisition::new(root.path()).unwrap();
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let transport = StalledTransport {
        started: started.clone(),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let request = artifact(b"verified");
    let action = acquire.acquire(&request, &transport, &cancel);
    let abort = async {
        started.notified().await;
        cancel.cancel();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(action, abort)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err(), "cancelled");
    let directory = root
        .path()
        .join("llamacpp/owner")
        .join(format!("model@{}", request.revision));
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
}
#[async_trait::async_trait]
impl DownloadTransport for ResponseTransport {
    async fn get(&self, _url: &str) -> Result<DownloadResponse, String> {
        Ok(DownloadResponse {
            status: 200,
            commit: self.commit.clone(),
            length: self.length,
            body: Box::pin(Cursor::new(self.bytes.clone())),
        })
    }
}

#[tokio::test]
async fn missing_commit_digest_mismatch_and_streamed_overflow_never_promote() {
    for (bytes, commit) in [
        (b"verified".to_vec(), None),
        (b"corrupt!".to_vec(), Some("a".repeat(40))),
        (b"verified plus overflow".to_vec(), Some("a".repeat(40))),
    ] {
        let root = tempfile::tempdir().unwrap();
        let acquire = Acquisition::new(root.path()).unwrap();
        let transport = ResponseTransport {
            bytes,
            commit,
            length: None,
        };
        let result = acquire
            .acquire(
                &artifact(b"verified"),
                &transport,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await;
        assert!(result.is_err());
        let directory = root
            .path()
            .join("llamacpp/owner")
            .join(format!("model@{}", "a".repeat(40)));
        assert_eq!(
            std::fs::read_dir(directory).unwrap().count(),
            0,
            "failed acquisition left lock or partial"
        );
    }
}

#[tokio::test]
async fn failed_redownload_preserves_existing_bytes() {
    let root = tempfile::tempdir().unwrap();
    let acquire = Acquisition::new(root.path()).unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let acquired = acquire
        .acquire(
            &artifact(b"verified"),
            &FakeTransport {
                bytes: b"verified".to_vec(),
            },
            &cancel,
        )
        .await
        .unwrap();
    std::fs::write(&acquired.path, b"existing cache").unwrap();
    assert!(
        acquire
            .acquire(
                &artifact(b"verified"),
                &FakeTransport {
                    bytes: b"corrupt!".to_vec()
                },
                &cancel
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&acquired.path).unwrap(), b"existing cache");
}

#[tokio::test]
async fn artifact_and_repository_locks_use_existing_cache_coordinates() {
    let root = tempfile::tempdir().unwrap();
    let acquire = Acquisition::new(root.path()).unwrap();
    let request = artifact(b"verified");
    let directory = root
        .path()
        .join("llamacpp/owner")
        .join(format!("model@{}", request.revision));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("weights.gguf.lock"),
        format!("{}", std::process::id()),
    )
    .unwrap();
    let transport = FakeTransport {
        bytes: b"verified".to_vec(),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    assert!(
        acquire
            .acquire(&request, &transport, &cancel)
            .await
            .is_err()
    );
    std::fs::remove_file(directory.join("weights.gguf.lock")).unwrap();
    std::fs::write(
        directory.with_file_name(format!("model@{}.repository.lock", request.revision)),
        format!("{}", std::process::id()),
    )
    .unwrap();
    assert!(
        acquire
            .acquire(&request, &transport, &cancel)
            .await
            .is_err(),
        "single-file acquisition bypassed repository lock"
    );
    assert!(
        acquire
            .repository(&[request], &transport, &cancel)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn mlx_repository_requires_complete_non_executable_manifest() {
    for files in [
        vec!["weights.safetensors"],
        vec![
            "config.json",
            "tokenizer_config.json",
            "weights.safetensors",
            "custom.py",
        ],
        vec!["config.json", "tokenizer_config.json", "notes.txt"],
    ] {
        let root = tempfile::tempdir().unwrap();
        let acquire = Acquisition::new(root.path()).unwrap();
        let artifacts: Vec<_> = files
            .into_iter()
            .map(|file| {
                let mut request = artifact(b"verified");
                request.backend = "mlx".into();
                request.file = file.into();
                request
            })
            .collect();
        assert!(
            acquire
                .repository(
                    &artifacts,
                    &FakeTransport {
                        bytes: b"verified".to_vec()
                    },
                    &tokio_util::sync::CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "invalid manifest wrote cache content"
        );
    }
}

/// Body that yields `bytes` and then fails like a dropped CDN connection.
struct Interrupted {
    bytes: Cursor<Vec<u8>>,
}
impl tokio::io::AsyncRead for Interrupted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.bytes.position() as usize == self.bytes.get_ref().len() {
            return std::task::Poll::Ready(Err(std::io::Error::other("connection reset")));
        }
        Pin::new(&mut self.bytes).poll_read(context, buffer)
    }
}
/// Drops the connection after every `chunk` bytes; resumes from `offset` when allowed.
struct Flaky {
    bytes: Vec<u8>,
    chunk: usize,
    resumable: bool,
    corrupt_resume: bool,
    offsets: std::sync::Mutex<Vec<u64>>,
}
impl Flaky {
    fn response(&self, offset: u64) -> DownloadResponse {
        let start = offset as usize;
        let end = (start + self.chunk).min(self.bytes.len());
        let mut slice = self.bytes[start..end].to_vec();
        if self.corrupt_resume && offset > 0 {
            slice[0] ^= 1;
        }
        let body: Pin<Box<dyn tokio::io::AsyncRead + Send>> = if end == self.bytes.len() {
            Box::pin(Cursor::new(slice))
        } else {
            Box::pin(Interrupted {
                bytes: Cursor::new(slice),
            })
        };
        DownloadResponse {
            status: if offset == 0 { 200 } else { 206 },
            commit: Some("a".repeat(40)),
            length: Some((self.bytes.len() - start) as u64),
            body,
        }
    }
}
#[async_trait::async_trait]
impl DownloadTransport for Flaky {
    async fn get(&self, _url: &str) -> Result<DownloadResponse, String> {
        self.offsets.lock().unwrap().push(0);
        Ok(self.response(0))
    }
    async fn get_from(&self, url: &str, offset: u64) -> Result<DownloadResponse, String> {
        if offset == 0 {
            return self.get(url).await;
        }
        if !self.resumable {
            return Err("download resume unsupported".into());
        }
        self.offsets.lock().unwrap().push(offset);
        Ok(self.response(offset))
    }
}
fn flaky(bytes: &[u8], resumable: bool, corrupt_resume: bool) -> Flaky {
    Flaky {
        bytes: bytes.to_vec(),
        chunk: 40_000,
        resumable,
        corrupt_resume,
        offsets: Default::default(),
    }
}

#[tokio::test]
async fn interrupted_downloads_resume_from_the_verified_offset() {
    let bytes: Vec<u8> = (0..100_000u32).map(|value| (value % 251) as u8).collect();
    let root = tempfile::tempdir().unwrap();
    let transport = flaky(&bytes, true, false);
    let acquired = Acquisition::new(root.path())
        .unwrap()
        .acquire(&artifact(&bytes), &transport, &Default::default())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&acquired.path).unwrap(), bytes);
    assert_eq!(*transport.offsets.lock().unwrap(), vec![0, 40_000, 80_000]);
}

#[tokio::test]
async fn resumed_bytes_are_still_hash_verified_and_unresumable_drops_fail() {
    let bytes: Vec<u8> = (0..100_000u32).map(|value| (value % 241) as u8).collect();
    for (transport, needle) in [
        (flaky(&bytes, true, true), "integrity mismatch"),
        (flaky(&bytes, false, false), "connection reset"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let error = Acquisition::new(root.path())
            .unwrap()
            .acquire(&artifact(&bytes), &transport, &Default::default())
            .await
            .unwrap_err();
        assert!(error.contains(needle), "{error}");
        let leftovers = walk(root.path());
        assert!(
            leftovers
                .iter()
                .all(|path| !path.ends_with(".gguf") && !path.ends_with(".part")),
            "{leftovers:?}"
        );
    }
}
fn walk(path: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            found.extend(walk(&entry.path()));
        } else {
            found.push(entry.path().display().to_string());
        }
    }
    found
}

#[cfg(unix)]
#[tokio::test]
async fn verified_cache_skips_rehash_until_the_file_or_stamp_changes() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"verified weights";
    let acquire = Acquisition::new(root.path()).unwrap();
    let transport = FakeTransport {
        bytes: bytes.to_vec(),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let downloaded = acquire
        .acquire(&artifact(bytes), &transport, &cancel)
        .await
        .unwrap();
    assert!(!downloaded.cached && !downloaded.rehashed);

    let reused = acquire
        .acquire(&artifact(bytes), &transport, &cancel)
        .await
        .unwrap();
    assert!(reused.cached, "unchanged file must be a cache hit");
    assert!(!reused.rehashed, "unchanged verified file was re-hashed");

    let stamps = root.path().join(".verified");
    for entry in std::fs::read_dir(&stamps).unwrap() {
        std::fs::write(entry.unwrap().path(), b"{not json").unwrap();
    }
    let unreadable = acquire
        .acquire(&artifact(bytes), &transport, &cancel)
        .await
        .unwrap();
    assert!(
        unreadable.cached && unreadable.rehashed,
        "bad stamp must force a full hash"
    );
    let restamped = acquire
        .acquire(&artifact(bytes), &transport, &cancel)
        .await
        .unwrap();
    assert!(
        restamped.cached && !restamped.rehashed,
        "full hash must refresh the stamp"
    );

    std::fs::remove_dir_all(&stamps).unwrap();
    let unstamped = acquire
        .acquire(&artifact(bytes), &transport, &cancel)
        .await
        .unwrap();
    assert!(
        unstamped.cached && unstamped.rehashed,
        "pre-existing caches are re-hashed once"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn verified_cache_detects_same_size_in_place_corruption() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"verified weights";
    let acquire = Acquisition::new(root.path()).unwrap();
    let transport = FakeTransport {
        bytes: bytes.to_vec(),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let first = acquire
        .acquire(&artifact(bytes), &transport, &cancel)
        .await
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&first.path)
            .unwrap();
        file.write_all(b"corrupt").unwrap();
    }
    assert_eq!(
        std::fs::metadata(&first.path).unwrap().len(),
        bytes.len() as u64
    );

    let repaired = acquire
        .acquire(&artifact(bytes), &transport, &cancel)
        .await
        .unwrap();
    assert!(!repaired.cached, "corrupted cache was trusted");
    assert_eq!(std::fs::read(&repaired.path).unwrap(), bytes);
}

#[tokio::test]
async fn verified_cache_stamps_stay_outside_mlx_repository_folders() {
    let root = tempfile::tempdir().unwrap();
    let acquire = Acquisition::new(root.path()).unwrap();
    let artifacts: Vec<_> = [
        "config.json",
        "tokenizer_config.json",
        "weights.safetensors",
    ]
    .into_iter()
    .map(|file| {
        let mut request = artifact(b"verified");
        request.backend = "mlx".into();
        request.file = file.into();
        request
    })
    .collect();
    let transport = FakeTransport {
        bytes: b"verified".to_vec(),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let first = acquire
        .repository(&artifacts, &transport, &cancel)
        .await
        .unwrap();
    let second = acquire
        .repository(&artifacts, &transport, &cancel)
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(std::fs::read_dir(&second).unwrap().count(), 3);
}
