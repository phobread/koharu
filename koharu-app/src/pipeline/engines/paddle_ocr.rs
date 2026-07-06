//! PaddleOCR-VL. Vision-language OCR driven by llama.cpp + mtmd.
//!
//! Each text node on the page is cropped out of the source image, passed
//! through the multimodal model, and the recognised text is written back
//! via `UpdateNode { TextDataPatch { text } }`.

use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use koharu_core::{NodeDataPatch, NodePatch, Op, TextDataPatch};
use koharu_llm::paddleocr_vl::{PaddleOcrVl, PaddleOcrVlGenerateOptions, PaddleOcrVlTask};
use koharu_ml::comic_text_detector::crop_text_block_deskewed;

use crate::app::shared_llama_backend;
use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::{
    load_source_image, single_line_ocr_text, text_node_to_region, text_nodes,
};

const MAX_NEW_TOKENS: usize = 256;

pub struct Model(Mutex<PaddleOcrVl>);

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
                .0
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

        let mut ops = Vec::with_capacity(texts.len());
        for ((node_id, _, _), out) in texts.iter().zip(outputs) {
            ops.push(Op::UpdateNode {
                page: ctx.page,
                id: *node_id,
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(TextDataPatch {
                        text: Some(Some(single_line_ocr_text(&out.text))),
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

inventory::submit! {
    EngineInfo {
        id: "paddle-ocr-vl-1.6",
        name: "PaddleOCR-VL",
        needs: &[Artifact::TextBoxes],
        produces: &[Artifact::OcrText],
        load: |runtime, cpu| Box::pin(async move {
            let backend = shared_llama_backend(runtime)?;
            let m = PaddleOcrVl::load(runtime, cpu, backend).await?;
            Ok(Box::new(Model(Mutex::new(m))) as Box<dyn Engine>)
        }),
    }
}
