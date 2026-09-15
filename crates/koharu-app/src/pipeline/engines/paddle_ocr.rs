//! Hybrid OCR: PaddleOCR-VL for complete block transcription, followed by
//! Korean PP-OCRv5 line recognition for confidence-gated Hangul repair.
//!
//! Each text node on the page is cropped out of the source image, passed
//! through the multimodal model, and the recognised text is written back
//! via `UpdateNode { TextDataPatch { text } }`.

use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use image::DynamicImage;
use koharu_core::{NodeDataPatch, NodePatch, Op, TextDataPatch};
use koharu_llm::paddleocr_vl::{PaddleOcrVl, PaddleOcrVlGenerateOptions, PaddleOcrVlTask};
use koharu_ml::{
    TextRegion,
    comic_text_detector::crop_text_block_deskewed,
    korean_ocr::{KoreanOcr, contains_lexical_hangul, repair_hangul},
};
use koharu_runtime::RuntimeManager;
use tokio::sync::OnceCell;

use crate::app::shared_llama_backend;
use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::{
    load_source_image, single_line_ocr_text, text_node_to_region, text_nodes,
};

const MAX_NEW_TOKENS: usize = 256;
const KOREAN_REPAIR_MIN_CONFIDENCE: f32 = 0.90;

pub struct Model {
    paddle: Mutex<PaddleOcrVl>,
    runtime: RuntimeManager,
    korean: OnceCell<Mutex<KoreanOcr>>,
}

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        let texts = text_nodes(ctx.scene, ctx.page);
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let image = load_source_image(ctx.scene, ctx.page, ctx.blobs)?;
        let regions: Vec<_> = texts
            .iter()
            .map(|(_, transform, text)| {
                let region = text_node_to_region(transform, text);
                crop_text_block_deskewed(&image, &region)
            })
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
            .map(|(_, transform, text)| {
                let region = text_node_to_region(transform, text);
                korean_verification_crop(&image, &region)
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
            match self
                .korean
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
                    let lines = korean.recognize_block(&verification_regions[index])?;
                    Ok(repair_hangul(&text, &lines, KOREAN_REPAIR_MIN_CONFIDENCE))
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

fn korean_verification_crop(image: &DynamicImage, region: &TextRegion) -> DynamicImage {
    let mut tight = region.clone();
    let pad = (tight.width.min(tight.height) * 0.03).max(2.0);
    tight.x -= pad;
    tight.y -= pad;
    tight.width += pad * 2.0;
    tight.height += pad * 2.0;
    // The verifier's line projection needs the actual detector rectangle,
    // not CTD's broader line-polygon union and OCR margin.
    tight.detector = None;
    tight.line_polygons = None;
    crop_text_block_deskewed(image, &tight)
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
                korean: OnceCell::new(),
            }) as Box<dyn Engine>)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::is_korean_language;
    use crate::pipeline::engine::Registry;

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
