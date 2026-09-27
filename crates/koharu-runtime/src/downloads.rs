use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt, TryStreamExt};
use hf_hub::{
    Cache, Repo, RepoType,
    api::tokio::{ApiBuilder, Metadata},
};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use koharu_core::events::{DownloadProgress, DownloadStatus};
use reqwest::header::{CONTENT_LENGTH, RANGE};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::broadcast;

use crate::checksums;
use crate::model_pins::{self, ModelFile, ModelPin};
use crate::runtime::{RuntimeHttpClient, RuntimeHttpConfig};

/// 10 MiB per ranged GET — same size hf-hub's `.high()` mode uses. Short enough
/// that reqwest's read_timeout catches a stalled connection quickly, and the
/// retry middleware can restart the chunk.
const CHUNK_SIZE: u64 = 10 * 1024 * 1024;

/// hf-hub's internal client has no read timeout, so we cap the metadata call
/// ourselves. The response body is a single byte — a short cap is safe.
const HF_METADATA_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Downloads — unified download manager
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Downloads {
    downloads_root: PathBuf,
    huggingface_cache: Cache,
    client: RuntimeHttpClient,
    tx: broadcast::Sender<DownloadProgress>,
    progress: Arc<MultiProgress>,
}

impl Downloads {
    pub(crate) fn new(
        downloads_root: PathBuf,
        huggingface_root: PathBuf,
        http: &RuntimeHttpConfig,
    ) -> Result<Self> {
        let client = http.build_client()?;

        Ok(Self {
            downloads_root,
            huggingface_cache: Cache::new(huggingface_root),
            client,
            tx: broadcast::channel(256).0,
            progress: Arc::new(MultiProgress::new()),
        })
    }

    pub fn client(&self) -> RuntimeHttpClient {
        Arc::clone(&self.client)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DownloadProgress> {
        self.tx.subscribe()
    }

    /// Download a HuggingFace model file, using the local cache first.
    ///
    /// hf-hub resolves URL + metadata + cache layout; the byte transfer runs
    /// on our retry-configured client so a stalled chunk is retried by the
    /// middleware instead of hanging the future.
    pub async fn huggingface_model(&self, repo: &str, filename: &str) -> Result<PathBuf> {
        self.download_huggingface(repo, filename, None).await
    }

    /// Resolve a bundled image-processing model at its audited immutable revision.
    /// Does not consult or update `refs/main` and never falls back to another revision.
    pub async fn bundled_model(&self, repo: &str, filename: &str) -> Result<PathBuf> {
        let (pin, file) = model_pins::get(repo, filename)?;
        self.download_huggingface(repo, filename, Some((pin, file)))
            .await
    }

    /// Read-only presence check shared by package registration and actual loading.
    pub fn cached_bundled_model(&self, repo: &str, filename: &str) -> Result<Option<PathBuf>> {
        let (pin, file) = model_pins::get(repo, filename)?;
        let cache = self.huggingface_cache.repo(pinned_repo(pin));
        cached_pinned_file(&cache, pin, file)
    }

    async fn download_huggingface(
        &self,
        repo: &str,
        filename: &str,
        pinned: Option<(&ModelPin, &ModelFile)>,
    ) -> Result<PathBuf> {
        let repository = pinned.map_or_else(
            || Repo::new(repo.to_string(), RepoType::Model),
            |(pin, _)| pinned_repo(pin),
        );
        let cache_repo = self.huggingface_cache.repo(repository.clone());

        let cached = match pinned {
            Some((pin, file)) => cached_pinned_file(&cache_repo, pin, file)?,
            None => cache_repo.get(filename),
        };
        if let Some(path) = cached {
            return Ok(path);
        }

        let api = ApiBuilder::from_cache(self.huggingface_cache.clone())
            .with_progress(false)
            .with_user_agent("koharu", env!("CARGO_PKG_VERSION"))
            .build()
            .context("failed to build HF Hub API")?;
        let repo_handle = api.repo(repository);
        let url = repo_handle.url(filename);
        let label = pinned.map_or_else(
            || format!("{repo}/{filename}"),
            |(pin, _)| format!("{repo}@{}/{filename}", pin.revision),
        );

        let metadata: Metadata = tokio::time::timeout(HF_METADATA_TIMEOUT, api.metadata(&url))
            .await
            .map_err(|_| anyhow::anyhow!("HF metadata request timed out for `{label}`"))?
            .with_context(|| format!("failed to fetch HF metadata for `{label}`"))?;

        if let Some((pin, file)) = pinned {
            validate_pinned_metadata(
                pin,
                file,
                metadata.commit_hash(),
                metadata.etag(),
                metadata.size() as u64,
            )?;
        }

        let blob_path = cache_repo.blob_path(metadata.etag());
        if let Some(parent) = blob_path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create HF blob directory `{}`", parent.display())
            })?;
        }

        if !blob_path.exists() {
            let reporter = self.begin(filename);
            let mut result = self
                .ranged_download(&url, &blob_path, &reporter, Some(metadata.size() as u64))
                .await;
            if result.is_ok()
                && let Some((_, file)) = pinned
            {
                result = verify_pinned_download(&blob_path, file).await;
                if result.is_err() {
                    tokio::fs::remove_file(&blob_path).await.ok();
                }
            }
            if let Err(error) = result {
                reporter.fail(&error);
                return Err(error.context(format!("failed to download HF model file `{label}`")));
            }
            reporter.finish();
        }

        let pointer_dir = cache_repo.pointer_path(metadata.commit_hash());
        let pointer_path = pointer_dir.join(filename);
        if let Some(parent) = pointer_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        if !pointer_path.exists() {
            #[cfg(target_os = "windows")]
            std::os::windows::fs::symlink_file(&blob_path, &pointer_path).ok();
            #[cfg(target_family = "unix")]
            std::os::unix::fs::symlink(&blob_path, &pointer_path).ok();
        }
        // Pinned cache lookup goes straight to snapshots/<commit> or its known
        // content-addressed blob. No ref is required, even on Windows without
        // symlink privileges. Preserve the legacy branch ref for custom models.
        if pinned.is_none() {
            cache_repo
                .create_ref(metadata.commit_hash())
                .context("failed to create HF cache ref")?;
        }

        Ok(if pointer_path.exists() {
            pointer_path
        } else {
            blob_path
        })
    }

    /// Download a file to the downloads cache, returning the cached path.
    ///
    /// The file must match the SHA-256 pinned for `url` in `checksums.txt`
    /// (see [`crate::checksums`]). A cached copy that doesn't match is
    /// discarded and downloaded again; a fresh download that doesn't match is
    /// deleted and rejected; an unpinned URL is refused before downloading.
    pub(crate) async fn cached_download(&self, url: &str, file_name: &str) -> Result<PathBuf> {
        let expected = checksums::expected_sha256(url)?;
        self.download_verified(url, file_name, expected).await
    }

    /// [`Self::cached_download`] against an explicit SHA-256.
    async fn download_verified(
        &self,
        url: &str,
        file_name: &str,
        expected: &str,
    ) -> Result<PathBuf> {
        let destination = self.downloads_root.join(file_name);
        if destination.exists() {
            match verify_file(&destination, expected).await {
                Ok(()) => return Ok(destination),
                Err(error) => {
                    tracing::warn!("discarding cached download: {error:#}");
                    tokio::fs::remove_file(&destination)
                        .await
                        .with_context(|| format!("failed to remove `{}`", destination.display()))?;
                }
            }
        }

        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create `{}`", parent.display()))?;
        }

        let reporter = self.begin(file_name);
        let result = match self
            .ranged_download(url, &destination, &reporter, None)
            .await
        {
            Ok(()) => verify_file(&destination, expected).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            tokio::fs::remove_file(&destination).await.ok();
            reporter.fail(&error);
            return Err(error);
        }
        reporter.finish();
        Ok(destination)
    }

    /// Stream a URL to `destination` as a set of ranged GETs running up to
    /// `chunk_parallelism()` in flight (defaults to the host's CPU core count).
    /// The temp file is pre-allocated to the full size so each worker can
    /// seek-and-write its range independently. Transient failures surface as
    /// `Err`; the retry middleware on `self.client` retries at the request
    /// level, and when retries are exhausted the whole download fails cleanly.
    async fn ranged_download(
        &self,
        url: &str,
        destination: &Path,
        reporter: &TransferReporter,
        total_hint: Option<u64>,
    ) -> Result<()> {
        let total = match total_hint {
            Some(t) => t,
            None => self.probe_content_length(url).await?,
        };
        reporter.start(Some(total));

        let temp = part_path(destination)?;
        tokio::fs::remove_file(&temp).await.ok();
        {
            let file = tokio::fs::File::create(&temp)
                .await
                .with_context(|| format!("failed to create `{}`", temp.display()))?;
            file.set_len(total)
                .await
                .with_context(|| format!("failed to preallocate `{}`", temp.display()))?;
        }

        let mut chunks = Vec::new();
        let mut start: u64 = 0;
        while start < total {
            let stop = (start + CHUNK_SIZE).min(total) - 1;
            chunks.push((start, stop));
            start = stop + 1;
        }

        let temp_ref: &Path = &temp;
        let write_result: Result<()> = stream::iter(chunks)
            .map(|(start, stop)| async move {
                let range = format!("bytes={start}-{stop}");
                let response = self
                    .client
                    .get(url)
                    .header(RANGE, &range)
                    .send()
                    .await
                    .with_context(|| format!("failed to fetch range {range} of `{url}`"))?
                    .error_for_status()
                    .with_context(|| format!("fetch failed for range {range} of `{url}`"))?;
                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("failed to read range {range} of `{url}`"))?;
                // A server or proxy that ignores `Range` sends the whole file.
                let expected = stop - start + 1;
                anyhow::ensure!(
                    bytes.len() as u64 == expected,
                    "range {range} of `{url}` returned {} bytes instead of {expected}",
                    bytes.len()
                );
                let mut file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(temp_ref)
                    .await
                    .with_context(|| format!("failed to open `{}`", temp_ref.display()))?;
                file.seek(std::io::SeekFrom::Start(start))
                    .await
                    .with_context(|| format!("failed to seek in `{}`", temp_ref.display()))?;
                file.write_all(&bytes)
                    .await
                    .with_context(|| format!("failed to write `{}`", temp_ref.display()))?;
                file.flush()
                    .await
                    .with_context(|| format!("failed to flush `{}`", temp_ref.display()))?;
                reporter.advance(bytes.len());
                Ok::<_, anyhow::Error>(())
            })
            .buffer_unordered(num_cpus::get())
            .try_collect()
            .await;

        if let Err(err) = write_result {
            tokio::fs::remove_file(&temp).await.ok();
            return Err(err);
        }

        tokio::fs::remove_file(destination).await.ok();
        tokio::fs::rename(&temp, destination)
            .await
            .with_context(|| {
                format!(
                    "failed to rename `{}` → `{}`",
                    temp.display(),
                    destination.display()
                )
            })?;
        Ok(())
    }

    async fn probe_content_length(&self, url: &str) -> Result<u64> {
        let response = self
            .client
            .head(url)
            .send()
            .await
            .with_context(|| format!("failed to HEAD `{url}`"))?
            .error_for_status()
            .with_context(|| format!("HEAD failed for `{url}`"))?;

        let content_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .ok_or_else(|| anyhow::anyhow!("missing Content-Length for `{url}`"))?
            .to_str()
            .context("invalid Content-Length header")?;
        content_length
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid Content-Length `{content_length}` for `{url}`"))
    }

    fn begin(&self, label: &str) -> TransferReporter {
        let bar = self.progress.add(ProgressBar::new_spinner());
        bar.enable_steady_tick(Duration::from_millis(120));
        bar.set_style(
            ProgressStyle::with_template(
                "{msg} [{elapsed_precise}] [{wide_bar}] {bytes}/{total_bytes} ({eta})",
            )
            .expect("progress style"),
        );
        bar.set_message(label.to_string());
        TransferReporter::new(self.tx.clone(), bar, label)
    }
}

// ---------------------------------------------------------------------------
// Transfer progress reporter
// ---------------------------------------------------------------------------

const UNKNOWN_TOTAL: u64 = u64::MAX;

#[derive(Clone)]
struct TransferReporter {
    tx: broadcast::Sender<DownloadProgress>,
    bar: ProgressBar,
    filename: Arc<str>,
    downloaded: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

impl TransferReporter {
    fn new(tx: broadcast::Sender<DownloadProgress>, bar: ProgressBar, label: &str) -> Self {
        Self {
            tx,
            bar,
            filename: Arc::<str>::from(label),
            downloaded: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(UNKNOWN_TOTAL)),
        }
    }

    fn start(&self, total: Option<u64>) {
        self.total
            .store(total.unwrap_or(UNKNOWN_TOTAL), Ordering::Relaxed);
        self.downloaded.store(0, Ordering::Relaxed);
        self.bar.set_length(total.unwrap_or(0));
        self.bar.set_position(0);
        self.emit(DownloadStatus::Started);
    }

    fn advance(&self, delta: usize) {
        self.downloaded.fetch_add(delta as u64, Ordering::Relaxed);
        self.bar.inc(delta as u64);
        self.emit(DownloadStatus::Downloading);
    }

    fn finish(&self) {
        self.bar.finish_and_clear();
        self.emit(DownloadStatus::Completed);
    }

    fn fail(&self, error: &anyhow::Error) {
        self.bar.finish_and_clear();
        self.emit(DownloadStatus::Failed {
            reason: error.to_string(),
        });
    }

    fn emit(&self, status: DownloadStatus) {
        let total = self.total.load(Ordering::Relaxed);
        let _ = self.tx.send(DownloadProgress {
            id: self.filename.to_string(),
            filename: self.filename.to_string(),
            downloaded: self.downloaded.load(Ordering::Relaxed),
            total: (total != UNKNOWN_TOTAL).then_some(total),
            status,
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pinned_repo(pin: &ModelPin) -> Repo {
    Repo::with_revision(
        pin.repo.to_string(),
        RepoType::Model,
        pin.revision.to_string(),
    )
}

fn cached_pinned_file(
    cache: &hf_hub::CacheRepo,
    pin: &ModelPin,
    file: &ModelFile,
) -> Result<Option<PathBuf>> {
    // hf-hub 0.5's get() requires refs/<revision>, even for a full commit hash.
    // Existing Windows installs may also have blobs with no snapshot symlinks.
    for path in [
        cache.pointer_path(pin.revision).join(file.filename),
        cache.blob_path(file.oid),
    ] {
        match std::fs::metadata(&path) {
            Ok(metadata) => {
                anyhow::ensure!(
                    metadata.is_file() && metadata.len() == file.size,
                    "cached model `{}/{}@{}` has the wrong size; expected {} bytes at `{}`",
                    pin.repo,
                    file.filename,
                    pin.revision,
                    file.size,
                    path.display()
                );
                return Ok(Some(path));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot read model cache `{}`", path.display()));
            }
        }
    }
    Ok(None)
}

fn validate_pinned_metadata(
    pin: &ModelPin,
    file: &ModelFile,
    commit: &str,
    etag: &str,
    size: u64,
) -> Result<()> {
    anyhow::ensure!(
        commit == pin.revision && etag == file.oid && size == file.size,
        "HF metadata does not match pinned model `{}/{}@{}`; refusing a different artifact",
        pin.repo,
        file.filename,
        pin.revision
    );
    Ok(())
}

/// A pinned LFS file's oid is its SHA-256, so the downloaded bytes are
/// checked, not only the metadata the server reports. Git-blob SHA-1 oids
/// belong to small config files, which stay size-checked.
async fn verify_pinned_download(path: &Path, file: &ModelFile) -> Result<()> {
    if file.oid.len() == 64 {
        verify_file(path, file.oid).await?;
    }
    Ok(())
}

/// [`checksums::verify_file`] off the async runtime; archives can be hundreds
/// of MB.
async fn verify_file(path: &Path, expected: &str) -> Result<()> {
    let path = path.to_path_buf();
    let expected = expected.to_owned();
    tokio::task::spawn_blocking(move || checksums::verify_file(&path, &expected))
        .await
        .context("checksum task panicked")?
}

fn part_path(destination: &Path) -> Result<PathBuf> {
    let file_name = destination.file_name().ok_or_else(|| {
        anyhow::anyhow!(
            "destination `{}` does not have a filename",
            destination.display()
        )
    })?;
    Ok(destination.with_file_name(format!("{}.part", file_name.to_string_lossy())))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::part_path;

    use super::*;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    const BODY: &[u8] = b"koharu runtime archive";

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Serves `body` for any path, with the HEAD + single-range GET subset
    /// that `ranged_download` uses. Returns the base URL.
    async fn serve(body: &'static [u8]) -> String {
        serve_with(body, true).await
    }

    /// Like [`serve`]; with `honour_ranges` false every GET gets the whole
    /// body, as from a server or proxy that ignores `Range`.
    async fn serve_with(body: &'static [u8], honour_ranges: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let mut len = 0;
                    while !buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = socket.read(&mut buf[len..]).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        len += n;
                    }
                    let request = String::from_utf8_lossy(&buf[..len]).to_ascii_lowercase();
                    let range = request.lines().find_map(|line| {
                        let (start, stop) = line.strip_prefix("range: bytes=")?.split_once('-')?;
                        Some((
                            start.parse::<usize>().ok()?,
                            stop.trim().parse::<usize>().ok()?,
                        ))
                    });
                    let (status, bytes) = match range {
                        Some((start, stop)) if honour_ranges => {
                            ("206 Partial Content", &body[start..=stop])
                        }
                        _ => ("200 OK", body),
                    };
                    let header = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    );
                    socket.write_all(header.as_bytes()).await.unwrap();
                    if !request.starts_with("head") {
                        socket.write_all(bytes).await.unwrap();
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    static TEST_FILE: ModelFile = ModelFile {
        filename: "model.bin",
        oid: "1111111111111111111111111111111111111111",
        size: 4,
    };
    static TEST_PIN: ModelPin = ModelPin {
        repo: "test/model",
        revision: "2222222222222222222222222222222222222222",
        files: &[],
    };

    fn test_downloads(temp: &tempfile::TempDir) -> Downloads {
        Downloads::new(
            temp.path().join("downloads"),
            temp.path().join("hf"),
            &RuntimeHttpConfig::default(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn pinned_cache_works_without_refs_or_snapshot_links() {
        let temp = tempfile::tempdir().unwrap();
        let downloads = test_downloads(&temp);
        let cache = downloads.huggingface_cache.repo(pinned_repo(&TEST_PIN));
        let blob = cache.blob_path(TEST_FILE.oid);
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::write(&blob, b"tiny").unwrap();
        for _ in 0..2 {
            let path = downloads
                .download_huggingface(
                    TEST_PIN.repo,
                    TEST_FILE.filename,
                    Some((&TEST_PIN, &TEST_FILE)),
                )
                .await
                .unwrap();
            assert_eq!(path, blob);
            assert_eq!(std::fs::read(path).unwrap(), b"tiny");
        }
        assert!(!temp.path().join("hf/models--test--model/refs").exists());
    }

    #[test]
    fn pinned_snapshot_ignores_a_moving_main_ref() {
        let temp = tempfile::tempdir().unwrap();
        let downloads = test_downloads(&temp);
        let cache = downloads.huggingface_cache.repo(pinned_repo(&TEST_PIN));
        let old_snapshot = cache
            .pointer_path(TEST_PIN.revision)
            .join(TEST_FILE.filename);
        std::fs::create_dir_all(old_snapshot.parent().unwrap()).unwrap();
        std::fs::write(&old_snapshot, b"old!").unwrap();
        let main = downloads.huggingface_cache.model(TEST_PIN.repo.into());
        main.create_ref("3333333333333333333333333333333333333333")
            .unwrap();
        let new_snapshot = main
            .pointer_path("3333333333333333333333333333333333333333")
            .join(TEST_FILE.filename);
        std::fs::create_dir_all(new_snapshot.parent().unwrap()).unwrap();
        std::fs::write(new_snapshot, b"new!").unwrap();
        assert_eq!(
            cached_pinned_file(&cache, &TEST_PIN, &TEST_FILE).unwrap(),
            Some(old_snapshot.clone())
        );
        std::fs::remove_file(old_snapshot).unwrap();
        assert!(
            cached_pinned_file(&cache, &TEST_PIN, &TEST_FILE)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn custom_repository_keeps_its_existing_branch_cache() {
        let temp = tempfile::tempdir().unwrap();
        let downloads = test_downloads(&temp);
        let cache = downloads.huggingface_cache.model("custom/model".into());
        cache.create_ref("custom-version").unwrap();
        let file = cache.pointer_path("custom-version").join("custom.gguf");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"custom").unwrap();
        assert_eq!(
            downloads
                .huggingface_model("custom/model", "custom.gguf")
                .await
                .unwrap(),
            file
        );
    }

    #[test]
    fn wrong_size_and_wrong_remote_revision_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let downloads = test_downloads(&temp);
        let cache = downloads.huggingface_cache.repo(pinned_repo(&TEST_PIN));
        let blob = cache.blob_path(TEST_FILE.oid);
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::write(&blob, b"partial").unwrap();
        assert!(
            cached_pinned_file(&cache, &TEST_PIN, &TEST_FILE)
                .unwrap_err()
                .to_string()
                .contains("wrong size")
        );
        validate_pinned_metadata(&TEST_PIN, &TEST_FILE, TEST_PIN.revision, TEST_FILE.oid, 4)
            .unwrap();
        for (revision, oid, size) in [
            ("main", TEST_FILE.oid, 4),
            (TEST_PIN.revision, "different", 4),
            (TEST_PIN.revision, TEST_FILE.oid, 5),
        ] {
            assert!(validate_pinned_metadata(&TEST_PIN, &TEST_FILE, revision, oid, size).is_err());
        }
    }

    #[tokio::test]
    #[ignore = "requires public HF network access; downloads only a 470-byte config into a temporary cache"]
    async fn downloads_small_pinned_asset_and_reuses_it() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let downloads = test_downloads(&temp);
        let repo = "ogkalu/comic-text-and-bubble-detector";
        let filename = "preprocessor_config.json";
        let first = downloads.bundled_model(repo, filename).await?;
        let bytes = std::fs::read(&first)?;
        assert_eq!(bytes.len(), 470);
        assert!(String::from_utf8(bytes.clone())?.contains("size"));
        let second = downloads.bundled_model(repo, filename).await?;
        assert_eq!(first, second);
        assert_eq!(bytes, std::fs::read(second)?);
        assert!(
            !temp
                .path()
                .join("hf/models--ogkalu--comic-text-and-bubble-detector/refs/main")
                .exists()
        );
        Ok(())
    }

    #[test]
    fn partial_download_path_appends_suffix() {
        let part = part_path(Path::new("/tmp/models/config.json")).unwrap();
        assert_eq!(part, Path::new("/tmp/models/config.json.part"));
    }

    #[tokio::test]
    async fn verified_download_accepts_matching_file() {
        let url = format!("{}/runtime.zip", serve(BODY).await);
        let root = tempfile::tempdir().unwrap();
        let path = test_downloads(&root)
            .download_verified(&url, "runtime.zip", &sha256_hex(BODY))
            .await
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), BODY);
    }

    #[tokio::test]
    async fn verified_download_rejects_and_removes_tampered_file() {
        let url = format!("{}/runtime.zip", serve(BODY).await);
        let root = tempfile::tempdir().unwrap();
        let err = test_downloads(&root)
            .download_verified(&url, "runtime.zip", &sha256_hex(b"the genuine archive"))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("checksum mismatch"), "{err:#}");
        assert!(!root.path().join("downloads/runtime.zip").exists());
    }

    #[tokio::test]
    async fn tampered_cache_entry_is_downloaded_again() {
        let url = format!("{}/runtime.zip", serve(BODY).await);
        let root = tempfile::tempdir().unwrap();
        let cached = root.path().join("downloads/runtime.zip");
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, b"tampered").unwrap();

        let path = test_downloads(&root)
            .download_verified(&url, "runtime.zip", &sha256_hex(BODY))
            .await
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), BODY);
    }

    #[tokio::test]
    async fn range_responses_of_the_wrong_length_are_rejected() {
        // Two chunks, so a server ignoring `Range` sends too much for each.
        let body: &'static [u8] = vec![7u8; CHUNK_SIZE as usize + 1].leak();
        let url = format!("{}/model.bin", serve_with(body, false).await);
        let root = tempfile::tempdir().unwrap();
        let err = test_downloads(&root)
            .download_verified(&url, "model.bin", &sha256_hex(body))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("instead of"), "{err:#}");
        assert!(!root.path().join("downloads/model.bin").exists());
    }

    #[tokio::test]
    async fn pinned_lfs_downloads_are_checked_against_their_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.safetensors");
        std::fs::write(&path, BODY).unwrap();
        let file = |oid: &'static str| ModelFile {
            filename: "model.safetensors",
            oid,
            size: BODY.len() as u64,
        };

        let genuine: &'static str = sha256_hex(BODY).leak();
        verify_pinned_download(&path, &file(genuine)).await.unwrap();
        let other: &'static str = sha256_hex(b"other weights").leak();
        let err = verify_pinned_download(&path, &file(other))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("checksum mismatch"), "{err:#}");
        // Git-blob SHA-1 oids (small non-LFS files) are size-checked only.
        verify_pinned_download(&path, &file(TEST_FILE.oid))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unpinned_url_is_refused_before_downloading() {
        let root = tempfile::tempdir().unwrap();
        let err = test_downloads(&root)
            .cached_download("https://example.com/unpinned.zip", "unpinned.zip")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no pinned SHA-256"), "{err:#}");
        assert!(!root.path().join("downloads/unpinned.zip").exists());
    }
}
