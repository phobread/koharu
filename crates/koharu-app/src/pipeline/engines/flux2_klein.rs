//! Flux.2 Klein inpainter. Uses the CTD segment mask to build a looser
//! text-region mask, then runs Flux.2 inpainting on the resulting crop.

use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use image::{DynamicImage, GrayImage, Luma};
use koharu_core::{ImageRole, MaskRole, Op, Region};
use koharu_ml::flux2_klein::{Flux2InpaintOptions, Flux2Klein};
use koharu_ml::inpainting::{
    UndetectedBlockFallback, mask::expand_mask_to_bubble_region_for_inpainting,
};

use crate::official::cleanup_base;
use crate::pipeline::artifacts::Artifact;
use crate::pipeline::engine::PipelineRunOptions;
use crate::pipeline::engine::{Engine, EngineCtx, EngineInfo};
use crate::pipeline::engines::support::{
    differs_under_mask, find_image_node, find_mask_node, glyph_erase_mask, grow_region,
    image_dimensions, mask_bounds, mask_intersection, mask_pieces_touching,
    restore_masked_from_source, restore_region_from_source, text_node_to_region, text_nodes,
    upsert_image_blob,
};

/// How far around a repair stroke the lettering it lies on is followed.
/// Flux2 has to see enough of the lettering to read it as text: on LOLExtras
/// 0026 (2026-11-12) the leftover came back with 64px in 3 of 5 strokes and
/// with 128px in 1 of 5, and with 256px in none. Further out only costs time.
const REDO_CONTEXT_PX: u32 = 256;
/// Flux2's region reaches this far past the erased lettering.
const REDO_REGION_MARGIN_PX: u32 = 32;

pub struct Model(Flux2Klein);

#[async_trait]
impl Engine for Model {
    async fn run(&self, ctx: EngineCtx<'_>) -> Result<Vec<Op>> {
        let (_, mask_ref) = find_mask_node(ctx.scene, ctx.page, MaskRole::Segment)
            .ok_or_else(|| anyhow!("no Segment mask on page"))?;
        let (_, bubble_ref) = find_mask_node(ctx.scene, ctx.page, MaskRole::Bubble)
            .ok_or_else(|| anyhow!("no Bubble mask on page"))?;
        let mask = ctx.blobs.load_image(&mask_ref)?;
        let bubble_mask = ctx.blobs.load_image(&bubble_ref)?;

        // The source, with an official release's onomatopoeia if the page
        // has one (`official`).
        let (source, pieces) = cleanup_base(ctx.scene, ctx.page, ctx.blobs)?;
        // The original page and, for a repair stroke, the segment mask before
        // it: what `glyph_erase_mask` keeps coloured decorations by.
        let original = source.clone();
        let previous_mask = ctx
            .options
            .previous_segment_mask
            .as_ref()
            .map(|blob| ctx.blobs.load_image(blob))
            .transpose()?;
        // Set when a repair stroke builds on the cleaned image: the original
        // page, to show Flux2 under the pixels the stroke repaints.
        let mut repaint_from_source = None;
        let (image, mask, bubble_mask) = match ctx.options.region {
            Some(r) => {
                let base = match find_image_node(ctx.scene, ctx.page, ImageRole::Inpainted) {
                    Some((_, blob)) => {
                        let inpainted = ctx.blobs.load_image(&blob)?;
                        if ctx.options.restore_source_region.unwrap_or(true) {
                            restore_region_from_source(&inpainted, &source, &r)
                        } else {
                            repaint_from_source = Some(source);
                            inpainted
                        }
                    }
                    None => source,
                };
                let clipped_mask = clip_mask_to_region(&mask, &r);
                let clipped_bubble = clip_mask_to_region(&bubble_mask, &r);
                (base, clipped_mask, clipped_bubble)
            }
            None => (source, mask, bubble_mask),
        };

        let text_blocks = text_nodes(ctx.scene, ctx.page)
            .into_iter()
            .map(|(_, transform, text)| text_node_to_region(transform, text))
            .collect::<Vec<_>>();
        let fallback = if ctx.options.region.is_some() {
            UndetectedBlockFallback::Skip
        } else {
            UndetectedBlockFallback::FillBubble
        };
        let generation_mask = expand_mask_to_bubble_region_for_inpainting(
            &mask,
            &bubble_mask,
            &text_blocks,
            fallback,
        );
        // Give FLUX the broad block-shaped context it needs to generate a
        // clean fill, but paste through the tighter glyph mask. Using the
        // broad mask for both stages leaves a flat, discoloured rectangle.
        let composite_mask = glyph_erase_mask(
            ctx.options,
            &original,
            previous_mask.as_ref(),
            &mask,
            &bubble_mask,
            &text_blocks,
        );
        let (generation_mask, composite_mask) = match ctx.options.region {
            Some(r) => (
                clip_gray_mask_to_region(&generation_mask, &r),
                clip_gray_mask_to_region(&composite_mask, &r),
            ),
            None => (generation_mask, composite_mask),
        };
        // Everything the stroke repaints is generated from the original page,
        // as in a full run, so an earlier fill that went wrong isn't copied.
        //
        // Where the stroke goes over pixels cleaned earlier, the original
        // comes back under the whole piece of lettering it lies on, and that
        // piece is cleaned again as a full run would clean it. Shown only
        // the part inside the stroke's rectangle, Flux2 does not read a
        // clipped scrap of hand-drawn lettering as text: it keeps it as an
        // object and recolours it. Only the rectangle is kept from the result.
        let (mut image, mut generation_mask, mut composite_mask, mut bubble_mask) =
            (image, generation_mask, composite_mask, bubble_mask);
        let mut cleaned_page = None;
        if let (Some(source), Some(r)) = (repaint_from_source, ctx.options.region) {
            let piece = if differs_under_mask(&image, &source, &composite_mask, &r) {
                let page_mask = ctx.blobs.load_image(&mask_ref)?;
                let page_bubbles = ctx.blobs.load_image(&bubble_ref)?;
                lettering_under_stroke(
                    ctx.options,
                    &source,
                    previous_mask.as_ref(),
                    &page_mask,
                    &page_bubbles,
                    &text_blocks,
                    &composite_mask,
                    &r,
                )
                .map(|piece| (piece, page_bubbles))
            } else {
                None
            };
            match piece {
                Some((piece, page_bubbles)) => {
                    let (width, height) = image_dimensions(&image);
                    let page = Region {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    };
                    let shown = restore_masked_from_source(&image, &source, &piece.erase, &page);
                    cleaned_page = Some((std::mem::replace(&mut image, shown), r));
                    generation_mask = piece.generation;
                    composite_mask = piece.erase;
                    bubble_mask = page_bubbles;
                    tracing::info!(
                        x = r.x,
                        y = r.y,
                        width = r.width,
                        height = r.height,
                        "Flux2 repair stroke over cleaned lettering: regenerating the whole piece"
                    );
                }
                None => image = restore_masked_from_source(&image, &source, &composite_mask, &r),
            }
        }
        let generation_mask = DynamicImage::ImageLuma8(generation_mask);
        let composite_mask = DynamicImage::ImageLuma8(composite_mask);
        let options = Flux2InpaintOptions {
            num_inference_steps: ctx.options.flux2_steps.unwrap_or(4).clamp(1, 20) as usize,
            strength: ctx.options.flux2_strength.unwrap_or(1.0).clamp(0.05, 1.0),
            flat_fill: ctx.options.flux2_flat_fill.unwrap_or(false),
            fast_large_crops: ctx.options.flux2_fast_large_crops.unwrap_or(false),
            ..Default::default()
        };
        let bubble_ids = bubble_mask.to_luma8();
        let result = self.0.inpaint_with_reference_and_composite_mask(
            &image,
            &generation_mask,
            &composite_mask,
            Some(&bubble_ids),
            None,
            &options,
        )?;
        // A stroke over cleaned pixels changes its own rectangle only.
        let result = match cleaned_page {
            Some((cleaned, r)) => restore_region_from_source(&cleaned, &result, &r),
            None => result,
        };
        let result = match &pieces {
            Some(pieces) => pieces.apply(&result),
            None => result,
        };
        let (w, h) = image_dimensions(&result);
        let store_started = Instant::now();
        let blob = ctx.blobs.put_webp(&result)?;
        tracing::info!(
            elapsed_ms = store_started.elapsed().as_millis(),
            width = w,
            height = h,
            "stored Flux2 inpainted image"
        );
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

/// What Flux2 regenerates for a repair stroke over cleaned lettering.
struct LetteringPiece {
    /// The Flux2 region: the connected pieces of the page's clean-up region
    /// the stroke erases in, within [`REDO_CONTEXT_PX`] of the stroke and
    /// [`REDO_REGION_MARGIN_PX`] of the lettering.
    generation: GrayImage,
    /// The pixels erased inside it.
    erase: GrayImage,
}

/// The lettering a repair stroke lies on. `stroke_erase` holds the pixels the
/// stroke erases (inside `stroke`, its rectangle); `page` is the original
/// page, `previous_mask` the segment mask before the stroke, and the masks
/// are the page's.
/// `None` when the stroke erases nothing in the page's clean-up region.
#[allow(clippy::too_many_arguments)]
fn lettering_under_stroke(
    options: &PipelineRunOptions,
    page: &DynamicImage,
    previous_mask: Option<&DynamicImage>,
    page_mask: &DynamicImage,
    page_bubbles: &DynamicImage,
    text_blocks: &[koharu_ml::types::TextRegion],
    stroke_erase: &GrayImage,
    stroke: &Region,
) -> Option<LetteringPiece> {
    let region = expand_mask_to_bubble_region_for_inpainting(
        page_mask,
        page_bubbles,
        text_blocks,
        UndetectedBlockFallback::Skip,
    );
    let (width, height) = region.dimensions();
    let window = grow_region(stroke, REDO_CONTEXT_PX, width, height);
    let piece = mask_pieces_touching(&region, stroke_erase, &window);
    let erase = glyph_erase_mask(
        options,
        page,
        previous_mask,
        page_mask,
        page_bubbles,
        text_blocks,
    );
    let erase = mask_intersection(&erase, &piece);
    // A text box can be far bigger than the lettering in it, and two boxes
    // that overlap make one piece. Flux2 needs the region around what is
    // erased, not all of it: the crop's size is what a stroke takes in time.
    let lettering = mask_bounds(&erase)?;
    let around = grow_region(&lettering, REDO_REGION_MARGIN_PX, width, height);
    let generation = clip_gray_mask_to_region(&piece, &around);
    Some(LetteringPiece { generation, erase })
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
        id: "flux2-klein",
        name: "Flux.2 Klein",
        needs: &[Artifact::SegmentMask, Artifact::BubbleMask],
        produces: &[Artifact::Inpainted],
        load: |runtime, _cpu| Box::pin(async move {
            let m = Flux2Klein::load(runtime).await?;
            Ok(Box::new(Model(m)) as Box<dyn Engine>)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page with one long hand-traced bar and a separate blob, no bubbles
    /// and no text boxes.
    fn traced_page() -> (DynamicImage, DynamicImage) {
        let mut mask = GrayImage::new(700, 100);
        for y in 40..46 {
            for x in 20..680 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        for y in 80..90 {
            for x in 30..50 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        (
            DynamicImage::ImageLuma8(mask),
            DynamicImage::ImageLuma8(GrayImage::new(700, 100)),
        )
    }

    fn blank_page(mask: &DynamicImage) -> DynamicImage {
        DynamicImage::new_rgb8(mask.width(), mask.height())
    }

    fn stroke_options(stroke: Region) -> PipelineRunOptions {
        PipelineRunOptions {
            region: Some(stroke),
            restore_source_region: Some(false),
            ..Default::default()
        }
    }

    #[test]
    fn a_stroke_on_cleaned_lettering_takes_the_whole_piece_near_it() {
        let (mask, bubbles) = traced_page();
        let stroke = Region {
            x: 100,
            y: 35,
            width: 20,
            height: 15,
        };
        let stroke_erase = clip_gray_mask_to_region(&mask.to_luma8(), &stroke);

        let piece = lettering_under_stroke(
            &stroke_options(stroke),
            &blank_page(&mask),
            None,
            &mask,
            &bubbles,
            &[],
            &stroke_erase,
            &stroke,
        )
        .expect("the stroke lies on the bar");

        // The bar is followed out of the stroke's rectangle, both ways...
        for x in [22, 99, 200, 370] {
            assert_eq!(piece.generation.get_pixel(x, 43)[0], 255, "x = {x}");
            assert_eq!(piece.erase.get_pixel(x, 43)[0], 255, "x = {x}");
        }
        // ...but not further than the context around the stroke,
        assert_eq!(piece.generation.get_pixel(400, 43)[0], 0);
        assert_eq!(piece.erase.get_pixel(400, 43)[0], 0);
        // and the other piece of the mask is left out.
        assert_eq!(piece.generation.get_pixel(40, 85)[0], 0);
        assert_eq!(piece.erase.get_pixel(40, 85)[0], 0);
        // A hand stroke is erased as drawn, grown by 2px.
        assert_eq!(piece.erase.get_pixel(200, 38)[0], 255);
        assert_eq!(piece.erase.get_pixel(200, 36)[0], 0);
        // Every erased pixel is one Flux2 regenerates.
        for (x, y, pixel) in piece.erase.enumerate_pixels() {
            assert!(pixel[0] == 0 || piece.generation.get_pixel(x, y)[0] > 0);
        }
    }

    #[test]
    fn a_big_text_box_is_cut_down_to_the_lettering_in_it() {
        // A small traced blob in a text box that is far bigger than it.
        let mut mask = GrayImage::new(600, 600);
        for y in 280..300 {
            for x in 280..300 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        let mask = DynamicImage::ImageLuma8(mask);
        let bubbles = DynamicImage::ImageLuma8(GrayImage::new(600, 600));
        let text_box = koharu_ml::types::TextRegion {
            x: 50.0,
            y: 50.0,
            width: 500.0,
            height: 500.0,
            ..Default::default()
        };
        let stroke = Region {
            x: 284,
            y: 284,
            width: 10,
            height: 10,
        };
        let stroke_erase = clip_gray_mask_to_region(&mask.to_luma8(), &stroke);

        let piece = lettering_under_stroke(
            &stroke_options(stroke),
            &blank_page(&mask),
            None,
            &mask,
            &bubbles,
            &[text_box],
            &stroke_erase,
            &stroke,
        )
        .expect("the stroke lies on the blob");

        // The whole blob is erased, not only the part under the stroke.
        assert_eq!(piece.erase.get_pixel(281, 298)[0], 255);
        // Flux2's region is the box around the lettering, not the whole box.
        assert_eq!(piece.generation.get_pixel(290, 290)[0], 255);
        assert_eq!(piece.generation.get_pixel(250, 290)[0], 255);
        assert_eq!(piece.generation.get_pixel(200, 290)[0], 0);
        assert_eq!(piece.generation.get_pixel(100, 100)[0], 0);
    }

    #[test]
    fn a_stroke_that_erases_nothing_has_no_piece() {
        let (mask, bubbles) = traced_page();
        let stroke = Region {
            x: 300,
            y: 60,
            width: 20,
            height: 10,
        };
        let stroke_erase = clip_gray_mask_to_region(&mask.to_luma8(), &stroke);
        assert!(
            lettering_under_stroke(
                &stroke_options(stroke),
                &blank_page(&mask),
                None,
                &mask,
                &bubbles,
                &[],
                &stroke_erase,
                &stroke,
            )
            .is_none()
        );
    }
}
