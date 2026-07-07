//! Shared helpers used by multiple engine implementations.
//!
//! The patterns here map `koharu-ml` / `koharu-llm` outputs (plain
//! `TextRegion`s, `DynamicImage`s) into `Op` sequences that mutate the scene.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView};
use koharu_core::{
    BlobRef, ImageData, ImageRole, MaskData, MaskRole, Node, NodeDataPatch, NodeId, NodeKind, Op,
    PageId, ReadingOrder, Region, Scene, TextData, Transform,
};

use crate::blobs::BlobStore;

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

/// Convert a scene `(Transform, TextData)` pair into a `koharu-ml` `TextRegion`
/// for passing back through detector helpers that need geometry + language
/// hints (e.g. CTD's `refine_segmentation_mask`, OCR's `extract_text_block_regions`).
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
/// at a line break is treated as a soft hyphen and dropped.
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
        } else if out.ends_with('-') {
            out.pop();
            out.push_str(line);
        } else {
            out.push(' ');
            out.push_str(line);
        }
    }
    out
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
        // Blank lines and stray whitespace disappear; single lines pass through.
        assert_eq!(single_line_ocr_text("  one line  "), "one line");
        assert_eq!(single_line_ocr_text("a\r\n\r\nb"), "a b");
        assert_eq!(single_line_ocr_text(""), "");
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
