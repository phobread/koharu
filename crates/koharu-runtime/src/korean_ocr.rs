use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::archive::{self, ArchiveKind, ExtractPolicy};
use crate::install::InstallState;
use crate::{PackageFuture, PackageKind, PackageRegistration, Runtime};

const ORT_VERSION: &str = "1.22.0";
const ORT_WHEEL_NAME: &str = "onnxruntime-1.22.0-cp310-cp310-win_amd64.whl";
const ORT_WHEEL_URL: &str = "https://files.pythonhosted.org/packages/6d/6b/8267490476e8d4dd1883632c7e46a4634384c7ff1c35ae44edc8ab0bb7a9/onnxruntime-1.22.0-cp310-cp310-win_amd64.whl";
const MODEL_ARCHIVE_NAME: &str = "korean_PP-OCRv5_mobile_rec_onnx_infer.tar";
const MODEL_ARCHIVE_URL: &str = "https://paddle-model-ecology.bj.bcebos.com/paddlex/official_inference_model/paddle3.0.0/korean_PP-OCRv5_mobile_rec_onnx_infer.tar";

const ORT_SOURCE_ID: &str = concat!("onnxruntime/", "1.22.0", "/windows-x64");
const MODEL_SOURCE_ID: &str = "paddle3.0.0/korean_PP-OCRv5_mobile_rec_onnx_infer";

#[derive(Debug, Clone)]
pub struct KoreanOcrAssets {
    pub runtime_library: PathBuf,
    pub model: PathBuf,
    pub metadata: PathBuf,
}

fn supported(_: &Runtime) -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
}

fn ort_dir(runtime: &Runtime) -> PathBuf {
    runtime
        .root()
        .join("runtime")
        .join("onnxruntime")
        .join(ORT_VERSION)
        .join("windows-x64")
}

fn model_dir(runtime: &Runtime) -> PathBuf {
    runtime
        .root()
        .join("models")
        .join("paddleocr")
        .join("korean_PP-OCRv5_mobile_rec")
}

fn ort_present(runtime: &Runtime) -> Result<bool> {
    let dir = ort_dir(runtime);
    Ok(
        InstallState::new(&dir, ORT_SOURCE_ID).is_current()
            && dir.join("onnxruntime.dll").is_file(),
    )
}

fn model_present(runtime: &Runtime) -> Result<bool> {
    let dir = model_dir(runtime);
    Ok(InstallState::new(&dir, MODEL_SOURCE_ID).is_current()
        && dir.join("inference.onnx").is_file()
        && dir.join("inference.yml").is_file())
}

async fn prepare_ort(runtime: &Runtime) -> Result<()> {
    if !supported(runtime) {
        bail!("Korean OCR verifier is currently supported on Windows x64 only");
    }
    if ort_present(runtime)? {
        return Ok(());
    }

    let archive_path = runtime
        .downloads()
        .cached_download(ORT_WHEEL_URL, ORT_WHEEL_NAME)
        .await?;
    let dir = ort_dir(runtime);
    let state = InstallState::new(&dir, ORT_SOURCE_ID);
    state.reset()?;
    archive::extract(
        &archive_path,
        &dir,
        ArchiveKind::Zip,
        ExtractPolicy::Selected(&["onnxruntime.dll"]),
    )?;
    require_file(&dir.join("onnxruntime.dll"))?;
    state.commit()?;
    remove_installer(&archive_path).await;
    Ok(())
}

async fn prepare_model(runtime: &Runtime) -> Result<()> {
    if !supported(runtime) {
        bail!("Korean OCR verifier is currently supported on Windows x64 only");
    }
    if model_present(runtime)? {
        return Ok(());
    }

    let archive_path = runtime
        .downloads()
        .cached_download(MODEL_ARCHIVE_URL, MODEL_ARCHIVE_NAME)
        .await?;
    let dir = model_dir(runtime);
    let state = InstallState::new(&dir, MODEL_SOURCE_ID);
    state.reset()?;
    archive::extract(
        &archive_path,
        &dir,
        ArchiveKind::Tar,
        ExtractPolicy::Selected(&["inference.onnx", "inference.yml"]),
    )?;
    require_file(&dir.join("inference.onnx"))?;
    require_file(&dir.join("inference.yml"))?;
    state.commit()?;
    remove_installer(&archive_path).await;
    Ok(())
}

fn require_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("runtime archive did not contain `{}`", path.display());
    }
    Ok(())
}

async fn remove_installer(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await {
        tracing::warn!(path = %path.display(), %error, "failed to remove downloaded installer");
    }
}

pub(crate) async fn ensure_assets(runtime: &Runtime) -> Result<KoreanOcrAssets> {
    let ((), ()) = tokio::try_join!(prepare_ort(runtime), prepare_model(runtime))?;
    let ort = ort_dir(runtime);
    let model = model_dir(runtime);
    Ok(KoreanOcrAssets {
        runtime_library: ort.join("onnxruntime.dll"),
        model: model.join("inference.onnx"),
        metadata: model.join("inference.yml"),
    })
}

fn ensure_ort(runtime: &Runtime) -> PackageFuture<'_> {
    Box::pin(prepare_ort(runtime))
}

fn ensure_model(runtime: &Runtime) -> PackageFuture<'_> {
    Box::pin(prepare_model(runtime))
}

inventory::submit! {
    PackageRegistration {
        id: "runtime:onnxruntime-cpu",
        kind: PackageKind::Native,
        bootstrap: false,
        order: 30,
        enabled: supported,
        present: ort_present,
        ensure: ensure_ort,
    }
}

inventory::submit! {
    PackageRegistration {
        id: "model:korean-pp-ocr-v5",
        kind: PackageKind::Model,
        bootstrap: false,
        order: 220,
        enabled: supported,
        present: model_present,
        ensure: ensure_model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ComputePolicy;

    /// Both archives go through `Downloads::cached_download`, which refuses
    /// any URL without a pinned SHA-256. These are fork-only downloads the
    /// upstream checksum list never covered, so pin them explicitly here: if a
    /// regenerated `checksums.txt` ever drops them, this fails instead of
    /// Korean OCR breaking on the next fresh install.
    #[test]
    fn korean_ocr_downloads_have_pinned_checksums() {
        crate::checksums::expected_sha256(ORT_WHEEL_URL).unwrap();
        crate::checksums::expected_sha256(MODEL_ARCHIVE_URL).unwrap();
    }

    /// End-to-end: a fresh install fetches both archives through the verified,
    /// fail-closed download path and extracts them. Network; run with
    /// `cargo test -p koharu-runtime -- --ignored fresh_install`.
    #[tokio::test]
    #[ignore = "downloads ~27 MB (onnxruntime wheel + Korean model) from PyPI and Baidu BOS"]
    async fn fresh_install_downloads_verified_assets() {
        if !cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            return;
        }
        let tempdir = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(tempdir.path(), ComputePolicy::CpuOnly).unwrap();
        let assets = ensure_assets(&runtime).await.unwrap();
        assert!(assets.runtime_library.is_file());
        assert!(assets.model.is_file());
        assert!(assets.metadata.is_file());
    }

    #[test]
    fn assets_live_outside_the_huggingface_cache() {
        let tempdir = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(tempdir.path(), ComputePolicy::CpuOnly).unwrap();

        assert!(ort_dir(&runtime).ends_with("runtime/onnxruntime/1.22.0/windows-x64"));
        assert!(model_dir(&runtime).ends_with("models/paddleocr/korean_PP-OCRv5_mobile_rec"));
    }
}
