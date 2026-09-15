//! Keep the erase mask and the inpainted layer in sync with text-node deletion.
//!
//! Deleting a text block used to leave its glyph pixels in the page's
//! `Mask { Segment }` node, so every later inpaint kept erasing lettering the
//! user had already dismissed, and the pixels that block's last inpaint had
//! already overwritten stayed overwritten
//! (`.recovery/inpaint-109-2026-09-11/findings.md`).
//!
//! [`sync_deleted_text_erase`] wraps any op that actually removes text nodes
//! into one `Op::Batch` that also clears those footprints from the segment
//! mask, restores the affected `Image { Inpainted }` pixels from `Source`, and
//! drops the now-stale `Image { Rendered }` composite. One batch = one history
//! entry, so node, mask, pixels and composite commit or fail together and
//! undo/redo/replay stay correct. Nothing here changes the scene v8 or
//! history v3 layouts — it only emits existing `UpdateNode`/`RemoveNode` ops.
//!
//! Three rules keep this from eating work the user meant to keep:
//!   - footprints are derived from the *before/after* scenes, so a batch that
//!     replaces or splits a block (detector re-run, split/merge) only clears
//!     what genuinely disappeared;
//!   - footprints are the bounds the inpainting mask expansion really uses
//!     for a block, and every surviving block's footprint is subtracted, so
//!     neighbouring blocks keep their shared area;
//!   - only pixels inside a deleted footprint are touched. Repair-brush
//!     strokes live in the same segment mask and routinely lie outside every
//!     text box, so clipping the whole mask to the extant boxes would delete
//!     them.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView};
use koharu_core::{
    BlobRef, ImageDataPatch, ImageRole, MaskDataPatch, MaskRole, NodeDataPatch, NodeId, NodeKind,
    NodePatch, Op, Page, PageId, Scene, TextData, TextDirection, Transform,
};
use koharu_ml::comic_text_detector::expanded_text_block_crop_bounds;
use koharu_ml::types::TextRegion;

use crate::blobs::BlobStore;

/// Label for the batch synthesised around a bare (non-batch) removal.
const DELETE_LABEL: &str = "delete text";

// Block dilation radii mirrored from `koharu-ml`'s `inpainting::mask`. Both
// expansions start from `expanded_text_block_crop_bounds`: the glyph path
// (LaMa/AOT) dilates inside `crop + r` and merges up to `crop + 2r`, the
// region-fill path (Flux.2) fills `crop + r`. A block's erasure therefore
// never reaches further than `max(2 * legacy, modern)` past its crop bounds.
const LEGACY_MIN_DILATE_RADIUS: u8 = 2;
const LEGACY_MAX_DILATE_RADIUS: u8 = 8;
const LEGACY_BLOCK_DILATE_FONT_RATIO: f32 = 0.16;
const MODERN_MIN_DILATE_RADIUS: u8 = 3;
const MODERN_MAX_DILATE_RADIUS: u8 = 12;
const MODERN_BLOCK_DILATE_FONT_RATIO: f32 = 0.22;

/// Wrap `op` so that any text nodes it removes also lose their erase-mask
/// contribution, give their inpainted pixels back to `Source`, and
/// invalidate the rendered composite that still shows them.
///
/// Returns `op` unchanged when it removes no text node, when nothing on the
/// affected pages would change, or when the op cannot be applied at all — a
/// malformed op is handed straight through so `History::apply` produces the
/// canonical error with the scene, log and undo stacks untouched.
///
/// Fails when a layer that has to change cannot be read or written. The
/// caller must then not apply `op` either: committing the deletion alone
/// would silently keep the stale erase instructions, and skipping an
/// unreadable brush mask could overwrite protected work.
pub fn sync_deleted_text_erase(scene: &Scene, blobs: &BlobStore, op: Op) -> Result<Op> {
    if !removes_nodes(&op) {
        return Ok(op);
    }

    // Validate on a shadow scene before any blob is written, and use the
    // resulting after-state as the source of truth: a batch may itself have
    // replaced the mask or inpainted blob we are about to patch.
    let mut after = scene.clone();
    let mut probe = op.clone();
    if probe.apply(&mut after).is_err() {
        return Ok(op);
    }

    let mut extra = Vec::new();
    for (page_id, before_page) in &scene.pages {
        let Some(after_page) = after.pages.get(page_id) else {
            // The whole page went away; its mask and images went with it.
            continue;
        };
        let deleted = deleted_text_blocks(before_page, after_page);
        if deleted.is_empty() {
            continue;
        }
        let kept = kept_text_blocks(after_page);
        let ops = cleanup_ops(*page_id, after_page, blobs, &deleted, &kept)
            .with_context(|| format!("retire the erase mask of deleted text on page {page_id}"))?;
        extra.extend(ops);
    }

    if extra.is_empty() {
        return Ok(op);
    }
    Ok(match op {
        Op::Batch { mut ops, label } => {
            ops.extend(extra);
            Op::Batch { ops, label }
        }
        single => {
            let mut ops = vec![single];
            ops.extend(extra);
            Op::Batch {
                ops,
                label: DELETE_LABEL.to_string(),
            }
        }
    })
}

/// Whether `op` can remove a node at all. Cheap pre-filter: everything else
/// skips the scene clone.
fn removes_nodes(op: &Op) -> bool {
    match op {
        Op::RemoveNode { .. } => true,
        Op::Batch { ops, .. } => ops.iter().any(removes_nodes),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Blocks and footprints
// ---------------------------------------------------------------------------

/// A text node as the inpainting mask expansion sees it.
struct Block {
    region: TextRegion,
    /// The live (possibly slanted) box grown by `margin`.
    rotated: RotatedRect,
    margin: u32,
}

impl Block {
    fn new(transform: &Transform, text: &TextData) -> Option<Self> {
        let (x, y, w, h) = (transform.x, transform.y, transform.width, transform.height);
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
            return None;
        }
        let region = text_region(transform, text);
        let margin = dilate_reach(&region);
        let angle = if transform.rotation_deg.is_finite() {
            transform.rotation_deg
        } else {
            0.0
        };
        let (sin, cos) = angle.to_radians().sin_cos();
        let rotated = RotatedRect {
            cx: x + w * 0.5,
            cy: y + h * 0.5,
            hw: w * 0.5 + margin as f32,
            hh: h * 0.5 + margin as f32,
            sin,
            cos,
        };
        Some(Self {
            region,
            rotated,
            margin,
        })
    }

    /// The pixels this block's erasure can occupy on a `width`×`height` layer:
    /// the expansion's own crop bounds grown by the dilation reach, plus the
    /// live slanted box grown by the same reach. Plain detector boxes stay
    /// tight; CTD/line-polygon blocks get the crop padding the expansion
    /// really applies. The slanted box only matters for rotated blocks without
    /// line polygons, whose glyphs the upright crop bounds do not cover.
    fn footprint(&self, width: u32, height: u32) -> Footprint {
        let r = &self.region;
        // `expanded_text_block_crop_bounds` assumes a non-empty layer and,
        // for plain boxes, a top-left corner on it.
        let rect = if width > 0 && height > 0 && r.x < width as f32 && r.y < height as f32 {
            let [x1, y1, x2, y2] = expanded_text_block_crop_bounds(width, height, r);
            [
                x1.saturating_sub(self.margin),
                y1.saturating_sub(self.margin),
                x2.saturating_add(self.margin).min(width),
                y2.saturating_add(self.margin).min(height),
            ]
        } else {
            [0; 4]
        };
        Footprint {
            rect,
            rotated: self.rotated,
        }
    }
}

/// Same conversion as the pipeline's `text_node_to_region`, so the crop
/// bounds match what the inpainters computed for this node.
fn text_region(transform: &Transform, text: &TextData) -> TextRegion {
    TextRegion {
        x: transform.x,
        y: transform.y,
        width: transform.width,
        height: transform.height,
        confidence: text.confidence,
        line_polygons: text.line_polygons.clone(),
        source_direction: text.source_direction.map(|direction| match direction {
            TextDirection::Horizontal => koharu_ml::types::TextDirection::Horizontal,
            TextDirection::Vertical => koharu_ml::types::TextDirection::Vertical,
        }),
        rotation_deg: (transform.rotation_deg.abs() > 0.05).then_some(transform.rotation_deg),
        detected_font_size_px: text.detected_font_size_px,
        detector: text.detector.clone(),
    }
}

/// How far past its crop bounds a block's erasure can reach, with the exact
/// arithmetic of `koharu-ml`'s block dilate radii.
fn dilate_reach(region: &TextRegion) -> u32 {
    let font = region
        .detected_font_size_px
        .unwrap_or_else(|| region.width.min(region.height).max(1.0));
    let legacy = ((font * LEGACY_BLOCK_DILATE_FONT_RATIO).round() as u8)
        .clamp(LEGACY_MIN_DILATE_RADIUS, LEGACY_MAX_DILATE_RADIUS);
    let modern = ((font * MODERN_BLOCK_DILATE_FONT_RATIO).round() as u8)
        .clamp(MODERN_MIN_DILATE_RADIUS, MODERN_MAX_DILATE_RADIUS);
    (2 * u32::from(legacy)).max(u32::from(modern))
}

/// A box rotated about its centre, stored as half-extents in its own frame
/// so containment is exact.
#[derive(Clone, Copy, Debug)]
struct RotatedRect {
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    sin: f32,
    cos: f32,
}

impl RotatedRect {
    fn contains(&self, x: f32, y: f32) -> bool {
        let dx = x - self.cx;
        let dy = y - self.cy;
        let local_x = dx * self.cos + dy * self.sin;
        let local_y = -dx * self.sin + dy * self.cos;
        local_x.abs() <= self.hw && local_y.abs() <= self.hh
    }

    /// Axis-aligned pixel bounds `[x0, y0, x1, y1)` clamped to the layer.
    fn pixel_bounds(&self, width: u32, height: u32) -> [u32; 4] {
        let extent_x = self.hw * self.cos.abs() + self.hh * self.sin.abs();
        let extent_y = self.hw * self.sin.abs() + self.hh * self.cos.abs();
        let x0 = (self.cx - extent_x).floor().clamp(0.0, width as f32) as u32;
        let y0 = (self.cy - extent_y).floor().clamp(0.0, height as f32) as u32;
        let x1 = (self.cx + extent_x).ceil().clamp(x0 as f32, width as f32) as u32;
        let y1 = (self.cy + extent_y).ceil().clamp(y0 as f32, height as f32) as u32;
        [x0, y0, x1, y1]
    }
}

/// A block's erase footprint on one layer size.
#[derive(Clone, Copy, Debug)]
struct Footprint {
    rect: [u32; 4],
    rotated: RotatedRect,
}

impl Footprint {
    fn contains(&self, x: u32, y: u32) -> bool {
        let [x0, y0, x1, y1] = self.rect;
        (x >= x0 && x < x1 && y >= y0 && y < y1)
            || self.rotated.contains(x as f32 + 0.5, y as f32 + 0.5)
    }

    fn pixel_bounds(&self, width: u32, height: u32) -> Option<[u32; 4]> {
        [self.rect, self.rotated.pixel_bounds(width, height)]
            .into_iter()
            .filter(|[x0, y0, x1, y1]| x0 < x1 && y0 < y1)
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            })
    }
}

fn deleted_text_blocks(before: &Page, after: &Page) -> Vec<Block> {
    before
        .nodes
        .iter()
        .filter(|(id, _)| !after.nodes.contains_key(*id))
        .filter_map(|(_, node)| match &node.kind {
            NodeKind::Text(text) => Block::new(&node.transform, text),
            _ => None,
        })
        .collect()
}

fn kept_text_blocks(after: &Page) -> Vec<Block> {
    after
        .nodes
        .values()
        .filter_map(|node| match &node.kind {
            NodeKind::Text(text) => Block::new(&node.transform, text),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Clear region
// ---------------------------------------------------------------------------

/// Row-major "this pixel belonged to a deleted block and to no surviving one".
struct ClearRegion {
    width: u32,
    height: u32,
    bits: Vec<bool>,
    count: usize,
}

impl ClearRegion {
    fn build(width: u32, height: u32, deleted: &[Block], kept: &[Block]) -> Self {
        let kept: Vec<Footprint> = kept
            .iter()
            .map(|block| block.footprint(width, height))
            .collect();
        let mut bits = vec![false; width as usize * height as usize];
        let mut count = 0usize;
        for block in deleted {
            let footprint = block.footprint(width, height);
            let Some([x0, y0, x1, y1]) = footprint.pixel_bounds(width, height) else {
                continue;
            };
            for y in y0..y1 {
                let row = y as usize * width as usize;
                for x in x0..x1 {
                    let index = row + x as usize;
                    if bits[index]
                        || !footprint.contains(x, y)
                        || kept.iter().any(|keeper| keeper.contains(x, y))
                    {
                        continue;
                    }
                    bits[index] = true;
                    count += 1;
                }
            }
        }
        Self {
            width,
            height,
            bits,
            count,
        }
    }
}

/// Builds a [`ClearRegion`] on demand and reuses it while the dimensions
/// match — the mask and the inpainted image are the same size on every
/// well-formed page, so this normally rasterises once.
struct RegionCache<'a> {
    deleted: &'a [Block],
    kept: &'a [Block],
    cached: Option<ClearRegion>,
}

impl<'a> RegionCache<'a> {
    fn new(deleted: &'a [Block], kept: &'a [Block]) -> Self {
        Self {
            deleted,
            kept,
            cached: None,
        }
    }

    fn get(&mut self, width: u32, height: u32) -> &ClearRegion {
        let hit = matches!(&self.cached, Some(region) if region.width == width && region.height == height);
        if !hit {
            self.cached = Some(ClearRegion::build(width, height, self.deleted, self.kept));
        }
        self.cached.as_ref().expect("region was just built")
    }
}

// ---------------------------------------------------------------------------
// Op construction
// ---------------------------------------------------------------------------

fn cleanup_ops(
    page_id: PageId,
    page: &Page,
    blobs: &BlobStore,
    deleted: &[Block],
    kept: &[Block],
) -> Result<Vec<Op>> {
    let mut regions = RegionCache::new(deleted, kept);
    // Fast path, no blob decoding: when the surviving blocks already cover
    // every deleted footprint — a split, a merge, a detector re-run that
    // replaced a block in place — nothing genuinely disappeared. Layers are
    // page-sized in practice; a layer that isn't re-rasterises below.
    if regions.get(page.width, page.height).count == 0 {
        return Ok(Vec::new());
    }

    let mut ops = Vec::new();

    // The cached composite still shows the deleted translation over the old
    // background, and a deletion made straight through the API never
    // schedules a re-render. Drop it (undoably); the renderer recreates it.
    if let Some((index, (id, node))) = page.nodes.iter().enumerate().find(|(_, (_, node))| {
        matches!(&node.kind, NodeKind::Image(image) if image.role == ImageRole::Rendered)
    }) {
        ops.push(Op::RemoveNode {
            page: page_id,
            id: *id,
            prev_node: node.clone(),
            prev_index: index,
        });
    }

    if let Some((node_id, blob)) = find_mask(page, MaskRole::Segment) {
        let mask = blobs
            .load_image(&blob)
            .with_context(|| format!("load segment mask blob {}", blob.hash()))?;
        let mut luma = mask.to_luma8();
        let (width, height) = luma.dimensions();
        let region = regions.get(width, height);
        if region.count > 0 {
            let mut changed = false;
            for (index, pixel) in luma.pixels_mut().enumerate() {
                if region.bits[index] && pixel.0[0] != 0 {
                    pixel.0[0] = 0;
                    changed = true;
                }
            }
            if changed {
                let new_blob = blobs
                    .put_webp(&DynamicImage::ImageLuma8(luma))
                    .context("store cleared segment mask")?;
                if new_blob != blob {
                    ops.push(update_mask_blob_op(page_id, node_id, new_blob));
                }
            }
        }
    }

    let Some((node_id, blob)) = find_image(page, ImageRole::Inpainted) else {
        return Ok(ops);
    };
    let Some((_, source_blob)) = find_image(page, ImageRole::Source) else {
        tracing::warn!(page = %page_id, "page has no Source image; not restoring deleted-text pixels");
        return Ok(ops);
    };
    let base = blobs
        .load_image(&blob)
        .with_context(|| format!("load inpainted blob {}", blob.hash()))?;
    let source = blobs
        .load_image(&source_blob)
        .with_context(|| format!("load source blob {}", source_blob.hash()))?;
    let (width, height) = base.dimensions();
    if source.dimensions() != (width, height) {
        // Can't map pixels between differently sized layers. The mask
        // clearing above is independently correct, so keep it.
        tracing::warn!(
            inpainted = ?base.dimensions(),
            source = ?source.dimensions(),
            "inpainted layer and source differ in size; not restoring deleted-text pixels"
        );
        return Ok(ops);
    }
    // Explicit brush work is authoritative: never hand those pixels back to
    // the source, even inside a deleted block's footprint. If the mask can't
    // be read that protection is unknown, so the whole deletion fails.
    let brush = match find_mask(page, MaskRole::BrushInpaint) {
        None => None,
        Some((_, brush_blob)) => {
            let brush = blobs
                .load_image(&brush_blob)
                .with_context(|| format!("load brush-inpaint mask blob {}", brush_blob.hash()))?;
            if brush.dimensions() != (width, height) {
                tracing::warn!(
                    brush = ?brush.dimensions(),
                    inpainted = ?(width, height),
                    "brush-inpaint mask and inpainted layer differ in size; not restoring deleted-text pixels"
                );
                return Ok(ops);
            }
            // BrushInpaint is a painted RGBA overlay, not a binary glyph
            // mask. Black paint is still paint; transparent RGB is not.
            Some(brush.to_rgba8())
        }
    };

    let region = regions.get(width, height);
    if region.count == 0 {
        return Ok(ops);
    }
    let mut out = base.to_rgba8();
    let src = source.to_rgba8();
    let mut changed = false;
    for (index, set) in region.bits.iter().enumerate() {
        if !set {
            continue;
        }
        let x = (index % width as usize) as u32;
        let y = (index / width as usize) as u32;
        if let Some(brush) = brush.as_ref()
            && brush.get_pixel(x, y).0[3] > 0
        {
            continue;
        }
        let restored = *src.get_pixel(x, y);
        if *out.get_pixel(x, y) != restored {
            out.put_pixel(x, y, restored);
            changed = true;
        }
    }
    if changed {
        let restored = DynamicImage::ImageRgba8(out);
        // Keep the layer's colour type: engines write these as RGB.
        let restored = if base.color().has_alpha() {
            restored
        } else {
            DynamicImage::ImageRgb8(restored.to_rgb8())
        };
        let new_blob = blobs
            .put_webp(&restored)
            .context("store restored inpainted layer")?;
        if new_blob != blob {
            ops.push(update_image_blob_op(
                page_id, node_id, new_blob, width, height,
            ));
        }
    }

    Ok(ops)
}

fn find_mask(page: &Page, role: MaskRole) -> Option<(NodeId, BlobRef)> {
    page.nodes.iter().find_map(|(id, node)| match &node.kind {
        NodeKind::Mask(mask) if mask.role == role => Some((*id, mask.blob.clone())),
        _ => None,
    })
}

fn find_image(page: &Page, role: ImageRole) -> Option<(NodeId, BlobRef)> {
    page.nodes.iter().find_map(|(id, node)| match &node.kind {
        NodeKind::Image(image) if image.role == role => Some((*id, image.blob.clone())),
        _ => None,
    })
}

fn update_mask_blob_op(page: PageId, id: NodeId, blob: BlobRef) -> Op {
    Op::UpdateNode {
        page,
        id,
        patch: NodePatch {
            data: Some(NodeDataPatch::Mask(MaskDataPatch { blob: Some(blob) })),
            ..Default::default()
        },
        prev: NodePatch::default(),
    }
}

fn update_image_blob_op(page: PageId, id: NodeId, blob: BlobRef, width: u32, height: u32) -> Op {
    Op::UpdateNode {
        page,
        id,
        patch: NodePatch {
            data: Some(NodeDataPatch::Image(ImageDataPatch {
                blob: Some(blob),
                natural_width: Some(width),
                natural_height: Some(height),
                ..Default::default()
            })),
            ..Default::default()
        },
        prev: NodePatch::default(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use camino::Utf8PathBuf;
    use image::{GrayImage, Luma, Rgba, RgbaImage};
    use koharu_core::{ImageData, MaskData, Node, Page, TextData};
    use tempfile::TempDir;

    use super::*;
    use crate::session::ProjectSession;

    const W: u32 = 128;
    const H: u32 = 128;
    /// Default block side. With `detected_font_size_px = 8` and no line
    /// polygons the crop bounds are the box itself and the dilation reach is
    /// `max(2 * clamp(round(1.28), 2, 8), clamp(round(1.76), 3, 12))` = 4 px.
    const BLOCK: f32 = 24.0;
    const FONT: Option<f32> = Some(8.0);
    const DETECTOR: &str = "comic-text-bubble-detector";
    /// A brush stroke deliberately painted outside every text box.
    const STRAY: [u32; 4] = [60, 100, 72, 112];

    struct Fixture {
        _dir: TempDir,
        path: Utf8PathBuf,
        session: Arc<ProjectSession>,
        page: PageId,
        blocks: Vec<NodeId>,
    }

    fn source_pixel(x: u32, y: u32) -> Rgba<u8> {
        Rgba([x as u8, y as u8, 64, 255])
    }

    /// Inner glyph patch of a `size`-sided block whose top-left is `origin`.
    fn glyph_rect_sized(origin: [f32; 2], size: f32) -> [u32; 4] {
        let (x, y, size) = (origin[0] as u32, origin[1] as u32, size as u32);
        [x + 4, y + 4, x + size - 4, y + size - 4]
    }

    fn glyph_rect(origin: [f32; 2]) -> [u32; 4] {
        glyph_rect_sized(origin, BLOCK)
    }

    fn fill(mask: &mut GrayImage, [x0, y0, x1, y1]: [u32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
    }

    fn erase(image: &mut RgbaImage, [x0, y0, x1, y1]: [u32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                image.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
    }

    fn text_data(font: Option<f32>) -> TextData {
        TextData {
            detected_font_size_px: font,
            detector: Some(DETECTOR.to_string()),
            text: Some("テキスト".to_string()),
            ..Default::default()
        }
    }

    fn text_node(origin: [f32; 2], size: f32, font: Option<f32>) -> Node {
        Node {
            id: NodeId::new(),
            transform: Transform {
                x: origin[0],
                y: origin[1],
                width: size,
                height: size,
                rotation_deg: 0.0,
            },
            visible: true,
            kind: NodeKind::Text(text_data(font)),
        }
    }

    fn image_node(role: ImageRole, blob: BlobRef) -> Node {
        Node {
            id: NodeId::new(),
            transform: Transform::default(),
            visible: true,
            kind: NodeKind::Image(ImageData {
                role,
                blob,
                opacity: 1.0,
                natural_width: W,
                natural_height: H,
                name: None,
            }),
        }
    }

    fn mask_node(role: MaskRole, blob: BlobRef) -> Node {
        Node {
            id: NodeId::new(),
            transform: Transform::default(),
            visible: false,
            kind: NodeKind::Mask(MaskData { role, blob }),
        }
    }

    /// Default-sized blocks plus the stray brush stroke.
    fn fixture(origins: &[[f32; 2]]) -> Fixture {
        let blocks: Vec<_> = origins
            .iter()
            .map(|origin| (*origin, BLOCK, FONT))
            .collect();
        fixture_with(&blocks, &[STRAY])
    }

    /// A project with one page: source art, an inpainted layer whose blocks
    /// and strokes are already whited out, a segment mask holding each
    /// block's glyphs plus the strokes, one text node per block, and a
    /// rendered composite on top.
    fn fixture_with(blocks: &[([f32; 2], f32, Option<f32>)], strokes: &[[u32; 4]]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .join("proj.khrproj");
        let session = ProjectSession::create(&path, "erase").unwrap();

        let source = RgbaImage::from_fn(W, H, source_pixel);
        let mut mask = GrayImage::new(W, H);
        let mut inpainted = source.clone();
        let rects = blocks
            .iter()
            .map(|(origin, size, _)| glyph_rect_sized(*origin, *size))
            .chain(strokes.iter().copied());
        for rect in rects {
            fill(&mut mask, rect);
            erase(&mut inpainted, rect);
        }
        let rendered = RgbaImage::from_pixel(W, H, Rgba([9, 9, 9, 255]));

        let put = |image: DynamicImage| session.blobs.put_webp(&image).unwrap();
        let mut page = Page::new("p1", W, H);
        let page_id = page.id;
        for node in [
            image_node(ImageRole::Source, put(DynamicImage::ImageRgba8(source))),
            image_node(
                ImageRole::Inpainted,
                put(DynamicImage::ImageRgba8(inpainted)),
            ),
            mask_node(MaskRole::Segment, put(DynamicImage::ImageLuma8(mask))),
        ] {
            page.nodes.insert(node.id, node);
        }
        let mut ids = Vec::new();
        for (origin, size, font) in blocks {
            let node = text_node(*origin, *size, *font);
            ids.push(node.id);
            page.nodes.insert(node.id, node);
        }
        let node = image_node(ImageRole::Rendered, put(DynamicImage::ImageRgba8(rendered)));
        page.nodes.insert(node.id, node);
        session.apply(Op::AddPage { page, at: 0 }).unwrap();

        Fixture {
            _dir: dir,
            path,
            session,
            page: page_id,
            blocks: ids,
        }
    }

    impl Fixture {
        fn with_page<T>(&self, read: impl FnOnce(&Page) -> T) -> T {
            let scene = self.session.scene.read();
            read(scene.page(self.page).unwrap())
        }

        fn mask(&self) -> GrayImage {
            let (_, blob) = self.with_page(|page| find_mask(page, MaskRole::Segment).unwrap());
            self.session.blobs.load_image(&blob).unwrap().to_luma8()
        }

        fn inpainted(&self) -> RgbaImage {
            let (_, blob) = self.with_page(|page| find_image(page, ImageRole::Inpainted).unwrap());
            self.session.blobs.load_image(&blob).unwrap().to_rgba8()
        }

        fn blobs(&self) -> (BlobRef, BlobRef) {
            self.with_page(|page| {
                (
                    find_mask(page, MaskRole::Segment).unwrap().1,
                    find_image(page, ImageRole::Inpainted).unwrap().1,
                )
            })
        }

        fn rendered(&self) -> Option<BlobRef> {
            self.with_page(|page| find_image(page, ImageRole::Rendered).map(|(_, blob)| blob))
        }

        fn remove(&self, index: usize) -> Op {
            let id = self.blocks[index];
            self.with_page(|page| Op::RemoveNode {
                page: self.page,
                id,
                prev_node: page.nodes[&id].clone(),
                prev_index: page.nodes.get_index_of(&id).unwrap(),
            })
        }

        fn text_count(&self) -> usize {
            self.with_page(|page| {
                page.nodes
                    .values()
                    .filter(|node| matches!(node.kind, NodeKind::Text(_)))
                    .count()
            })
        }
    }

    fn footprint_at(origin: [f32; 2], rotation_deg: f32) -> Footprint {
        Block::new(
            &Transform {
                x: origin[0],
                y: origin[1],
                width: BLOCK,
                height: BLOCK,
                rotation_deg,
            },
            &text_data(FONT),
        )
        .expect("fixture blocks are well formed")
        .footprint(W, H)
    }

    fn inside([x0, y0, x1, y1]: [u32; 4], x: u32, y: u32) -> bool {
        x >= x0 && x < x1 && y >= y0 && y < y1
    }

    fn assert_clear(mask: &GrayImage, [x0, y0, x1, y1]: [u32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                assert_eq!(mask.get_pixel(x, y).0[0], 0, "mask pixel ({x},{y})");
            }
        }
    }

    fn assert_set(mask: &GrayImage, [x0, y0, x1, y1]: [u32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                assert_eq!(mask.get_pixel(x, y).0[0], 255, "mask pixel ({x},{y})");
            }
        }
    }

    fn assert_restored(image: &RgbaImage, [x0, y0, x1, y1]: [u32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                assert_eq!(
                    *image.get_pixel(x, y),
                    source_pixel(x, y),
                    "pixel ({x},{y})"
                );
            }
        }
    }

    fn assert_erased(image: &RgbaImage, [x0, y0, x1, y1]: [u32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                assert_eq!(
                    *image.get_pixel(x, y),
                    Rgba([255, 255, 255, 255]),
                    "pixel ({x},{y})"
                );
            }
        }
    }

    /// Whole-layer check: painted pixels survive exactly where `cleared` is
    /// false, everything else shows the source.
    fn assert_layers(
        fixture: &Fixture,
        painted: impl Fn(u32, u32) -> bool,
        cleared: impl Fn(u32, u32) -> bool,
    ) {
        let mask = fixture.mask();
        let inpainted = fixture.inpainted();
        for y in 0..H {
            for x in 0..W {
                let survives = painted(x, y) && !cleared(x, y);
                assert_eq!(
                    mask.get_pixel(x, y).0[0],
                    if survives { 255 } else { 0 },
                    "mask ({x},{y})"
                );
                assert_eq!(
                    *inpainted.get_pixel(x, y),
                    if survives {
                        Rgba([255, 255, 255, 255])
                    } else {
                        source_pixel(x, y)
                    },
                    "pixel ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn deleting_a_block_clears_its_mask_restores_its_pixels_and_drops_the_render() {
        let fixture = fixture(&[[16.0, 16.0], [80.0, 16.0]]);
        let before = fixture.session.epoch();
        assert!(fixture.rendered().is_some());

        fixture.session.apply(fixture.remove(0)).unwrap();

        // One user action, one history entry.
        assert_eq!(fixture.session.epoch(), before + 1);
        assert_eq!(fixture.text_count(), 1);
        assert!(
            fixture.rendered().is_none(),
            "stale composite still shows the deleted translation"
        );

        let mask = fixture.mask();
        assert_clear(&mask, glyph_rect([16.0, 16.0]));
        // The surviving block and the stray brush stroke are untouched.
        assert_set(&mask, glyph_rect([80.0, 16.0]));
        assert_set(&mask, STRAY);

        let inpainted = fixture.inpainted();
        assert_restored(&inpainted, glyph_rect([16.0, 16.0]));
        assert_erased(&inpainted, glyph_rect([80.0, 16.0]));
        assert_erased(&inpainted, STRAY);
    }

    #[test]
    fn overlapping_kept_block_keeps_its_shared_area() {
        // The boxes overlap by 8 px, so each footprint reaches into the
        // other's glyphs. Everything the survivor could still need —
        // including the deleted block's own glyph pixels inside its
        // footprint — has to stay.
        let deleted_origin = [16.0, 16.0];
        let kept_origin = [16.0 + BLOCK - 8.0, 16.0];
        let fixture = fixture(&[deleted_origin, kept_origin]);

        fixture.session.apply(fixture.remove(0)).unwrap();

        let deleted = footprint_at(deleted_origin, 0.0);
        let kept = footprint_at(kept_origin, 0.0);
        let painted = |x, y| {
            inside(glyph_rect(deleted_origin), x, y)
                || inside(glyph_rect(kept_origin), x, y)
                || inside(STRAY, x, y)
        };
        let cleared = |x, y| deleted.contains(x, y) && !kept.contains(x, y);
        assert_layers(&fixture, painted, cleared);

        let glyphs = glyph_rect(deleted_origin);
        let (mut gone, mut protected) = (0, 0);
        for y in glyphs[1]..glyphs[3] {
            for x in glyphs[0]..glyphs[2] {
                if cleared(x, y) {
                    gone += 1;
                } else {
                    protected += 1;
                }
            }
        }
        assert!(gone > 0, "the deleted block's own glyphs must be cleared");
        assert!(
            protected > 0,
            "fixture must overlap the survivor's footprint"
        );
        assert_set(&fixture.mask(), glyph_rect(kept_origin));
    }

    #[test]
    fn ordinary_boxes_with_a_modest_gap_use_the_real_mask_bounds() {
        // Two plain 48 px detector boxes 24 px apart. Their crop bounds are
        // the boxes themselves and the dilation reach is
        // max(2 * clamp(round(7.68), 2, 8), clamp(round(10.56), 3, 12)) = 16.
        // Invented crop padding (~9 px on top) would let the survivor protect
        // the deleted box's right-hand glyphs and let the deletion eat the
        // stroke below it.
        const SIZE: f32 = 48.0;
        let left = [8.0, 8.0];
        let right = [80.0, 8.0];
        let within_reach: [u32; 4] = [20, 58, 40, 70];
        let beyond_reach: [u32; 4] = [20, 76, 40, 84];
        let fixture = fixture_with(
            &[(left, SIZE, None), (right, SIZE, None)],
            &[within_reach, beyond_reach],
        );

        fixture.session.apply(fixture.remove(0)).unwrap();

        let painted = |x, y| {
            inside(glyph_rect_sized(left, SIZE), x, y)
                || inside(glyph_rect_sized(right, SIZE), x, y)
                || inside(within_reach, x, y)
                || inside(beyond_reach, x, y)
        };
        // Left box [8, 56) grown by 16 on every side.
        let cleared = |x, y| inside([0, 0, 72, 72], x, y);
        assert_layers(&fixture, painted, cleared);
        let mask = fixture.mask();
        assert_clear(&mask, glyph_rect_sized(left, SIZE));
        assert_clear(&mask, within_reach);
        assert_set(&mask, beyond_reach);
        assert_set(&mask, glyph_rect_sized(right, SIZE));
    }

    #[test]
    fn removing_every_block_leaves_only_brush_pixels() {
        let origins = [[16.0, 16.0], [80.0, 16.0]];
        let fixture = fixture(&origins);

        fixture
            .session
            .apply(Op::Batch {
                ops: vec![fixture.remove(1), fixture.remove(0)],
                label: "clear page".into(),
            })
            .unwrap();

        assert_eq!(fixture.text_count(), 0);
        let mask = fixture.mask();
        for origin in origins {
            assert_clear(&mask, glyph_rect(origin));
        }
        // The brush stroke lies outside every box: a blanket "clip the mask to
        // the extant boxes" would have deleted it.
        assert_set(&mask, STRAY);

        let inpainted = fixture.inpainted();
        for origin in origins {
            assert_restored(&inpainted, glyph_rect(origin));
        }
        assert_erased(&inpainted, STRAY);
    }

    #[test]
    fn undo_redo_and_reopen_round_trip_mask_pixels_and_render() {
        let fixture = fixture(&[[16.0, 16.0], [80.0, 16.0]]);
        let (mask_before, inpainted_before) = fixture.blobs();
        let rendered_before = fixture.rendered().unwrap();

        fixture.session.apply(fixture.remove(0)).unwrap();
        let (mask_after, inpainted_after) = fixture.blobs();
        assert_ne!(mask_before, mask_after);
        assert_ne!(inpainted_before, inpainted_after);

        fixture.session.undo().unwrap().expect("undo the deletion");
        assert_eq!(fixture.text_count(), 2);
        assert_eq!(fixture.blobs(), (mask_before, inpainted_before));
        assert_eq!(fixture.rendered(), Some(rendered_before));
        assert_set(&fixture.mask(), glyph_rect([16.0, 16.0]));
        assert_erased(&fixture.inpainted(), glyph_rect([16.0, 16.0]));

        fixture.session.redo().unwrap().expect("redo the deletion");
        assert_eq!(fixture.text_count(), 1);
        assert_eq!(fixture.blobs(), (mask_after, inpainted_after));
        assert!(fixture.rendered().is_none());

        // Survives compaction and a fresh open.
        fixture.session.compact().unwrap();
        let epoch = fixture.session.epoch();
        let page = fixture.page;
        drop(fixture.session);
        let reopened = ProjectSession::open(&fixture.path).unwrap();
        assert_eq!(reopened.epoch(), epoch);
        let scene = reopened.scene.read();
        let reopened_page = scene.page(page).unwrap();
        assert!(find_image(reopened_page, ImageRole::Rendered).is_none());
        let (_, blob) = find_mask(reopened_page, MaskRole::Segment).unwrap();
        let mask = reopened.blobs.load_image(&blob).unwrap().to_luma8();
        assert_clear(&mask, glyph_rect([16.0, 16.0]));
        assert_set(&mask, glyph_rect([80.0, 16.0]));
        assert_set(&mask, STRAY);
    }

    #[test]
    fn same_batch_replacement_keeps_the_covered_area_and_render() {
        // A split (or a detector re-run) removes a block and adds nodes over
        // the same area in one batch: nothing there actually disappeared.
        let origin = [16.0, 16.0];
        let fixture = fixture(&[origin, [80.0, 16.0]]);
        let before = fixture.blobs();
        let rendered = fixture.rendered();

        let mut ops = vec![fixture.remove(0)];
        for index in 0..2 {
            let mut node = text_node(origin, BLOCK, FONT);
            node.transform.y = origin[1] + index as f32 * BLOCK * 0.5;
            node.transform.height = BLOCK * 0.5;
            ops.push(Op::AddNode {
                page: fixture.page,
                node,
                at: 3 + index,
            });
        }
        fixture
            .session
            .apply(Op::Batch {
                ops,
                label: "split block".into(),
            })
            .unwrap();

        assert_eq!(fixture.text_count(), 3);
        // The halves cover the original box, so nothing else changed.
        assert_eq!(fixture.blobs(), before);
        assert_eq!(fixture.rendered(), rendered);
        assert_set(&fixture.mask(), glyph_rect(origin));
        assert_erased(&fixture.inpainted(), glyph_rect(origin));
    }

    #[test]
    fn malformed_batch_leaves_scene_history_and_blobs_unchanged() {
        let fixture = fixture(&[[16.0, 16.0], [80.0, 16.0]]);
        let before = fixture.blobs();
        let epoch = fixture.session.epoch();

        let result = fixture.session.apply(Op::Batch {
            ops: vec![
                fixture.remove(0),
                Op::RemoveNode {
                    page: fixture.page,
                    id: NodeId::new(),
                    prev_node: text_node([0.0, 0.0], BLOCK, FONT),
                    prev_index: 0,
                },
            ],
            label: "half valid".into(),
        });

        assert!(result.is_err());
        assert_eq!(fixture.session.epoch(), epoch);
        assert_eq!(fixture.text_count(), 2);
        assert_eq!(fixture.blobs(), before);
        assert!(fixture.rendered().is_some());
        assert_set(&fixture.mask(), glyph_rect([16.0, 16.0]));
        assert_erased(&fixture.inpainted(), glyph_rect([16.0, 16.0]));
    }

    #[test]
    fn painted_brush_pixels_are_protected_by_alpha_not_brightness() {
        let fixture = fixture(&[[16.0, 16.0]]);
        let mut brush = RgbaImage::from_pixel(W, H, Rgba([255, 255, 255, 0]));
        brush.put_pixel(24, 24, Rgba([0, 0, 0, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(brush)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let blob = fixture.session.blobs.put_bytes(encoded.get_ref()).unwrap();
        fixture
            .session
            .apply(Op::AddNode {
                page: fixture.page,
                node: mask_node(MaskRole::BrushInpaint, blob.clone()),
                at: fixture.with_page(|page| page.nodes.len()),
            })
            .unwrap();
        fixture.session.apply(fixture.remove(0)).unwrap();
        assert_eq!(*fixture.inpainted().get_pixel(24, 24), Rgba([255; 4]));
        assert_eq!(*fixture.inpainted().get_pixel(25, 24), source_pixel(25, 24));
        assert_eq!(
            fixture.with_page(|page| find_mask(page, MaskRole::BrushInpaint).unwrap().1),
            blob
        );
    }

    #[test]
    fn unreadable_layers_fail_the_whole_deletion() {
        for case in [
            "missing segment mask",
            "corrupt inpainted layer",
            "corrupt brush-inpaint mask",
        ] {
            let fixture = fixture(&[[16.0, 16.0], [80.0, 16.0]]);
            let corrupt = fixture.session.blobs.put_bytes(b"not an image").unwrap();
            let sabotage = fixture.with_page(|page| match case {
                "missing segment mask" => update_mask_blob_op(
                    fixture.page,
                    find_mask(page, MaskRole::Segment).unwrap().0,
                    BlobRef::new("00ff-no-such-blob"),
                ),
                "corrupt inpainted layer" => update_image_blob_op(
                    fixture.page,
                    find_image(page, ImageRole::Inpainted).unwrap().0,
                    corrupt.clone(),
                    W,
                    H,
                ),
                _ => Op::AddNode {
                    page: fixture.page,
                    node: mask_node(MaskRole::BrushInpaint, corrupt.clone()),
                    at: page.nodes.len(),
                },
            });
            fixture.session.apply(sabotage).unwrap();

            let scene_before = postcard::to_allocvec(&*fixture.session.scene.read()).unwrap();
            let epoch = fixture.session.epoch();
            let log = fixture.path.join("history.log");
            let log_len = std::fs::metadata(log.as_std_path()).unwrap().len();

            let result = fixture.session.apply(fixture.remove(0));

            assert!(result.is_err(), "{case}: deletion must fail");
            assert_eq!(fixture.session.epoch(), epoch, "{case}");
            assert_eq!(
                postcard::to_allocvec(&*fixture.session.scene.read()).unwrap(),
                scene_before,
                "{case}"
            );
            assert_eq!(
                std::fs::metadata(log.as_std_path()).unwrap().len(),
                log_len,
                "{case}"
            );
            assert_eq!(fixture.text_count(), 2, "{case}");
        }
    }

    #[test]
    fn rotated_block_clears_its_rotated_footprint() {
        let fixture = fixture(&[[48.0, 48.0]]);
        // Slant the block after the fact; the transform is the live truth.
        let id = fixture.blocks[0];
        let op = fixture.with_page(|page| {
            let mut transform = page.nodes[&id].transform;
            transform.rotation_deg = 30.0;
            Op::UpdateNode {
                page: fixture.page,
                id,
                patch: NodePatch {
                    transform: Some(transform),
                    ..Default::default()
                },
                prev: NodePatch::default(),
            }
        });
        fixture.session.apply(op).unwrap();

        fixture.session.apply(fixture.remove(0)).unwrap();

        let footprint = footprint_at([48.0, 48.0], 30.0);
        assert_layers(
            &fixture,
            |x, y| inside(glyph_rect([48.0, 48.0]), x, y) || inside(STRAY, x, y),
            |x, y| footprint.contains(x, y),
        );
        assert_set(&fixture.mask(), STRAY);
    }

    #[test]
    fn footprint_follows_the_rotated_box_not_its_bounding_box() {
        // A wide plain block slanted 45°: the far end of its own long axis
        // must be cleared, while the corner of the axis-aligned bounding box
        // — which a naive implementation would also erase — must not be.
        let wide = |x, y, width, height, rotation_deg| {
            Block::new(
                &Transform {
                    x,
                    y,
                    width,
                    height,
                    rotation_deg,
                },
                &text_data(FONT),
            )
            .unwrap()
        };
        let deleted = [wide(20.0, 56.0, 88.0, 16.0, 45.0)];
        let [x0, y0, ..] = deleted[0].footprint(W, H).pixel_bounds(W, H).unwrap();
        assert!(x0 < 24 && y0 < 24, "the bounding box reaches the corner");

        let region = ClearRegion::build(W, H, &deleted, &[]);
        let bit = |x: u32, y: u32| region.bits[y as usize * W as usize + x as usize];
        assert!(bit(92, 92), "along the block's own long axis");
        assert!(!bit(24, 24), "corner of the axis-aligned bounding box");

        // A survivor sitting on the slanted end takes its area back.
        let kept = [wide(84.0, 84.0, 16.0, 16.0, 0.0)];
        let region = ClearRegion::build(W, H, &deleted, &kept);
        let bit = |x: u32, y: u32| region.bits[y as usize * W as usize + x as usize];
        assert!(!bit(92, 92));
        assert!(bit(40, 40), "the far end is still cleared");
    }

    #[test]
    fn ops_without_removals_are_passed_through_untouched() {
        let fixture = fixture(&[[16.0, 16.0]]);
        let before = fixture.blobs();
        let rendered = fixture.rendered();

        fixture
            .session
            .apply(Op::UpdateNode {
                page: fixture.page,
                id: fixture.blocks[0],
                patch: NodePatch {
                    data: Some(NodeDataPatch::Text(koharu_core::TextDataPatch {
                        translation: Some(Some("Hello".into())),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                prev: NodePatch::default(),
            })
            .unwrap();

        assert_eq!(fixture.blobs(), before);
        assert_eq!(fixture.rendered(), rendered);
        assert_set(&fixture.mask(), glyph_rect([16.0, 16.0]));
    }

    #[test]
    fn deleting_a_non_text_node_changes_no_pixels() {
        let fixture = fixture(&[[16.0, 16.0]]);
        let before = fixture.blobs();

        let op = fixture.with_page(|page| {
            let (id, _) = find_image(page, ImageRole::Inpainted).unwrap();
            Op::RemoveNode {
                page: fixture.page,
                id,
                prev_node: page.nodes[&id].clone(),
                prev_index: page.nodes.get_index_of(&id).unwrap(),
            }
        });
        fixture.session.apply(op).unwrap();

        fixture.with_page(|page| {
            assert!(find_image(page, ImageRole::Inpainted).is_none());
            assert!(find_image(page, ImageRole::Rendered).is_some());
            assert_eq!(find_mask(page, MaskRole::Segment).unwrap().1, before.0);
        });
    }
}
