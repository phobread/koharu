//! Remote inpainter. Sends the page + expanded mask to a user-hosted HTTP
//! service (e.g. FLUX.1 Fill on a rented GPU — see `remote-inpaint-server/`
//! at the repo root) and stores the returned image as
//! `Image { role: Inpainted }`.
//!
//! Configuration deliberately lives OUTSIDE `config.toml` (zero churn on the
//! upstream config plumbing): `<app data root>/remote_inpaint.toml`, re-read
//! on every run so endpoint changes apply without restarting the app. A
//! commented template is written on first use.
//!
//! Protocol: `POST {endpoint}/inpaint` as multipart form-data with `image`
//! (PNG) and `mask` (PNG, white = repaint) parts plus optional `prompt` /
//! `steps` / `guidance` text fields; `Authorization: Bearer <api_key>` when a
//! key is configured. The server responds with the full inpainted image
//! (same dimensions) as image bytes.

use std::io::Cursor;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use camino::Utf8PathBuf;
use image::{DynamicImage, GrayImage, Luma};
use koharu_core::{ImageRole, MaskRole, Op, Region};
use koharu_ml::inpainting::expand_mask_for_inpainting;
use koharu_ml::inpainting::mask::expand_mask_to_bubble_region_for_inpainting;
use koharu_runtime::default_app_data_root;
use serde::Deserialize;

use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::{
    find_image_node, find_mask_node, image_dimensions, load_source_image,
    restore_region_from_source, text_node_to_region, text_nodes, upsert_image_blob,
};

const CONFIG_FILE: &str = "remote_inpaint.toml";

const CONFIG_TEMPLATE: &str = r#"# Remote inpaint engine configuration. Re-read on every pipeline run —
# edits apply immediately, no app restart needed.

# Base URL of the inpaint server (required). For a RunPod pod exposing
# port 8000 this looks like: https://<pod-id>-8000.proxy.runpod.net
endpoint = ""

# Bearer token, must match the server's API_KEY env var. Leave empty if the
# server runs without auth.
api_key = ""

# Which erase mask to send:
#   "bubble" — whole bubble interior (what diffusion models like FLUX.1 Fill
#              want; they repaint the full region)
#   "glyph"  — dilated glyph outlines only (what LaMa-style erasers want)
mask = "bubble"

# Optional prompt forwarded to diffusion backends. When omitted the server's
# default applies.
# prompt = "empty white speech bubble, clean solid background"

# Optional overrides forwarded to the server.
# steps = 28
# guidance = 30.0

# Give up on a request after this many seconds.
timeout_secs = 600
"#;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MaskMode {
    Bubble,
    Glyph,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RemoteInpaintConfig {
    endpoint: String,
    api_key: String,
    mask: MaskMode,
    prompt: Option<String>,
    steps: Option<u32>,
    guidance: Option<f32>,
    timeout_secs: u64,
}

impl Default for RemoteInpaintConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            api_key: String::new(),
            mask: MaskMode::Bubble,
            prompt: None,
            steps: None,
            guidance: None,
            timeout_secs: 600,
        }
    }
}

fn config_path() -> Utf8PathBuf {
    default_app_data_root().join(CONFIG_FILE)
}

fn load_config() -> Result<RemoteInpaintConfig> {
    let path = config_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create `{parent}`"))?;
        }
        std::fs::write(&path, CONFIG_TEMPLATE)
            .with_context(|| format!("failed to write `{path}`"))?;
        bail!(
            "remote inpaint is not configured yet — a template was created at `{path}`; \
             set `endpoint` to your inpaint server's URL and re-run"
        );
    }
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("failed to read `{path}`"))?;
    let config: RemoteInpaintConfig =
        toml::from_str(&content).with_context(|| format!("failed to parse `{path}`"))?;
    if config.endpoint.trim().is_empty() {
        bail!("`endpoint` is empty in `{path}` — set it to your inpaint server's URL");
    }
    Ok(config)
}

pub struct Model;

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        let config = load_config()?;

        let (_, mask_ref) = find_mask_node(ctx.scene, ctx.page, MaskRole::Segment)
            .ok_or_else(|| anyhow!("no Segment mask on page"))?;
        let (_, bubble_ref) = find_mask_node(ctx.scene, ctx.page, MaskRole::Bubble)
            .ok_or_else(|| anyhow!("no Bubble mask on page"))?;
        let mask = ctx.blobs.load_image(&mask_ref)?;
        let bubble_mask = ctx.blobs.load_image(&bubble_ref)?;

        let (image, mask, bubble_mask) = match ctx.options.region {
            Some(r) => {
                let base = match find_image_node(ctx.scene, ctx.page, ImageRole::Inpainted) {
                    Some((_, blob)) => {
                        let inpainted = ctx.blobs.load_image(&blob)?;
                        let source = load_source_image(ctx.scene, ctx.page, ctx.blobs)?;
                        restore_region_from_source(&inpainted, &source, &r)
                    }
                    None => load_source_image(ctx.scene, ctx.page, ctx.blobs)?,
                };
                let clipped_mask = clip_mask_to_region(&mask, &r);
                let clipped_bubble = clip_mask_to_region(&bubble_mask, &r);
                (base, clipped_mask, clipped_bubble)
            }
            None => {
                let image = load_source_image(ctx.scene, ctx.page, ctx.blobs)?;
                (image, mask, bubble_mask)
            }
        };

        let text_blocks = text_nodes(ctx.scene, ctx.page)
            .into_iter()
            .map(|(_, transform, text)| text_node_to_region(transform, text))
            .collect::<Vec<_>>();
        let expanded = match config.mask {
            MaskMode::Bubble => {
                expand_mask_to_bubble_region_for_inpainting(&mask, &bubble_mask, &text_blocks)
            }
            MaskMode::Glyph => expand_mask_for_inpainting(&mask, &bubble_mask, &text_blocks),
        };
        let expanded = match ctx.options.region {
            Some(r) => clip_gray_mask_to_region(&expanded, &r),
            None => expanded,
        };

        if expanded.pixels().all(|p| p.0[0] == 0) {
            // Nothing to erase. For a regional run the base image already has
            // the region reverted to source (un-inpaint) — persist that.
            // A full-page run with an empty mask is a plain no-op.
            return match ctx.options.region {
                Some(_) => {
                    let (w, h) = image_dimensions(&image);
                    let blob = ctx.blobs.put_webp(&image)?;
                    Ok(vec![upsert_image_blob(
                        ctx.scene,
                        ctx.page,
                        ImageRole::Inpainted,
                        blob,
                        w,
                        h,
                    )])
                }
                None => Ok(Vec::new()),
            };
        }

        let mask_image = DynamicImage::ImageLuma8(expanded);
        let result = request_inpaint(&config, &image, &mask_image).await?;
        if ctx.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("cancelled");
        }
        let (w, h) = image_dimensions(&image);
        let (rw, rh) = image_dimensions(&result);
        if (rw, rh) != (w, h) {
            bail!(
                "remote inpaint server returned a {rw}x{rh} image for a {w}x{h} page — \
                 the server must preserve dimensions"
            );
        }
        let blob = ctx.blobs.put_webp(&result)?;
        Ok(vec![upsert_image_blob(
            ctx.scene,
            ctx.page,
            ImageRole::Inpainted,
            blob,
            w,
            h,
        )])
    }
}

async fn request_inpaint(
    config: &RemoteInpaintConfig,
    image: &DynamicImage,
    mask: &DynamicImage,
) -> Result<DynamicImage> {
    let url = format!("{}/inpaint", config.endpoint.trim().trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs))
        .build()
        .context("failed to build HTTP client")?;

    let mut form = reqwest::multipart::Form::new()
        .part(
            "image",
            reqwest::multipart::Part::bytes(png_bytes(image)?)
                .file_name("image.png")
                .mime_str("image/png")?,
        )
        .part(
            "mask",
            reqwest::multipart::Part::bytes(png_bytes(mask)?)
                .file_name("mask.png")
                .mime_str("image/png")?,
        );
    if let Some(prompt) = &config.prompt {
        form = form.text("prompt", prompt.clone());
    }
    if let Some(steps) = config.steps {
        form = form.text("steps", steps.to_string());
    }
    if let Some(guidance) = config.guidance {
        form = form.text("guidance", guidance.to_string());
    }

    let mut request = client.post(&url).multipart(form);
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(config.api_key.trim());
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("POST {url} failed — is the remote inpaint server running?"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(500).collect();
        bail!("remote inpaint server returned {status}: {body}");
    }
    let bytes = response.bytes().await?;
    image::load_from_memory(&bytes)
        .context("remote inpaint server returned a response that is not a decodable image")
}

fn png_bytes(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .context("failed to encode PNG")?;
    Ok(buf)
}

fn clip_mask_to_region(mask: &DynamicImage, region: &Region) -> DynamicImage {
    DynamicImage::ImageLuma8(clip_gray_mask_to_region(&mask.to_luma8(), region))
}

fn clip_gray_mask_to_region(src: &GrayImage, region: &Region) -> GrayImage {
    let (w, h) = src.dimensions();
    let x0 = region.x.min(w);
    let y0 = region.y.min(h);
    let x1 = region.x.saturating_add(region.width).min(w);
    let y1 = region.y.saturating_add(region.height).min(h);

    let mut clipped = GrayImage::new(w, h);
    for y in y0..y1 {
        for x in x0..x1 {
            clipped.put_pixel(x, y, Luma([src.get_pixel(x, y).0[0]]));
        }
    }
    clipped
}

inventory::submit! {
    EngineInfo {
        id: "remote-inpaint",
        name: "Remote Inpaint (HTTP)",
        needs: &[Artifact::SegmentMask, Artifact::BubbleMask],
        produces: &[Artifact::Inpainted],
        load: |_runtime, _cpu| Box::pin(async move {
            Ok(Box::new(Model) as Box<dyn Engine>)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_config_parses() {
        let config: RemoteInpaintConfig = toml::from_str(
            r#"
                endpoint = "https://abc-8000.proxy.runpod.net"
                api_key = "s3cret"
                mask = "glyph"
                prompt = "clean background"
                steps = 20
                guidance = 30.0
                timeout_secs = 120
            "#,
        )
        .unwrap();

        assert_eq!(config.endpoint, "https://abc-8000.proxy.runpod.net");
        assert_eq!(config.api_key, "s3cret");
        assert_eq!(config.mask, MaskMode::Glyph);
        assert_eq!(config.prompt.as_deref(), Some("clean background"));
        assert_eq!(config.steps, Some(20));
        assert_eq!(config.guidance, Some(30.0));
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn minimal_config_uses_defaults() {
        let config: RemoteInpaintConfig =
            toml::from_str(r#"endpoint = "http://127.0.0.1:8787""#).unwrap();

        assert_eq!(config.mask, MaskMode::Bubble);
        assert!(config.api_key.is_empty());
        assert!(config.prompt.is_none());
        assert!(config.steps.is_none());
        assert!(config.guidance.is_none());
        assert_eq!(config.timeout_secs, 600);
    }

    #[test]
    fn unknown_keys_are_rejected_as_typos() {
        let err = toml::from_str::<RemoteInpaintConfig>(
            r#"
                endpoint = "http://x"
                endpiont = "http://y"
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("endpiont"));
    }

    #[test]
    fn config_template_parses_and_is_unconfigured() {
        let config: RemoteInpaintConfig = toml::from_str(CONFIG_TEMPLATE).unwrap();
        assert!(config.endpoint.is_empty());
        assert_eq!(config.mask, MaskMode::Bubble);
        assert_eq!(config.timeout_secs, 600);
    }
}
