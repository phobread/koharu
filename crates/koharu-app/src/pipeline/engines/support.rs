//! Shared helpers used by multiple engine implementations.
//!
//! The patterns here map `koharu-ml` / `koharu-llm` outputs (plain
//! `TextRegion`s, `DynamicImage`s) into `Op` sequences that mutate the scene.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, GrayImage};
use koharu_core::{
    BlobRef, ImageData, ImageRole, MaskData, MaskRole, Node, NodeDataPatch, NodeId, NodeKind, Op,
    PageId, ReadingOrder, Region, Scene, TextData, Transform,
};

use crate::blobs::BlobStore;
use crate::pipeline::engine::PipelineRunOptions;

// ---------------------------------------------------------------------------
// Read helpers
// ---------------------------------------------------------------------------

/// Find the Source image node on `page`. Returns `(node_id, image_data)`.
/// Every valid page has exactly one; absence means the page is malformed.
pub fn source_node(scene: &Scene, page: PageId) -> Result<(NodeId, &ImageData)> {
    let page = scene
        .page(page)
        .with_context(|| format!("page {} not found", page))?;
    for (id, node) in page.nodes.iter() {
        if let NodeKind::Image(img) = &node.kind
            && img.role == ImageRole::Source
        {
            return Ok((*id, img));
        }
    }
    anyhow::bail!("page has no Source image node")
}

/// Load the source image bytes + decoded image for `page`.
pub fn load_source_image(scene: &Scene, page: PageId, blobs: &BlobStore) -> Result<DynamicImage> {
    let (_, img_data) = source_node(scene, page)?;
    blobs.load_image(&img_data.blob)
}

/// Find a node of `Image { role }` on `page`, if any.
pub fn find_image_node(scene: &Scene, page: PageId, role: ImageRole) -> Option<(NodeId, BlobRef)> {
    let page = scene.page(page)?;
    page.nodes.iter().find_map(|(id, node)| match &node.kind {
        NodeKind::Image(img) if img.role == role => Some((*id, img.blob.clone())),
        _ => None,
    })
}

/// Find a node of `Mask { role }` on `page`, if any.
pub fn find_mask_node(scene: &Scene, page: PageId, role: MaskRole) -> Option<(NodeId, BlobRef)> {
    let page = scene.page(page)?;
    page.nodes.iter().find_map(|(id, node)| match &node.kind {
        NodeKind::Mask(mask) if mask.role == role => Some((*id, mask.blob.clone())),
        _ => None,
    })
}

/// Collect `(NodeId, &Transform, &TextData)` for every text node on `page`,
/// in stacking order.
pub fn text_nodes(scene: &Scene, page: PageId) -> Vec<(NodeId, &Transform, &TextData)> {
    let Some(page) = scene.page(page) else {
        return Vec::new();
    };
    page.nodes
        .iter()
        .filter_map(|(id, node)| match &node.kind {
            NodeKind::Text(t) => Some((*id, &node.transform, t)),
            _ => None,
        })
        .collect()
}

/// [`text_nodes`] narrowed to `options.text_node_ids` when the run names
/// specific boxes.
pub fn requested_text_nodes<'a>(
    scene: &'a Scene,
    page: PageId,
    options: &crate::pipeline::PipelineRunOptions,
) -> Vec<(NodeId, &'a Transform, &'a TextData)> {
    let mut texts = text_nodes(scene, page);
    if let Some(ids) = options.text_node_ids.as_deref() {
        texts.retain(|(id, _, _)| ids.contains(id));
    }
    texts
}

/// Convert a scene `(Transform, TextData)` pair into a `koharu-ml` `TextRegion`
/// for passing back through detector helpers that need geometry + language
/// hints (e.g. CTD's `refine_segmentation_mask`, OCR's `extract_text_block_regions`).
/// The pixels a clean-up run erases: the segment mask, grown to take in the
/// edges of the lettering.
///
/// A repair-brush stroke that adds to the mask (`restore_source_region` is
/// `Some(false)`) is erased as drawn, grown by the minimum only. The usual
/// growth follows the size of the text box or of the stroke, up to 8px, and
/// that reaches a heart drawn beside a traced SFX.
///
/// Coloured decorations, such as pink hearts, stay out of the mask where the
/// lettering beside them is dark (`keep_coloured_decorations`); `page` is
/// the original page (the source, not the cleaned image). A brush stroke
/// still erases the decorations it paints over: the pixels it added to the
/// mask are told apart with `previous_mask`, the segment mask before it.
/// Without that mask a brush stroke erases everything it covers, as before.
pub fn glyph_erase_mask(
    options: &PipelineRunOptions,
    page: &DynamicImage,
    previous_mask: Option<&DynamicImage>,
    mask: &DynamicImage,
    bubble_mask: &DynamicImage,
    text_blocks: &[koharu_ml::types::TextRegion],
) -> GrayImage {
    use koharu_ml::inpainting::{
        UndetectedBlockFallback, expand_hand_stroke_mask_for_inpainting,
        expand_mask_for_inpainting, keep_coloured_decorations, keep_coloured_decorations_except,
    };
    match (options.region, options.restore_source_region) {
        (None, _) => keep_coloured_decorations(
            &expand_mask_for_inpainting(
                mask,
                bubble_mask,
                text_blocks,
                UndetectedBlockFallback::FillBubble,
            ),
            page,
            text_blocks,
        ),
        (Some(_), Some(false)) => {
            let erase = expand_hand_stroke_mask_for_inpainting(mask, bubble_mask, text_blocks);
            match previous_mask {
                Some(previous) => keep_coloured_decorations_except(
                    &erase,
                    page,
                    text_blocks,
                    &painted_pixels(mask, previous),
                ),
                None => erase,
            }
        }
        (Some(_), _) => keep_coloured_decorations(
            &expand_mask_for_inpainting(
                mask,
                bubble_mask,
                text_blocks,
                UndetectedBlockFallback::Skip,
            ),
            page,
            text_blocks,
        ),
    }
}

/// The pixels set in `mask` that were not set in `previous`: what a brush
/// stroke painted. Empty when the sizes differ.
pub fn painted_pixels(mask: &DynamicImage, previous: &DynamicImage) -> GrayImage {
    let mask = mask.to_luma8();
    let previous = previous.to_luma8();
    if mask.dimensions() != previous.dimensions() {
        return GrayImage::new(mask.width(), mask.height());
    }
    GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        let painted = mask.get_pixel(x, y).0[0] > 127 && previous.get_pixel(x, y).0[0] <= 127;
        image::Luma([if painted { 255 } else { 0 }])
    })
}

pub fn text_node_to_region(transform: &Transform, text: &TextData) -> koharu_ml::types::TextRegion {
    koharu_ml::types::TextRegion {
        x: transform.x,
        y: transform.y,
        width: transform.width,
        height: transform.height,
        confidence: text.confidence,
        line_polygons: text.line_polygons.clone(),
        source_direction: text.source_direction.map(core_text_direction_to_ml),
        // The node transform is the live truth for the block's angle:
        // detectors seed it and the slant editor mutates it, while
        // `TextData::rotation_deg` only records what a detector last
        // reported. Preferring the transform lets a manual slant (or
        // straighten) drive OCR deskewing on a re-run.
        rotation_deg: (transform.rotation_deg.abs() > 0.05).then_some(transform.rotation_deg),
        detected_font_size_px: text.detected_font_size_px,
        detector: text.detector.clone(),
    }
}

/// Inverse of `ml_text_direction_to_core`.
pub fn core_text_direction_to_ml(d: koharu_core::TextDirection) -> koharu_ml::types::TextDirection {
    match d {
        koharu_core::TextDirection::Horizontal => koharu_ml::types::TextDirection::Horizontal,
        koharu_core::TextDirection::Vertical => koharu_ml::types::TextDirection::Vertical,
    }
}

// ---------------------------------------------------------------------------
// Op constructors
// ---------------------------------------------------------------------------

/// Build an `AddNode` for a new `Image { role }` layer.
#[allow(clippy::too_many_arguments)]
pub fn add_image_node_op(
    page: PageId,
    role: ImageRole,
    blob: BlobRef,
    natural_width: u32,
    natural_height: u32,
    transform: Transform,
    visible: bool,
    at: usize,
) -> Op {
    let node = Node {
        id: NodeId::new(),
        transform,
        visible,
        kind: NodeKind::Image(ImageData {
            role,
            blob,
            opacity: 1.0,
            natural_width,
            natural_height,
            name: None,
        }),
    };
    Op::AddNode { page, node, at }
}

/// Build an `AddNode` for a new `Mask { role }` layer.
pub fn add_mask_node_op(
    page: PageId,
    role: MaskRole,
    blob: BlobRef,
    transform: Transform,
    visible: bool,
    at: usize,
) -> Op {
    let node = Node {
        id: NodeId::new(),
        transform,
        visible,
        kind: NodeKind::Mask(MaskData { role, blob }),
    };
    Op::AddNode { page, node, at }
}

/// Replace or add an `Image { role }` blob for `page`. If a node already
/// exists with that role, emits an `UpdateNode` with `ImageDataPatch`.
/// Otherwise emits `AddNode` at the top of the stack (renderer role) or
/// after Source (inpainted/custom role).
pub fn upsert_image_blob(
    scene: &Scene,
    page: PageId,
    role: ImageRole,
    blob: BlobRef,
    natural_width: u32,
    natural_height: u32,
) -> Op {
    if let Some((node_id, _)) = find_image_node(scene, page, role) {
        Op::UpdateNode {
            page,
            id: node_id,
            patch: koharu_core::NodePatch {
                data: Some(NodeDataPatch::Image(koharu_core::ImageDataPatch {
                    blob: Some(blob),
                    opacity: None,
                    name: None,
                    natural_width: Some(natural_width),
                    natural_height: Some(natural_height),
                })),
                transform: None,
                visible: None,
            },
            prev: koharu_core::NodePatch::default(),
        }
    } else {
        let at = {
            let page_ref = scene.page(page);
            let base = page_ref.map(|p| p.nodes.len()).unwrap_or(0);
            match role {
                // Rendered on top.
                ImageRole::Rendered => base,
                // Inpainted directly after source (index 1 if source is present).
                ImageRole::Inpainted => 1.min(base),
                // Custom / Source → append.
                _ => base,
            }
        };
        add_image_node_op(
            page,
            role,
            blob,
            natural_width,
            natural_height,
            Transform::default(),
            role != ImageRole::Rendered, // hide Rendered by default; make a toggle explicit
            at,
        )
    }
}

/// Replace or add a `Mask { role }` blob for `page`.
pub fn upsert_mask_blob(scene: &Scene, page: PageId, role: MaskRole, blob: BlobRef) -> Op {
    if let Some((node_id, _)) = find_mask_node(scene, page, role) {
        Op::UpdateNode {
            page,
            id: node_id,
            patch: koharu_core::NodePatch {
                data: Some(NodeDataPatch::Mask(koharu_core::MaskDataPatch {
                    blob: Some(blob),
                })),
                transform: None,
                visible: None,
            },
            prev: koharu_core::NodePatch::default(),
        }
    } else {
        let at = scene.page(page).map(|p| p.nodes.len()).unwrap_or(0);
        let visible = matches!(role, MaskRole::BrushInpaint);
        add_mask_node_op(page, role, blob, Transform::default(), visible, at)
    }
}

/// Build a `Node` ready to be added for a new Text region.
pub fn new_text_node(bbox: [f32; 4], text_data: TextData) -> Node {
    Node {
        id: NodeId::new(),
        transform: Transform {
            x: bbox[0],
            y: bbox[1],
            width: bbox[2] - bbox[0],
            height: bbox[3] - bbox[1],
            rotation_deg: text_data.rotation_deg.unwrap_or(0.0),
        },
        visible: true,
        kind: NodeKind::Text(text_data),
    }
}

/// Small helper: decoded image dimensions.
pub fn image_dimensions(image: &DynamicImage) -> (u32, u32) {
    image.dimensions()
}

/// Collapse a multi-line OCR result into one line. OCR line breaks mirror
/// the bubble's layout, not sentence structure — the renderer re-wraps to
/// the box anyway, and single-line text is easier to proofread and edit.
///
/// Japanese and Chinese run words together with no inter-word spaces, so
/// their lines join bare. Korean, Latin, Cyrillic and the like DO separate
/// words with spaces — joining those bare would fuse the last word of one
/// line with the first word of the next, so they join with a space. A hyphen
/// between two Latin letters at a line break is treated as a soft hyphen and
/// dropped; anywhere else (e.g. Korean's long-vowel dash in `레이이-`) it is
/// real text and kept.
pub fn single_line_ocr_text(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    // Korean shares the CJK code blocks but spaces its words, so a page with
    // any Hangul is treated as space-separated even if it also carries Hanja.
    let space_less = !text.chars().any(is_hangul) && text.chars().any(is_han_or_kana);
    let mut out = String::with_capacity(text.len());
    for line in lines {
        if out.is_empty() {
            out.push_str(line);
        } else if space_less {
            out.push_str(line);
        } else if is_soft_hyphen_break(&out, line) {
            out.pop();
            out.push_str(line);
        } else {
            out.push(' ');
            out.push_str(line);
        }
    }
    out
}

/// `before` ends in `-` that splits one Latin-script word across a line break.
fn is_soft_hyphen_break(before: &str, next: &str) -> bool {
    let mut tail = before.chars().rev();
    tail.next() == Some('-')
        && tail.next().is_some_and(is_latin_letter)
        && next.chars().next().is_some_and(is_latin_letter)
}

fn is_latin_letter(c: char) -> bool {
    c.is_alphabetic() && !is_hangul(c) && !is_han_or_kana(c)
}

/// True when OCR output for a text region carries an emoji/pictograph but no
/// actual letters — the signature of PaddleOCR-VL hallucinating (e.g. `🌞.💚`)
/// on a glyph it cannot read. Such a block is blanked rather than fed
/// downstream to the translator. Real hearts (`♡` U+2661, `♥` U+2665) are BMP
/// symbols below the emoji planes, so decorative bubbles are untouched, and any
/// block that still contains a letter is kept as-is.
pub fn is_degenerate_ocr_text(text: &str) -> bool {
    let has_emoji = text.chars().any(|c| c as u32 >= 0x1_F000);
    let has_letter = text.chars().any(char::is_alphabetic);
    has_emoji && !has_letter
}

/// Hangul: Korean writes with spaces between words.
fn is_hangul(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF     // hangul jamo
        | 0x3130..=0x318F   // hangul compatibility jamo
        | 0xA960..=0xA97F   // hangul jamo extended-A
        | 0xAC00..=0xD7AF   // hangul syllables + extended-B
    )
}

/// Han ideographs or Japanese kana: scripts that run words together with no
/// inter-word spaces.
fn is_han_or_kana(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF     // hiragana + katakana
        | 0x3400..=0x4DBF   // CJK extension A
        | 0x4E00..=0x9FFF   // CJK unified ideographs
        | 0xF900..=0xFAFF   // CJK compatibility ideographs
        | 0xFF66..=0xFF9F   // halfwidth katakana
    )
}

/// Base image for a regional re-inpaint: `base` (the existing inpainted
/// page) with `region` reverted to the `source` pixels. A regional run must
/// recompute its area from scratch — the model only paints where the mask is
/// white, so compositing onto the stale inpainted image would keep the old
/// fill forever wherever mask pixels were *cleared* (un-inpaint, mask
/// eraser) instead of bringing the original art back.
pub fn restore_region_from_source(
    base: &DynamicImage,
    source: &DynamicImage,
    region: &Region,
) -> DynamicImage {
    let mut out = base.to_rgba8();
    let src = source.to_rgba8();
    let (w, h) = out.dimensions();
    let (sw, sh) = src.dimensions();
    let x0 = region.x.min(w).min(sw);
    let y0 = region.y.min(h).min(sh);
    let x1 = region.x.saturating_add(region.width).min(w).min(sw);
    let y1 = region.y.saturating_add(region.height).min(h).min(sh);
    for y in y0..y1 {
        for x in x0..x1 {
            out.put_pixel(x, y, *src.get_pixel(x, y));
        }
    }
    DynamicImage::ImageRgba8(out)
}

/// `base` with `source` restored under `mask` (non-zero pixels) inside
/// `region`, leaving every other pixel as it was.
///
/// A repair stroke repaints exactly these pixels. Flux2 edits what it is
/// shown: shown an earlier fill that went wrong (for example the white
/// outline of lettering, kept as if it were part of the bubble), it keeps that
/// fill; shown the original lettering, it removes it. Restoring only the
/// repainted pixels, not the stroke's whole rectangle, leaves no seam.
pub fn restore_masked_from_source(
    base: &DynamicImage,
    source: &DynamicImage,
    mask: &image::GrayImage,
    region: &Region,
) -> DynamicImage {
    let mut out = base.to_rgba8();
    let src = source.to_rgba8();
    let (w, h) = out.dimensions();
    let (sw, sh) = src.dimensions();
    let (mw, mh) = mask.dimensions();
    let x1 = region.x.saturating_add(region.width).min(w).min(sw).min(mw);
    let y1 = region
        .y
        .saturating_add(region.height)
        .min(h)
        .min(sh)
        .min(mh);
    for y in region.y.min(y1)..y1 {
        for x in region.x.min(x1)..x1 {
            if mask.get_pixel(x, y)[0] > 0 {
                out.put_pixel(x, y, *src.get_pixel(x, y));
            }
        }
    }
    DynamicImage::ImageRgba8(out)
}

/// Whether `a` and `b` differ in colour at any pixel under `mask` (non-zero)
/// inside `region`.
///
/// Between the cleaned page and the original page this tells a repair stroke
/// that goes over pixels cleaned earlier from one on lettering that is still
/// on the page: cleaned pages are stored without loss, so a pixel that was
/// never repainted is the original's.
pub fn differs_under_mask(
    a: &DynamicImage,
    b: &DynamicImage,
    mask: &GrayImage,
    region: &Region,
) -> bool {
    let (w, h) = a.dimensions();
    let (bw, bh) = b.dimensions();
    let (mw, mh) = mask.dimensions();
    let x1 = region.x.saturating_add(region.width).min(w).min(bw).min(mw);
    let y1 = region
        .y
        .saturating_add(region.height)
        .min(h)
        .min(bh)
        .min(mh);
    let (x0, y0) = (region.x.min(x1), region.y.min(y1));
    if x0 == x1 || y0 == y1 {
        return false;
    }
    let a = a.crop_imm(x0, y0, x1 - x0, y1 - y0).to_rgb8();
    let b = b.crop_imm(x0, y0, x1 - x0, y1 - y0).to_rgb8();
    a.enumerate_pixels()
        .any(|(x, y, pixel)| mask.get_pixel(x0 + x, y0 + y)[0] > 0 && pixel != b.get_pixel(x, y))
}

/// `region` grown by `margin` on every side, kept on a `width` x `height` page.
pub fn grow_region(region: &Region, margin: u32, width: u32, height: u32) -> Region {
    let x0 = region.x.saturating_sub(margin).min(width);
    let y0 = region.y.saturating_sub(margin).min(height);
    let x1 = region
        .x
        .saturating_add(region.width)
        .saturating_add(margin)
        .min(width);
    let y1 = region
        .y
        .saturating_add(region.height)
        .saturating_add(margin)
        .min(height);
    Region {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    }
}

/// The connected pieces of `mask` (non-zero pixels, joined along edges and
/// corners) that hold a `seeds` pixel, cut to `window`. Empty when the two
/// masks differ in size.
///
/// A repair stroke over cleaned lettering uses this to find the whole piece
/// of lettering it lies on, so Flux2 can be shown all of it.
pub fn mask_pieces_touching(mask: &GrayImage, seeds: &GrayImage, window: &Region) -> GrayImage {
    let (w, h) = mask.dimensions();
    let mut out = GrayImage::new(w, h);
    if seeds.dimensions() != (w, h) {
        return out;
    }
    let x0 = window.x.min(w);
    let y0 = window.y.min(h);
    let x1 = window.x.saturating_add(window.width).min(w);
    let y1 = window.y.saturating_add(window.height).min(h);
    let mut pending = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            if seeds.get_pixel(x, y)[0] > 0 && mask.get_pixel(x, y)[0] > 0 {
                out.put_pixel(x, y, image::Luma([255]));
                pending.push((x, y));
            }
        }
    }
    while let Some((x, y)) = pending.pop() {
        for ny in y.saturating_sub(1).max(y0)..(y + 2).min(y1) {
            for nx in x.saturating_sub(1).max(x0)..(x + 2).min(x1) {
                if mask.get_pixel(nx, ny)[0] > 0 && out.get_pixel(nx, ny)[0] == 0 {
                    out.put_pixel(nx, ny, image::Luma([255]));
                    pending.push((nx, ny));
                }
            }
        }
    }
    out
}

/// The smallest rectangle that holds every non-zero pixel of `mask`.
pub fn mask_bounds(mask: &GrayImage) -> Option<Region> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (x, y, pixel) in mask.enumerate_pixels() {
        if pixel[0] == 0 {
            continue;
        }
        bounds = Some(match bounds {
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            None => (x, y, x, y),
        });
    }
    bounds.map(|(x0, y0, x1, y1)| Region {
        x: x0,
        y: y0,
        width: x1 - x0 + 1,
        height: y1 - y0 + 1,
    })
}

/// The pixels set (non-zero) in both masks. Empty when they differ in size.
pub fn mask_intersection(a: &GrayImage, b: &GrayImage) -> GrayImage {
    if a.dimensions() != b.dimensions() {
        return GrayImage::new(a.width(), a.height());
    }
    GrayImage::from_fn(a.width(), a.height(), |x, y| {
        let both = a.get_pixel(x, y)[0] > 0 && b.get_pixel(x, y)[0] > 0;
        image::Luma([if both { 255 } else { 0 }])
    })
}

/// Translate the `koharu-ml` `TextDirection` primitive into the scene-layer one.
pub fn ml_text_direction_to_core(d: koharu_ml::types::TextDirection) -> koharu_core::TextDirection {
    match d {
        koharu_ml::types::TextDirection::Horizontal => koharu_core::TextDirection::Horizontal,
        koharu_ml::types::TextDirection::Vertical => koharu_core::TextDirection::Vertical,
    }
}

/// Translate a `koharu-ml::TextRegion` (detector output) into a scene-layer
/// `(bbox, TextData)` pair ready for `new_text_node`.
pub fn text_region_to_pair(
    r: koharu_ml::types::TextRegion,
    default_detector: &'static str,
) -> ([f32; 4], TextData) {
    let bbox = [r.x, r.y, r.x + r.width, r.y + r.height];
    let data = TextData {
        confidence: r.confidence,
        source_direction: r.source_direction.map(ml_text_direction_to_core),
        line_polygons: r.line_polygons,
        rotation_deg: r.rotation_deg,
        detected_font_size_px: r.detected_font_size_px,
        detector: r.detector.or_else(|| Some(default_detector.to_string())),
        ..Default::default()
    };
    (bbox, data)
}

/// Current node count on `page`, or 0 if the page doesn't exist.
pub fn page_node_count(scene: &Scene, page: PageId) -> usize {
    scene.page(page).map(|p| p.nodes.len()).unwrap_or(0)
}

/// Emit `RemoveNode` ops for every text node currently on `page`. Detectors
/// prepend these so a re-detect replaces the previous blocks instead of
/// layering on top. `prev_node` / `prev_index` are the best snapshot we have
/// at emission time — `ops::apply` overwrites them with the live state for
/// undo anyway.
pub fn clear_text_nodes_ops(scene: &Scene, page: PageId) -> Vec<Op> {
    let Some(page_ref) = scene.page(page) else {
        return Vec::new();
    };
    page_ref
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, (_, node))| matches!(&node.kind, NodeKind::Text(_)))
        .map(|(idx, (id, node))| Op::RemoveNode {
            page,
            id: *id,
            prev_node: node.clone(),
            prev_index: idx,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Manga reading-order sort (Recursive XY-Cut)
//
// Right-to-left columns, top-to-bottom within each column. Shared by every
// detector that emits text blocks (CTD, comic-text-bubble, PP-DocLayout).
// ---------------------------------------------------------------------------

/// Sort `(bbox, data)` pairs in a reading order (RTL, LTR, or Custom).
pub fn sort_manga_reading_order<T>(blocks: &mut [([f32; 4], T)], order: ReadingOrder) {
    #[derive(Debug, PartialEq, Clone, Copy)]
    enum Axis {
        X,
        Y,
    }

    if order == ReadingOrder::Custom {
        return;
    }

    if blocks.len() <= 1 {
        return;
    }

    let mut widths: Vec<f32> = blocks.iter().map(|(b, _)| b[2] - b[0]).collect();
    let mut heights: Vec<f32> = blocks.iter().map(|(b, _)| b[3] - b[1]).collect();
    widths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let median_w = widths[widths.len() / 2].max(1.0);
    let median_h = heights[heights.len() / 2].max(1.0);
    let min_gap_x = (median_w * 0.15).max(10.0);
    let min_gap_y = (median_h * 0.10).max(8.0);

    fn xy_cut_recursive<T>(
        blocks: &mut [([f32; 4], T)],
        min_gap_x: f32,
        min_gap_y: f32,
        order: ReadingOrder,
    ) {
        use std::cmp::Ordering;
        if blocks.len() <= 1 {
            return;
        }
        let cut = find_best_cut(blocks, min_gap_x, min_gap_y);
        let Some((axis, gap)) = cut else {
            let row_height = min_gap_y * 4.0;
            blocks.sort_by(|a, b| {
                let row_a = (a.0[1] / row_height).floor();
                let row_b = (b.0[1] / row_height).floor();
                row_a
                    .partial_cmp(&row_b)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| match order {
                        ReadingOrder::Rtl => b.0[0].partial_cmp(&a.0[0]).unwrap_or(Ordering::Equal),
                        ReadingOrder::Ltr => a.0[0].partial_cmp(&b.0[0]).unwrap_or(Ordering::Equal),
                        _ => Ordering::Equal,
                    })
            });
            return;
        };

        let cut_coord = (gap.0 + gap.1) / 2.0;
        blocks.sort_by_key(|(b, _)| {
            if axis == Axis::X {
                let center_x = b[0] + (b[2] - b[0]) * 0.5;
                match order {
                    ReadingOrder::Rtl => center_x < cut_coord, // Right first
                    ReadingOrder::Ltr => center_x > cut_coord, // Left first
                    _ => false,
                }
            } else {
                // Top partition first: items whose center is BELOW cut go second.
                (b[1] + (b[3] - b[1]) * 0.5) > cut_coord
            }
        });

        let group1_len = blocks
            .iter()
            .filter(|(b, _)| {
                if axis == Axis::X {
                    let center_x = b[0] + (b[2] - b[0]) * 0.5;
                    match order {
                        ReadingOrder::Rtl => center_x >= cut_coord,
                        ReadingOrder::Ltr => center_x <= cut_coord,
                        _ => true,
                    }
                } else {
                    (b[1] + (b[3] - b[1]) * 0.5) <= cut_coord
                }
            })
            .count();

        if group1_len == 0 || group1_len == blocks.len() {
            blocks.sort_by(|a, b| match order {
                ReadingOrder::Rtl => b.0[0].partial_cmp(&a.0[0]).unwrap_or(Ordering::Equal),
                ReadingOrder::Ltr => a.0[0].partial_cmp(&b.0[0]).unwrap_or(Ordering::Equal),
                _ => Ordering::Equal,
            });
            return;
        }

        let (left, right) = blocks.split_at_mut(group1_len);
        xy_cut_recursive(left, min_gap_x, min_gap_y, order);
        xy_cut_recursive(right, min_gap_x, min_gap_y, order);
    }

    fn find_best_cut<T>(
        blocks: &[([f32; 4], T)],
        min_gap_x: f32,
        min_gap_y: f32,
    ) -> Option<(Axis, (f32, f32))> {
        let mut x_intervals: Vec<(f32, f32)> = blocks.iter().map(|(b, _)| (b[0], b[2])).collect();
        let mut y_intervals: Vec<(f32, f32)> = blocks.iter().map(|(b, _)| (b[1], b[3])).collect();
        x_intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        y_intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let gap_x = find_largest_gap(&x_intervals, min_gap_x);
        let gap_y = find_largest_gap(&y_intervals, min_gap_y);
        match (gap_x, gap_y) {
            (Some(gx), Some(gy)) => {
                let width_y = gy.1 - gy.0;
                let width_x = gx.1 - gx.0;
                if width_y > 12.0 || width_y > (width_x * 0.4) {
                    Some((Axis::Y, gy))
                } else {
                    Some((Axis::X, gx))
                }
            }
            (None, Some(gy)) => Some((Axis::Y, gy)),
            (Some(gx), None) => Some((Axis::X, gx)),
            (None, None) => None,
        }
    }

    fn find_largest_gap(intervals: &[(f32, f32)], min_gap: f32) -> Option<(f32, f32)> {
        if intervals.is_empty() {
            return None;
        }
        let mut largest: Option<(f32, f32)> = None;
        let mut current_max_end = intervals[0].1;
        for interval in intervals.iter().skip(1) {
            if interval.0 > current_max_end {
                let gap = interval.0 - current_max_end;
                if gap >= min_gap
                    && match largest {
                        Some(best) => gap > best.1 - best.0,
                        None => true,
                    }
                {
                    largest = Some((current_max_end, interval.0));
                }
            }
            current_max_end = current_max_end.max(interval.1);
        }
        largest
    }

    xy_cut_recursive(blocks, min_gap_x, min_gap_y, order);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use koharu_core::ReadingOrder;

    #[test]
    fn single_line_ocr_text_joins_by_script() {
        // Japanese: lines are fragments of one sentence — no separator.
        assert_eq!(
            single_line_ocr_text("こんな告白\nされても\n困る…"),
            "こんな告白されても困る…"
        );
        // Korean shares the CJK blocks but spaces its words, so join with a
        // space instead of fusing the adjacent words into one.
        assert_eq!(
            single_line_ocr_text("반가워요\n오늘도\n좋은 하루"),
            "반가워요 오늘도 좋은 하루"
        );
        // Korean carrying a stray Hanja still counts as space-separated.
        assert_eq!(single_line_ocr_text("한국\n語"), "한국 語");
        // Latin scripts: space-separated, soft hyphens at breaks dropped.
        assert_eq!(
            single_line_ocr_text("even this confe-\nssion is\ntoo much"),
            "even this confession is too much"
        );
        // A Korean long-vowel dash at a break is text, not a soft hyphen: it
        // stays and the next word keeps its space.
        assert_eq!(
            single_line_ocr_text("그치만\n레이이-\n저 새끼가"),
            "그치만 레이이- 저 새끼가"
        );
        assert_eq!(single_line_ocr_text("에이-\nOK"), "에이- OK");
        // Blank lines and stray whitespace disappear; single lines pass through.
        assert_eq!(single_line_ocr_text("  one line  "), "one line");
        assert_eq!(single_line_ocr_text("a\r\n\r\nb"), "a b");
        assert_eq!(single_line_ocr_text(""), "");
    }

    #[test]
    fn is_degenerate_ocr_text_flags_only_emoji_hallucinations() {
        // Emoji + no letters = a hallucinated OCR failure (real BadEnd block 7).
        assert!(is_degenerate_ocr_text("🌞.💚"));
        assert!(is_degenerate_ocr_text("🎈"));
        // Real text, even with a stray emoji, is kept.
        assert!(!is_degenerate_ocr_text("안녕🌞"));
        assert!(!is_degenerate_ocr_text("응"));
        // BMP heart symbols the manga actually uses are not emoji.
        assert!(!is_degenerate_ocr_text("♡"));
        assert!(!is_degenerate_ocr_text("변태 자식이..♡"));
        // Punctuation/empty only: ambiguous, left alone (not blanked).
        assert!(!is_degenerate_ocr_text("..!?"));
        assert!(!is_degenerate_ocr_text(""));
    }

    #[test]
    fn restore_region_reverts_only_the_region_to_source() {
        let base = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([255; 4])));
        let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([10, 20, 30, 255])));

        let region = Region {
            x: 2,
            y: 2,
            width: 3,
            height: 3,
        };
        let out = restore_region_from_source(&base, &source, &region).to_rgba8();
        assert_eq!(out.get_pixel(2, 2).0, [10, 20, 30, 255]);
        assert_eq!(out.get_pixel(4, 4).0, [10, 20, 30, 255]);
        // Exclusive right/bottom edge and everything outside stay untouched.
        assert_eq!(out.get_pixel(5, 5).0, [255; 4]);
        assert_eq!(out.get_pixel(1, 1).0, [255; 4]);

        // A region overflowing the image is clamped, not a panic.
        let big = Region {
            x: 6,
            y: 6,
            width: 100,
            height: 100,
        };
        let out = restore_region_from_source(&base, &source, &big).to_rgba8();
        assert_eq!(out.get_pixel(7, 7).0, [10, 20, 30, 255]);
        assert_eq!(out.get_pixel(5, 5).0, [255; 4]);
    }

    #[test]
    fn restore_masked_reverts_only_masked_pixels_inside_the_region() {
        let base = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([255; 4])));
        let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([10, 20, 30, 255])));
        let mut mask = image::GrayImage::new(8, 8);
        for (x, y) in [(2, 2), (3, 3), (6, 6)] {
            mask.put_pixel(x, y, image::Luma([255]));
        }
        let region = Region {
            x: 1,
            y: 1,
            width: 4,
            height: 4,
        };

        let out = restore_masked_from_source(&base, &source, &mask, &region).to_rgba8();
        assert_eq!(out.get_pixel(2, 2).0, [10, 20, 30, 255]);
        assert_eq!(out.get_pixel(3, 3).0, [10, 20, 30, 255]);
        // Unmasked pixels in the region and masked ones outside it keep the base.
        assert_eq!(out.get_pixel(2, 3).0, [255; 4]);
        assert_eq!(out.get_pixel(6, 6).0, [255; 4]);

        // A region overflowing the image is clamped, not a panic.
        let big = Region {
            x: 5,
            y: 5,
            width: 100,
            height: 100,
        };
        let out = restore_masked_from_source(&base, &source, &mask, &big).to_rgba8();
        assert_eq!(out.get_pixel(6, 6).0, [10, 20, 30, 255]);
        assert_eq!(out.get_pixel(3, 3).0, [255; 4]);
    }

    #[test]
    fn differs_under_mask_looks_only_at_masked_pixels_in_the_region() {
        let a = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([255; 4])));
        let mut changed = RgbaImage::from_pixel(8, 8, Rgba([255; 4]));
        changed.put_pixel(3, 3, Rgba([10, 20, 30, 255]));
        // The other picture has no alpha channel: only the colour counts.
        let b = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(changed).to_rgb8());
        let region = Region {
            x: 2,
            y: 2,
            width: 4,
            height: 4,
        };

        let mut mask = image::GrayImage::new(8, 8);
        mask.put_pixel(4, 4, image::Luma([255]));
        assert!(!differs_under_mask(&a, &b, &mask, &region));
        mask.put_pixel(3, 3, image::Luma([255]));
        assert!(differs_under_mask(&a, &b, &mask, &region));

        // The changed pixel is masked but outside the region.
        let elsewhere = Region {
            x: 4,
            y: 4,
            width: 4,
            height: 4,
        };
        assert!(!differs_under_mask(&a, &b, &mask, &elsewhere));
        // A region off the page is empty, not a panic.
        let off = Region {
            x: 20,
            y: 20,
            width: 5,
            height: 5,
        };
        assert!(!differs_under_mask(&a, &b, &mask, &off));
    }

    #[test]
    fn grow_region_stays_on_the_page() {
        let region = Region {
            x: 10,
            y: 90,
            width: 20,
            height: 5,
        };
        let grown = grow_region(&region, 16, 100, 100);
        assert_eq!(
            (grown.x, grown.y, grown.width, grown.height),
            (0, 74, 46, 26)
        );
        let same = grow_region(&region, 0, 100, 100);
        assert_eq!((same.x, same.y, same.width, same.height), (10, 90, 20, 5));
    }

    #[test]
    fn mask_pieces_touching_follows_a_piece_out_of_the_seed_area() {
        let mut mask = image::GrayImage::new(40, 20);
        // A long bar, a second piece joined to nothing, and a third one that
        // touches the bar only at a corner.
        for x in 2..30 {
            mask.put_pixel(x, 5, image::Luma([255]));
        }
        for x in 2..10 {
            mask.put_pixel(x, 15, image::Luma([200]));
        }
        mask.put_pixel(30, 6, image::Luma([255]));
        let mut seeds = image::GrayImage::new(40, 20);
        seeds.put_pixel(3, 5, image::Luma([255]));
        // A seed off the mask starts nothing.
        seeds.put_pixel(20, 10, image::Luma([255]));
        let page = Region {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        };

        let pieces = mask_pieces_touching(&mask, &seeds, &page);
        assert_eq!(pieces.get_pixel(2, 5)[0], 255);
        assert_eq!(pieces.get_pixel(29, 5)[0], 255);
        assert_eq!(pieces.get_pixel(30, 6)[0], 255);
        assert_eq!(pieces.get_pixel(5, 15)[0], 0);
        assert_eq!(pieces.get_pixel(20, 10)[0], 0);
        assert_eq!(pieces.pixels().filter(|p| p[0] > 0).count(), 29);

        // The window cuts the piece short.
        let window = Region {
            x: 0,
            y: 0,
            width: 12,
            height: 20,
        };
        let cut = mask_pieces_touching(&mask, &seeds, &window);
        assert_eq!(cut.pixels().filter(|p| p[0] > 0).count(), 10);
        assert_eq!(cut.get_pixel(12, 5)[0], 0);

        // Masks of different sizes give nothing.
        let small = image::GrayImage::new(10, 10);
        let none = mask_pieces_touching(&mask, &small, &page);
        assert!(none.pixels().all(|p| p[0] == 0));
    }

    #[test]
    fn mask_intersection_keeps_pixels_set_in_both() {
        let mut a = image::GrayImage::new(4, 4);
        let mut b = image::GrayImage::new(4, 4);
        a.put_pixel(1, 1, image::Luma([255]));
        a.put_pixel(2, 2, image::Luma([90]));
        b.put_pixel(2, 2, image::Luma([255]));
        b.put_pixel(3, 3, image::Luma([255]));
        let both = mask_intersection(&a, &b);
        assert_eq!(both.get_pixel(2, 2)[0], 255);
        assert_eq!(both.pixels().filter(|p| p[0] > 0).count(), 1);
        let other = image::GrayImage::new(5, 4);
        assert!(mask_intersection(&a, &other).pixels().all(|p| p[0] == 0));
    }

    #[test]
    fn mask_bounds_holds_every_set_pixel() {
        let mut mask = image::GrayImage::new(20, 10);
        assert!(mask_bounds(&mask).is_none());
        mask.put_pixel(4, 7, image::Luma([1]));
        let one = mask_bounds(&mask).unwrap();
        assert_eq!((one.x, one.y, one.width, one.height), (4, 7, 1, 1));
        mask.put_pixel(15, 2, image::Luma([255]));
        let two = mask_bounds(&mask).unwrap();
        assert_eq!((two.x, two.y, two.width, two.height), (4, 2, 12, 6));
    }

    #[test]
    fn test_reading_order_sort() {
        // Two blocks side-by-side
        // B1: [100, 100, 200, 200] (Left)
        // B2: [300, 100, 400, 200] (Right)
        let b1 = [100.0, 100.0, 200.0, 200.0];
        let b2 = [300.0, 100.0, 400.0, 200.0];

        let mut blocks = vec![(b1, "left"), (b2, "right")];

        // RTL: Right should come first
        sort_manga_reading_order(&mut blocks, ReadingOrder::Rtl);
        assert_eq!(blocks[0].1, "right");
        assert_eq!(blocks[1].1, "left");

        // LTR: Left should come first
        sort_manga_reading_order(&mut blocks, ReadingOrder::Ltr);
        assert_eq!(blocks[0].1, "left");
        assert_eq!(blocks[1].1, "right");
    }
}
