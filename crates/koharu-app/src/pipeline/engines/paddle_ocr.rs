//! Hybrid OCR: PaddleOCR-VL for complete block transcription, followed by
//! Korean PP-OCRv5 line recognition for confidence-gated Hangul repair.
//!
//! Each text node on the page is cropped out of the source image, passed
//! through the multimodal model, and the recognised text is written back
//! via `UpdateNode { TextDataPatch { text } }`.

use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use image::{DynamicImage, RgbImage};
use koharu_core::{NodeDataPatch, NodePatch, Op, TextDataPatch};
use koharu_llm::paddleocr_vl::{PaddleOcrVl, PaddleOcrVlGenerateOptions, PaddleOcrVlTask};
use koharu_ml::{
    TextRegion,
    comic_text_detector::{crop_text_block_deskewed, crop_text_block_exact},
    korean_ocr::{
        KoreanOcr, contains_lexical_hangul, is_dark_panel, repair_hangul, space_at_line_breaks,
    },
    outlined_text::{clean_outlined_text, outline_window},
    quad_bbox, rotated_box_corners,
};
use koharu_runtime::RuntimeManager;
use tokio::sync::OnceCell;

use crate::app::shared_llama_backend;
use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::{
    is_degenerate_ocr_text, load_source_image, requested_text_nodes, single_line_ocr_text,
    text_node_to_region,
};

const MAX_NEW_TOKENS: usize = 256;
// Per-line trust bar for `repair_hangul` (a line's Hangul-only mean) and for
// the dark-bubble inverted-polarity fallback. On BadEnd 017 the dedicated
// recognizer's *correct* outlined-Hangul lines sit at ~0.77-0.99 while its
// clearly-wrong lines scored <=0.48, so 0.77 admits the good lines (incl.
// block 3's `변태`@0.771) and rejects the bad ones; alignment + never-indel in
// `repair_hangul` guards the rest. Re-validated 2026-09-27 on 9 unseen BadEnd
// pages (40 bubbles, blind key, rules frozen before the test run) together
// with the sturdier line splitter: Hangul errors 50 -> 32.
const KOREAN_REPAIR_MIN_CONFIDENCE: f32 = 0.77;

/// The PP-OCRv5 verifier lives for the whole process, not with the engine.
/// It runs on the CPU, so unloading it would free no GPU memory, and the
/// registry now unloads this engine before every Flux2 run; keeping the
/// verifier avoids rebuilding its ONNX Runtime session on every page.
static KOREAN_OCR: OnceCell<Mutex<KoreanOcr>> = OnceCell::const_new();

pub struct Model {
    paddle: Mutex<PaddleOcrVl>,
    runtime: RuntimeManager,
}

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        let texts = requested_text_nodes(ctx.scene, ctx.page, ctx.options);
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let image = load_source_image(ctx.scene, ctx.page, ctx.blobs)?;
        // Each box is read from its own view of the page: outlined lettering
        // cleaned into black-on-white (see `outlined_text`), else the page.
        let page_rgb = image.to_rgb8();
        let sources: Vec<_> = texts
            .iter()
            .map(|(_, transform, text)| {
                let region = text_node_to_region(transform, text);
                outline_cleaned_source(&page_rgb, &region)
                    .map_or((None, region), |(view, moved)| (Some(view), moved))
            })
            .collect();
        let cleaned = sources.iter().filter(|(view, _)| view.is_some()).count();
        if cleaned > 0 {
            tracing::info!(
                cleaned,
                total = sources.len(),
                "reading outlined lettering from cleaned crops"
            );
        }
        let regions: Vec<_> = texts
            .iter()
            .zip(&sources)
            .map(|((_, transform, text), (view, moved))| match view {
                Some(view) => crop_text_block_deskewed(view, moved),
                _ => crop_text_block_deskewed(&image, &text_node_to_region(transform, text)),
            })
            .collect();
        // The verifier's inverted-polarity retry follows the crop it reads: a
        // cleaned crop is black-on-white; otherwise the page crop decides, as
        // before.
        let dark_panels: Vec<bool> = sources
            .iter()
            .zip(&regions)
            .map(|((view, _), region)| view.is_none() && is_dark_panel(region))
            .collect();

        let options = PaddleOcrVlGenerateOptions {
            max_new_tokens: MAX_NEW_TOKENS,
            language: ctx.options.source_language.clone(),
            ..PaddleOcrVlGenerateOptions::default()
        };
        let outputs = {
            let mut ocr = self
                .paddle
                .lock()
                .map_err(|_| anyhow::anyhow!("PaddleOCR mutex poisoned"))?;
            let mut outputs =
                ocr.inference_images_with_options(&regions, PaddleOcrVlTask::Ocr, &options)?;
            // The language hint steers script choice but is off the model's
            // training prompt, and on some crops it derails generation into
            // nothing at all. Any block that comes back empty gets one retry
            // with the plain auto-detect prompt so a hint can only ever add
            // accuracy, never lose text.
            if options.language.is_some() {
                let fallback = PaddleOcrVlGenerateOptions {
                    max_new_tokens: MAX_NEW_TOKENS,
                    ..PaddleOcrVlGenerateOptions::default()
                };
                for (region, out) in regions.iter().zip(outputs.iter_mut()) {
                    if out.text.trim().is_empty() {
                        *out =
                            ocr.inference_with_options(region, PaddleOcrVlTask::Ocr, &fallback)?;
                    }
                }
            }
            outputs
        };

        let korean_hint = options.language.as_deref().is_some_and(is_korean_language);
        let verification_regions = texts
            .iter()
            .zip(&sources)
            .map(|((_, transform, text), (view, moved))| match view {
                Some(view) => korean_verification_crop(view, moved),
                _ => korean_verification_crop(&image, &text_node_to_region(transform, text)),
            })
            .collect::<Vec<_>>();
        // Once a page is explicitly Korean or any block is recognized as
        // Hangul, run both recognizers on every block. This lets PP-OCRv5
        // recover a block that PaddleOCR-VL misclassified as another script.
        let use_hybrid = korean_hint
            || outputs
                .iter()
                .any(|output| contains_lexical_hangul(&output.text));

        let korean = if use_hybrid {
            match KOREAN_OCR
                .get_or_try_init(|| async {
                    Ok::<_, anyhow::Error>(Mutex::new(KoreanOcr::load(&self.runtime).await?))
                })
                .await
            {
                Ok(model) => Some(model),
                Err(error) => {
                    tracing::warn!(%error, "Korean OCR verifier unavailable; keeping PaddleOCR-VL output");
                    None
                }
            }
        } else {
            None
        };

        let mut ops = Vec::with_capacity(texts.len());
        for (index, ((node_id, _, _), out)) in texts.iter().zip(outputs).enumerate() {
            let mut text = single_line_ocr_text(&out.text);
            if use_hybrid && let Some(korean) = korean {
                let repaired = (|| -> Result<String> {
                    let mut korean = korean
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Korean OCR mutex poisoned"))?;
                    let lines = korean.recognize_block_with_fallback(
                        &verification_regions[index],
                        dark_panels[index],
                        KOREAN_REPAIR_MIN_CONFIDENCE,
                    )?;
                    let repaired = repair_hangul(&text, &lines, KOREAN_REPAIR_MIN_CONFIDENCE);
                    Ok(space_at_line_breaks(&repaired, &lines))
                })();
                match repaired {
                    Ok(repaired) if repaired != text => {
                        tracing::info!(before = %text, after = %repaired, "repaired Korean OCR with PP-OCRv5");
                        text = repaired;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error, "Korean OCR verification failed; keeping PaddleOCR-VL output");
                    }
                }
            }
            // Emoji-only output (no letters) means the recognizer hallucinated on
            // a glyph it could not read; blank it rather than pass garbage to the
            // translator.
            if is_degenerate_ocr_text(&text) {
                tracing::info!(discarded = %text, "blanking degenerate emoji-only OCR output");
                text.clear();
            }
            ops.push(Op::UpdateNode {
                page: ctx.page,
                id: *node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        text: Some(Some(text)),
                        ..Default::default()
                    })),
                    transform: None,
                    visible: None,
                },
                prev: NodePatch::default(),
            });
        }
        Ok(ops)
    }
}

fn is_korean_language(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "korean" | "ko" | "kor" | "ko-kr" | "ko_kr"
    )
}

/// When `region` holds thick-outlined lettering, a small view of the page
/// around it with the lettering redrawn black-on-white, and `region` moved
/// into that view's coordinates. The view extends well past the cleaned
/// window so the readers' own crop margins and deskew see the same pixels
/// they would on the page; other boxes are unaffected.
fn outline_cleaned_source(
    page: &RgbImage,
    region: &TextRegion,
) -> Option<(DynamicImage, TextRegion)> {
    let (page_width, page_height) = page.dimensions();
    let [min_x, min_y, max_x, max_y] = region_bounds(region);
    let (window, rect) = outline_window(page_width, page_height, min_x, min_y, max_x, max_y);
    let [wx0, wy0, wx1, wy1] = window;
    if wx1 <= wx0 || wy1 <= wy0 {
        return None;
    }
    let crop = image::imageops::crop_imm(page, wx0, wy0, wx1 - wx0, wy1 - wy0).to_image();
    let cleaned = clean_outlined_text(
        &crop,
        [rect[0] - wx0, rect[1] - wy0, rect[2] - wx0, rect[3] - wy0],
    )?;

    let margin = (wx1 - wx0).max(wy1 - wy0) / 2 + 32;
    let vx0 = wx0.saturating_sub(margin);
    let vy0 = wy0.saturating_sub(margin);
    let vx1 = (wx1 + margin).min(page_width);
    let vy1 = (wy1 + margin).min(page_height);
    let mut view = image::imageops::crop_imm(page, vx0, vy0, vx1 - vx0, vy1 - vy0).to_image();
    image::imageops::replace(
        &mut view,
        &cleaned,
        i64::from(wx0 - vx0),
        i64::from(wy0 - vy0),
    );
    let (dx, dy) = (vx0 as f32, vy0 as f32);
    let mut moved = region.clone();
    moved.x -= dx;
    moved.y -= dy;
    for point in moved.line_polygons.iter_mut().flatten().flatten() {
        point[0] -= dx;
        point[1] -= dy;
    }
    Some((DynamicImage::ImageRgb8(view), moved))
}

/// Axis-aligned bounds `[min_x, min_y, max_x, max_y]` of a (possibly
/// rotated) region.
fn region_bounds(region: &TextRegion) -> [f32; 4] {
    let angle = region.rotation_deg.unwrap_or(0.0);
    if !angle.is_finite() || angle.abs() < 0.05 {
        return [
            region.x,
            region.y,
            region.x + region.width,
            region.y + region.height,
        ];
    }
    quad_bbox(&rotated_box_corners(
        [
            region.x + region.width * 0.5,
            region.y + region.height * 0.5,
        ],
        [region.width, region.height],
        angle,
    ))
}

fn korean_verification_crop(image: &DynamicImage, region: &TextRegion) -> DynamicImage {
    let mut tight = region.clone();
    let pad = (tight.width.min(tight.height) * 0.03).max(2.0);
    tight.x -= pad;
    tight.y -= pad;
    tight.width += pad * 2.0;
    tight.height += pad * 2.0;
    // The verifier's line projection needs the actual detector rectangle with
    // only the 3% margin added above. `crop_text_block_exact` adds no further
    // OCR margin: the generic margin is proportional to the whole block and is
    // large enough to pull the bright speech-balloon border into the crop,
    // where `split_text_lines` mistakes it for glyph rows.
    crop_text_block_exact(image, &tight)
}

inventory::submit! {
    EngineInfo {
        id: "paddle-ocr-vl-1.6",
        name: "Hybrid",
        needs: &[Artifact::TextBoxes],
        produces: &[Artifact::OcrText],
        load: |runtime, cpu| Box::pin(async move {
            let backend = shared_llama_backend(runtime)?;
            let m = PaddleOcrVl::load(runtime, cpu, backend).await?;
            Ok(Box::new(Model {
                paddle: Mutex::new(m),
                runtime: runtime.clone(),
            }) as Box<dyn Engine>)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_korean_language, outline_cleaned_source, region_bounds};
    use crate::pipeline::engine::Registry;
    use image::{Rgb, RgbImage};
    use koharu_ml::TextRegion;

    fn region(x: f32, y: f32, width: f32, height: f32) -> TextRegion {
        TextRegion {
            x,
            y,
            width,
            height,
            confidence: 1.0,
            line_polygons: None,
            source_direction: None,
            rotation_deg: None,
            detected_font_size_px: None,
            detector: None,
        }
    }

    #[test]
    fn rotated_region_bounds_cover_the_turned_box() {
        let mut r = region(100.0, 100.0, 40.0, 20.0);
        assert_eq!(region_bounds(&r), [100.0, 100.0, 140.0, 120.0]);
        r.rotation_deg = Some(90.0);
        let [x0, y0, x1, y1] = region_bounds(&r);
        assert!((x0 - 110.0).abs() < 1e-3 && (x1 - 130.0).abs() < 1e-3);
        assert!((y0 - 90.0).abs() < 1e-3 && (y1 - 130.0).abs() < 1e-3);
    }

    #[test]
    fn a_cleaned_view_moves_the_box_and_its_line_polygons_with_it() {
        // Purple bar with a thick white outline on grey artwork.
        let page = RgbImage::from_fn(400, 300, |x, y| {
            if (180..220).contains(&x) && (140..160).contains(&y) {
                Rgb([110, 90, 220])
            } else if (176..224).contains(&x) && (136..164).contains(&y) {
                Rgb([255, 255, 255])
            } else {
                Rgb([120, 120, 120])
            }
        });
        let mut r = region(170.0, 130.0, 60.0, 40.0);
        r.line_polygons = Some(vec![[
            [180.0, 140.0],
            [220.0, 140.0],
            [220.0, 160.0],
            [180.0, 160.0],
        ]]);
        let (view, moved) = outline_cleaned_source(&page, &r).expect("outlined lettering");
        let (dx, dy) = (r.x - moved.x, r.y - moved.y);
        assert!(dx > 0.0 && dy > 0.0);
        assert_eq!(moved.line_polygons.unwrap()[0][0], [180.0 - dx, 140.0 - dy]);
        let view = view.to_rgb8();
        let at = |x: f32, y: f32| *view.get_pixel((x - dx) as u32, (y - dy) as u32);
        assert_eq!(at(200.0, 150.0), Rgb([0, 0, 0]), "fill is black");
        assert_eq!(at(178.0, 138.0), Rgb([255, 255, 255]), "outline is white");

        let bubble = RgbImage::from_pixel(400, 300, Rgb([255, 255, 255]));
        assert!(
            outline_cleaned_source(&bubble, &r).is_none(),
            "plain white left alone"
        );
    }

    #[test]
    fn recognizes_korean_language_hints() {
        for language in ["Korean", "ko", "kor", "ko-KR", "ko_kr"] {
            assert!(is_korean_language(language));
        }
        assert!(!is_korean_language("Japanese"));
    }

    #[test]
    fn default_ocr_engine_is_catalogued_as_hybrid() {
        let engine = Registry::find("paddle-ocr-vl-1.6").unwrap();
        assert_eq!(engine.name, "Hybrid");
    }
}
