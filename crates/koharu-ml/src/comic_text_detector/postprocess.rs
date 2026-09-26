use crate::types::{TextDirection, TextRegion};
use image::{
    DynamicImage, GrayImage, Luma, Rgb, RgbImage,
    imageops::{self},
};
use imageproc::{
    distance_transform::Norm,
    geometric_transformations::{Interpolation, Projection, warp_into},
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};

const FINAL_MASK_DILATE_RADIUS: u8 = 2;

pub type Quad = [[f32; 2]; 4];

#[derive(Debug, Clone)]
pub struct ComicTextDetection {
    pub shrink_map: GrayImage,
    pub threshold_map: GrayImage,
    pub line_polygons: Vec<Quad>,
    pub text_blocks: Vec<TextRegion>,
    pub mask: GrayImage,
}

pub fn refine_segmentation_mask(
    image: &DynamicImage,
    pred_mask: &GrayImage,
    blocks: &[TextRegion],
) -> GrayImage {
    let width = pred_mask.width();
    let height = pred_mask.height();

    if blocks.is_empty() {
        return GrayImage::new(width, height);
    }

    // Extract expanded bounding boxes globally to validate intersection constraints.
    let expanded_bounds: Vec<[u32; 4]> = blocks
        .iter()
        .map(|b| expanded_text_block_crop_bounds(width, height, b))
        .collect();

    // Rasterize the union of expanded text block bounds once to avoid an
    // O(width * height * blocks) per-pixel rectangle membership test.
    let mut in_bounds_mask = GrayImage::new(width, height);
    for &[x1, y1, x2, y2] in &expanded_bounds {
        for y in y1..y2 {
            for x in x1..x2 {
                in_bounds_mask.put_pixel(x, y, Luma([255]));
            }
        }
    }

    // Apply a threshold mask: Pixels are preserved exclusively if their probability
    // exceeds the core threshold (`super::BINARY_THRESHOLD`) and they reside within a known TextRegion geometry.
    let base = GrayImage::from_fn(width, height, |x, y| {
        if in_bounds_mask.get_pixel(x, y)[0] != 0
            && pred_mask.get_pixel(x, y)[0] > super::BINARY_THRESHOLD
        {
            Luma([255])
        } else {
            Luma([0])
        }
    });

    let completed = complete_partial_glyphs(image, &base, &expanded_bounds);
    let dilated = dilate(&completed, Norm::L1, FINAL_MASK_DILATE_RADIUS);

    // Final clipping pass: Ensure the dilated mask never escapes the block boundaries
    // even if it thickens beyond its original source pixel edges.
    GrayImage::from_fn(width, height, |x, y| {
        if in_bounds_mask.get_pixel(x, y)[0] != 0 {
            *dilated.get_pixel(x, y)
        } else {
            Luma([0])
        }
    })
}

/// Complete partially segmented, high-contrast glyph components. The source
/// component must be anchored in the detector's mask and entirely contained
/// in a text box: a background or drawing line crossing the box is excluded.
/// This runs during segmentation only. Subsequent manual mask erasures remain
/// authoritative when the user runs an inpainter again.
pub fn complete_partial_glyphs(
    image: &DynamicImage,
    base: &GrayImage,
    bounds: &[[u32; 4]],
) -> GrayImage {
    let mut completed = base.clone();
    if image.width() != base.width() || image.height() != base.height() {
        return completed;
    }
    let rgb = image.to_rgb8();
    for &[x0, y0, x1, y1] in bounds {
        let width = x1.saturating_sub(x0);
        let height = y1.saturating_sub(y0);
        if width < 3 || height < 3 {
            continue;
        }
        let mut additions = GrayImage::new(width, height);
        for white in [false, true] {
            let candidates = GrayImage::from_fn(width, height, |x, y| {
                let pixel = rgb.get_pixel(x0 + x, y0 + y).0;
                let extreme = if white {
                    pixel.into_iter().all(|v| v >= 235)
                } else {
                    pixel.into_iter().all(|v| v <= 45)
                };
                Luma([if extreme { 255 } else { 0 }])
            });
            let labels = connected_components(&candidates, Connectivity::Eight, Luma([0]));
            let count = labels.pixels().map(|p| p[0]).max().unwrap_or(0) as usize + 1;
            // area, anchored pixels, min x/y, max x/y
            let mut stats = vec![[0, 0, width, height, 0, 0]; count];
            for (x, y, label) in labels.enumerate_pixels() {
                if label[0] == 0 {
                    continue;
                }
                let stat = &mut stats[label[0] as usize];
                stat[0] += 1;
                stat[1] += u32::from(base.get_pixel(x0 + x, y0 + y)[0] > 0);
                stat[2] = stat[2].min(x);
                stat[3] = stat[3].min(y);
                stat[4] = stat[4].max(x);
                stat[5] = stat[5].max(y);
            }
            let accepted: Vec<bool> = stats
                .iter()
                .map(|&[area, anchored, min_x, min_y, max_x, max_y]| {
                    if area < 8 || anchored < 2 || anchored * 10 < area {
                        return false;
                    }
                    if min_x == 0 || min_y == 0 || max_x + 1 == width || max_y + 1 == height {
                        return false;
                    }
                    let w = max_x - min_x + 1;
                    let h = max_y - min_y + 1;
                    // Long panel/drawing strokes are not glyph completions.
                    w.min(h) >= 2 && w.max(h) <= w.min(h) * 10
                })
                .collect();
            for (x, y, label) in labels.enumerate_pixels() {
                if accepted[label[0] as usize] {
                    additions.put_pixel(x, y, Luma([255]));
                }
            }
        }
        let additions = crate::inpainting::mask::fill_enclosed_holes(&additions);
        for (x, y, pixel) in additions.enumerate_pixels() {
            if pixel[0] > 0 {
                completed.put_pixel(x0 + x, y0 + y, *pixel);
            }
        }
    }
    completed
}

pub fn crop_text_block_bbox(image: &DynamicImage, block: &TextRegion) -> DynamicImage {
    let [x1, y1, x2, y2] = if has_expanded_crop_bounds(block) {
        expanded_text_block_crop_bounds(image.width(), image.height(), block)
    } else {
        // Plain detector boxes hug the ink, and OCR models misread glyphs
        // that touch the crop border. Add the margin here rather than in the
        // shared bounds: mask consumers rely on those staying tight.
        let (pad_x, pad_y) = ocr_crop_margin(block);
        clamp_crop_bounds(
            image.width(),
            image.height(),
            block.x - pad_x,
            block.y - pad_y,
            block.x + block.width + pad_x,
            block.y + block.height + pad_y,
        )
    };
    image.crop_imm(x1, y1, x2.saturating_sub(x1), y2.saturating_sub(y1))
}

/// Minimum block angle (degrees) before a crop is worth deskewing; below
/// this the axis-aligned crop is effectively identical.
const DESKEW_MIN_DEG: f32 = 0.5;

/// Crop `block` out of `image`, warping it upright first when the block
/// carries a meaningful `rotation_deg`. The block geometry is the upright
/// `x/y/width/height` rect rotated about its own centre — the same
/// convention as the scene `Transform` and the renderer — so the returned
/// image contains the text as if it had been printed horizontally.
///
/// Straight blocks fall back to [`crop_text_block_bbox`] unchanged.
pub fn crop_text_block_deskewed(image: &DynamicImage, block: &TextRegion) -> DynamicImage {
    let angle = block.rotation_deg.unwrap_or(0.0);
    if !angle.is_finite() || angle.abs() < DESKEW_MIN_DEG {
        return crop_text_block_bbox(image, block);
    }

    // Pad the upright rect a little: rotation estimates hug the ink and OCR
    // models prefer a small margin around the glyphs.
    let pad = (block.width.min(block.height) * 0.08).clamp(2.0, 12.0);
    warp_block_upright(image, block, pad).unwrap_or_else(|| crop_text_block_bbox(image, block))
}

/// Crop `block` out of `image` exactly as its rect describes — deskewing a
/// rotated block, otherwise an axis-aligned crop — and adding **no** extra
/// OCR margin. The caller owns whatever padding the crop should carry.
///
/// This is the tight-crop path for callers that pre-expand the rect by their
/// own small margin (the Korean verifier) and must not also inherit the
/// generic [`ocr_crop_margin`], which is large enough to pull the bright
/// speech-balloon border into the crop and corrupt the verifier's row
/// projection.
pub fn crop_text_block_exact(image: &DynamicImage, block: &TextRegion) -> DynamicImage {
    let angle = block.rotation_deg.unwrap_or(0.0);
    if angle.is_finite()
        && angle.abs() >= DESKEW_MIN_DEG
        && let Some(warped) = warp_block_upright(image, block, 0.0)
    {
        return warped;
    }
    let [x1, y1, x2, y2] = clamp_crop_bounds(
        image.width(),
        image.height(),
        block.x,
        block.y,
        block.x + block.width,
        block.y + block.height,
    );
    image.crop_imm(x1, y1, x2.saturating_sub(x1), y2.saturating_sub(y1))
}

/// Warp the rotated `block` upright, padding the upright rect by `pad` on
/// every side. Returns `None` only when the projection is degenerate, so the
/// caller can fall back to an axis-aligned crop.
fn warp_block_upright(image: &DynamicImage, block: &TextRegion, pad: f32) -> Option<DynamicImage> {
    let w = block.width + 2.0 * pad;
    let h = block.height + 2.0 * pad;
    let cx = block.x + block.width * 0.5;
    let cy = block.y + block.height * 0.5;
    let angle = block.rotation_deg.unwrap_or(0.0);
    let (sin, cos) = angle.to_radians().sin_cos();

    // Corners of the padded upright rect rotated into image space
    // (clockwise from top-left, screen convention: y-down, CW-positive).
    let local = [
        [-w * 0.5, -h * 0.5],
        [w * 0.5, -h * 0.5],
        [w * 0.5, h * 0.5],
        [-w * 0.5, h * 0.5],
    ];
    let quad: Quad = local.map(|[lx, ly]| [cx + cos * lx - sin * ly, cy + sin * lx + cos * ly]);

    // Warp from a crop of the quad's bounding box, not the whole page, so
    // the source stays small; pixels outside the page fill white (manga
    // margins are white, and OCR reads dark-on-light).
    let bbox = quad_bbox(&quad);
    let x1 = bbox[0].floor().max(0.0) as u32;
    let y1 = bbox[1].floor().max(0.0) as u32;
    let x2 = (bbox[2].ceil().min(image.width() as f32) as u32).max(x1 + 1);
    let y2 = (bbox[3].ceil().min(image.height() as f32) as u32).max(y1 + 1);
    let cropped = image.crop_imm(x1, y1, x2 - x1, y2 - y1).to_rgb8();
    let src = quad.map(|[px, py]| (px - x1 as f32, py - y1 as f32));

    let out_w = (w.round() as u32).max(1);
    let out_h = (h.round() as u32).max(1);
    let dst = [
        (0.0f32, 0.0f32),
        ((out_w - 1) as f32, 0.0),
        ((out_w - 1) as f32, (out_h - 1) as f32),
        (0.0, (out_h - 1) as f32),
    ];
    let projection = Projection::from_control_points(src, dst)?;

    let mut out = RgbImage::from_pixel(out_w, out_h, Rgb([255, 255, 255]));
    warp_into(
        &cropped,
        projection,
        Interpolation::Bilinear,
        imageproc::geometric_transformations::Border::Constant(Rgb([255, 255, 255])),
        &mut out,
    );
    Some(DynamicImage::ImageRgb8(out))
}

// ---------------------------------------------------------------------------
// Rotation estimation from the segmentation mask
// ---------------------------------------------------------------------------

/// The tightest rotated rectangle around a block's ink. `angle_deg` follows
/// the scene `Transform` / CSS convention: clockwise-positive on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotatedRect {
    pub cx: f32,
    pub cy: f32,
    pub width: f32,
    pub height: f32,
    pub angle_deg: f32,
}

/// Too little ink and any angle estimate is noise.
const ROTATION_MIN_INK_PIXELS: usize = 64;
/// Angles below this are treated as straight — keeps ordinary pages
/// byte-identical and avoids jittering every box by fractions of a degree.
const ROTATION_MIN_DEG: f32 = 3.0;
/// Manga text slants; it rarely lies past 45°, and beyond that the
/// line-vs-column ambiguity makes the estimate unreliable.
const ROTATION_MAX_DEG: f32 = 45.0;
/// The best angle's profile score must beat the straight score by this
/// factor, otherwise the block is straight text with a ragged outline.
const ROTATION_SCORE_MARGIN: f32 = 1.2;
/// Profile scoring is O(samples × angles); cap the ink sample count.
const ROTATION_MAX_SAMPLES: usize = 20_000;

/// Estimate the rotation of the text inside `block` from the page-level
/// segmentation probability mask (`pred_mask`, one byte per pixel).
///
/// Uses projection-profile scoring: at the true angle the ink collapses
/// into sharp line/gap bands, which maximises the variance of the profile
/// histogram. This is robust for single slanted lines *and* multi-line
/// paragraphs, and — unlike a min-area rectangle — is not fooled by
/// straight text with ragged line lengths.
///
/// Returns `None` when the block reads as straight (the common case) or
/// there is too little ink to judge.
pub fn estimate_block_rotation(pred_mask: &GrayImage, block: &TextRegion) -> Option<RotatedRect> {
    let [x1, y1, x2, y2] =
        expanded_text_block_crop_bounds(pred_mask.width(), pred_mask.height(), block);
    let mut points: Vec<[f32; 2]> = Vec::new();
    for y in y1..y2 {
        for x in x1..x2 {
            if pred_mask.get_pixel(x, y)[0] > super::BINARY_THRESHOLD {
                points.push([x as f32, y as f32]);
            }
        }
    }
    if points.len() < ROTATION_MIN_INK_PIXELS {
        return None;
    }
    if points.len() > ROTATION_MAX_SAMPLES {
        let stride = points.len().div_ceil(ROTATION_MAX_SAMPLES);
        points = points.iter().step_by(stride).copied().collect();
    }

    // Coarse sweep across the full range, then refine around the winner.
    let straight_score = profile_score(&points, 0.0);
    let mut best_angle = 0.0f32;
    let mut best_score = straight_score;
    let mut deg = -ROTATION_MAX_DEG;
    while deg <= ROTATION_MAX_DEG {
        let score = profile_score(&points, deg);
        if score > best_score {
            best_score = score;
            best_angle = deg;
        }
        deg += 1.5;
    }
    let mut deg = best_angle - 1.25;
    let fine_end = best_angle + 1.25;
    while deg <= fine_end {
        let score = profile_score(&points, deg);
        if score > best_score {
            best_score = score;
            best_angle = deg;
        }
        deg += 0.25;
    }

    if best_angle.abs() < ROTATION_MIN_DEG
        || best_angle.abs() > ROTATION_MAX_DEG
        || best_score < straight_score * ROTATION_SCORE_MARGIN
    {
        return None;
    }

    // Tight rect: extents of the ink in the derotated frame, centre mapped
    // back into image space. +1 accounts for the pixel's own footprint.
    let (sin, cos) = best_angle.to_radians().sin_cos();
    let mut min_u = f32::MAX;
    let mut max_u = f32::MIN;
    let mut min_v = f32::MAX;
    let mut max_v = f32::MIN;
    for [x, y] in &points {
        let u = cos * x + sin * y;
        let v = -sin * x + cos * y;
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    let cu = (min_u + max_u) * 0.5;
    let cv = (min_v + max_v) * 0.5;
    Some(RotatedRect {
        cx: cos * cu - sin * cv,
        cy: sin * cu + cos * cv,
        width: max_u - min_u + 1.0,
        height: max_v - min_v + 1.0,
        angle_deg: best_angle,
    })
}

/// Score how "text-like at angle `deg`" the ink is: derotate the points and
/// take the larger variance of the row / column occupancy histograms. Sharp
/// line (or column) bands with clean gaps → high variance.
fn profile_score(points: &[[f32; 2]], deg: f32) -> f32 {
    let (sin, cos) = deg.to_radians().sin_cos();
    let mut rows: Vec<f32> = Vec::new();
    let mut cols: Vec<f32> = Vec::new();
    let mut min_u = f32::MAX;
    let mut min_v = f32::MAX;
    let derot: Vec<[f32; 2]> = points
        .iter()
        .map(|[x, y]| {
            let u = cos * x + sin * y;
            let v = -sin * x + cos * y;
            min_u = min_u.min(u);
            min_v = min_v.min(v);
            [u, v]
        })
        .collect();
    for [u, v] in derot {
        let col = (u - min_u) as usize;
        let row = (v - min_v) as usize;
        if cols.len() <= col {
            cols.resize(col + 1, 0.0);
        }
        if rows.len() <= row {
            rows.resize(row + 1, 0.0);
        }
        cols[col] += 1.0;
        rows[row] += 1.0;
    }
    f32::max(histogram_variance(&rows), histogram_variance(&cols))
}

fn histogram_variance(bins: &[f32]) -> f32 {
    if bins.is_empty() {
        return 0.0;
    }
    let n = bins.len() as f32;
    let mean = bins.iter().sum::<f32>() / n;
    bins.iter().map(|b| (b - mean) * (b - mean)).sum::<f32>() / n
}

pub fn extract_text_block_regions(image: &DynamicImage, block: &TextRegion) -> Vec<DynamicImage> {
    // Without per-line polygons the whole-block crop must carry the deskew:
    // a rotated block cropped axis-aligned hands the OCR slanted glyphs.
    let Some(line_polygons) = block.line_polygons.as_ref() else {
        return vec![crop_text_block_deskewed(image, block)];
    };
    if line_polygons.is_empty() {
        return vec![crop_text_block_deskewed(image, block)];
    }

    let rgb = image.to_rgb8();
    let mut regions = Vec::with_capacity(line_polygons.len());
    for line in line_polygons {
        if let Some(region) = warp_line_region(&rgb, block, line) {
            regions.push(DynamicImage::ImageRgb8(region));
        }
    }

    if regions.is_empty() {
        vec![crop_text_block_deskewed(image, block)]
    } else {
        regions
    }
}

/// CTD blocks and anything carrying line polygons get expanded crop bounds
/// (line-polygon union plus OCR margin) from
/// [`expanded_text_block_crop_bounds`]; plain detector boxes come back tight.
fn has_expanded_crop_bounds(block: &TextRegion) -> bool {
    block.detector.as_deref() == Some("ctd")
        || block
            .line_polygons
            .as_ref()
            .is_some_and(|lines| !lines.is_empty())
}

/// Margin around a text block for OCR crops: proportional to the detected
/// font size, slightly larger across the text direction than along it.
fn ocr_crop_margin(block: &TextRegion) -> (f32, f32) {
    let font = block
        .detected_font_size_px
        .unwrap_or_else(|| block.width.min(block.height).max(1.0));
    let base_pad = (font * 0.08).max(2.0);
    match block.source_direction.unwrap_or(TextDirection::Horizontal) {
        TextDirection::Horizontal => ((font * 0.12).max(base_pad), (font * 0.18).max(base_pad)),
        TextDirection::Vertical => ((font * 0.18).max(base_pad), (font * 0.12).max(base_pad)),
    }
}

fn clamp_crop_bounds(
    image_width: u32,
    image_height: u32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> [u32; 4] {
    let x1 = min_x
        .floor()
        .clamp(0.0, image_width.saturating_sub(1) as f32) as u32;
    let y1 = min_y
        .floor()
        .clamp(0.0, image_height.saturating_sub(1) as f32) as u32;
    let x2 = max_x.ceil().clamp(x1 as f32 + 1.0, image_width as f32) as u32;
    let y2 = max_y.ceil().clamp(y1 as f32 + 1.0, image_height as f32) as u32;
    [x1, y1, x2, y2]
}

pub fn expanded_text_block_crop_bounds(
    image_width: u32,
    image_height: u32,
    block: &TextRegion,
) -> [u32; 4] {
    if !has_expanded_crop_bounds(block) {
        let x1 = block.x.max(0.0).floor() as u32;
        let y1 = block.y.max(0.0).floor() as u32;
        let x2 = (block.x + block.width)
            .ceil()
            .clamp(x1 as f32 + 1.0, image_width as f32) as u32;
        let y2 = (block.y + block.height)
            .ceil()
            .clamp(y1 as f32 + 1.0, image_height as f32) as u32;
        return [x1, y1, x2, y2];
    }

    let mut min_x = block.x;
    let mut min_y = block.y;
    let mut max_x = block.x + block.width;
    let mut max_y = block.y + block.height;

    if let Some(line_polygons) = block.line_polygons.as_ref() {
        for line in line_polygons {
            let quad = maybe_expand_ctd_line(block, line);
            let bbox = quad_bbox(&quad);
            min_x = min_x.min(bbox[0]);
            min_y = min_y.min(bbox[1]);
            max_x = max_x.max(bbox[2]);
            max_y = max_y.max(bbox[3]);
        }
    }

    let (pad_x, pad_y) = ocr_crop_margin(block);
    clamp_crop_bounds(
        image_width,
        image_height,
        min_x - pad_x,
        min_y - pad_y,
        max_x + pad_x,
        max_y + pad_y,
    )
}

fn warp_line_region(image: &RgbImage, block: &TextRegion, line: &Quad) -> Option<RgbImage> {
    let expanded = maybe_expand_ctd_line(block, line);
    let clipped = clip_quad(&expanded, image.width() as f32, image.height() as f32);
    let bbox = quad_bbox(&clipped);
    let x1 = bbox[0].floor().max(0.0) as u32;
    let y1 = bbox[1].floor().max(0.0) as u32;
    let x2 = bbox[2].ceil().min(image.width() as f32) as u32;
    let y2 = bbox[3].ceil().min(image.height() as f32) as u32;
    if x2 <= x1 || y2 <= y1 {
        return None;
    }

    let cropped = imageops::crop_imm(image, x1, y1, x2 - x1, y2 - y1).to_image();
    let mut src = clipped;
    for point in &mut src {
        point[0] -= x1 as f32;
        point[1] -= y1 as f32;
    }

    let (norm_v, norm_h) = quad_axis_lengths(&src);
    if norm_v <= 0.0 || norm_h <= 0.0 {
        return None;
    }

    let direction = block.source_direction.unwrap_or(TextDirection::Horizontal);
    let text_height = match direction {
        TextDirection::Horizontal => norm_v.max(1.0).round() as u32,
        TextDirection::Vertical => norm_h.max(1.0).round() as u32,
    }
    .max(1);
    let ratio = norm_v / norm_h;

    let (width, height, rotate_vertical) = match direction {
        TextDirection::Horizontal => {
            let h = text_height.max(1);
            let w = ((text_height as f32 / ratio).round() as u32).max(1);
            (w, h, false)
        }
        TextDirection::Vertical => {
            let w = text_height.max(1);
            let h = ((text_height as f32 * ratio).round() as u32).max(1);
            (w, h, true)
        }
    };

    let dst = [
        (0.0f32, 0.0f32),
        ((width.saturating_sub(1)) as f32, 0.0f32),
        (
            (width.saturating_sub(1)) as f32,
            (height.saturating_sub(1)) as f32,
        ),
        (0.0f32, (height.saturating_sub(1)) as f32),
    ];
    let src = quad_to_tuples(&src);
    let projection = Projection::from_control_points(src, dst)?;

    let mut region = RgbImage::from_pixel(width, height, Rgb([0, 0, 0]));
    warp_into(
        &cropped,
        projection,
        Interpolation::Bilinear,
        imageproc::geometric_transformations::Border::Constant(Rgb([0, 0, 0])),
        &mut region,
    );

    if rotate_vertical {
        Some(imageops::rotate270(&region))
    } else {
        Some(region)
    }
}

fn maybe_expand_ctd_line(block: &TextRegion, line: &Quad) -> Quad {
    let should_expand = block.detector.as_deref() == Some("ctd")
        && block.source_direction == Some(TextDirection::Horizontal);
    if !should_expand {
        return *line;
    }

    let expand_size = (block.detected_font_size_px.unwrap_or(0.0) * 0.1).max(3.0);
    let angle = block.rotation_deg.unwrap_or(0.0).to_radians();
    let sin = angle.sin();
    let cos = angle.cos();
    let signs = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

    let mut out = *line;
    for (index, point) in out.iter_mut().enumerate() {
        point[0] += signs[index][0] * sin * expand_size;
        point[1] += signs[index][1] * cos * expand_size;
    }
    out
}

fn clip_quad(quad: &Quad, width: f32, height: f32) -> Quad {
    let mut clipped = *quad;
    for point in &mut clipped {
        point[0] = point[0].clamp(0.0, width);
        point[1] = point[1].clamp(0.0, height);
    }
    clipped
}

fn quad_bbox(quad: &Quad) -> [f32; 4] {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for point in quad {
        min_x = min_x.min(point[0]);
        min_y = min_y.min(point[1]);
        max_x = max_x.max(point[0]);
        max_y = max_y.max(point[1]);
    }
    [min_x, min_y, max_x, max_y]
}

fn quad_to_tuples(quad: &Quad) -> [(f32, f32); 4] {
    [
        (quad[0][0], quad[0][1]),
        (quad[1][0], quad[1][1]),
        (quad[2][0], quad[2][1]),
        (quad[3][0], quad[3][1]),
    ]
}

fn quad_axis_lengths(quad: &Quad) -> (f32, f32) {
    let midpoints = [
        midpoint(quad[0], quad[1]),
        midpoint(quad[1], quad[2]),
        midpoint(quad[2], quad[3]),
        midpoint(quad[3], quad[0]),
    ];
    let vec_v = [
        midpoints[2][0] - midpoints[0][0],
        midpoints[2][1] - midpoints[0][1],
    ];
    let vec_h = [
        midpoints[1][0] - midpoints[3][0],
        midpoints[1][1] - midpoints[3][1],
    ];
    (vector_norm(vec_v), vector_norm(vec_h))
}

fn midpoint(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

fn vector_norm(vector: [f32; 2]) -> f32 {
    (vector[0] * vector[0] + vector[1] * vector[1]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segmentation_completes_anchored_outline_but_preserves_art_and_unseeded_marks() {
        let mut source = RgbImage::from_pixel(100, 100, Rgb([140; 3]));
        let mut seed = GrayImage::new(100, 100);
        // An outlined glyph whose right half segmentation missed.
        for y in 20..50 {
            for x in 20..50 {
                if x < 24 || x >= 46 || y < 24 || y >= 46 {
                    source.put_pixel(x, y, Rgb([0; 3]));
                    if x < 30 {
                        seed.put_pixel(x, y, Luma([255]));
                    }
                }
            }
        }
        // A drawing stroke crosses the box: overlap alone cannot extend it.
        for y in 65..70 {
            for x in 0..100 {
                source.put_pixel(x, y, Rgb([0; 3]));
            }
        }
        seed.put_pixel(30, 67, Luma([255]));
        // An unrelated isolated mark has no detector support.
        for y in 30..36 {
            for x in 70..76 {
                source.put_pixel(x, y, Rgb([0; 3]));
            }
        }
        let completed =
            complete_partial_glyphs(&DynamicImage::ImageRgb8(source), &seed, &[[10, 10, 90, 90]]);
        assert_eq!(completed.get_pixel(48, 32)[0], 255);
        assert_eq!(completed.get_pixel(35, 35)[0], 255); // enclosed counter
        assert_eq!(completed.get_pixel(70, 67)[0], 0);
        assert_eq!(completed.get_pixel(72, 32)[0], 0);
        assert_eq!(completed.get_pixel(30, 67)[0], 255); // original seed kept
    }

    #[test]
    fn segmentation_completes_white_glyphs_without_filling_bright_background() {
        let mut source = RgbImage::from_pixel(80, 80, Rgb([100; 3]));
        let mut seed = GrayImage::new(80, 80);
        for y in 20..40 {
            for x in 20..40 {
                source.put_pixel(x, y, Rgb([255; 3]));
                if x < 25 {
                    seed.put_pixel(x, y, Luma([255]));
                }
            }
        }
        let result =
            complete_partial_glyphs(&DynamicImage::ImageRgb8(source), &seed, &[[10, 10, 70, 70]]);
        assert_eq!(result.get_pixel(38, 30)[0], 255);
        let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(80, 80, Rgb([255; 3])));
        assert_eq!(
            complete_partial_glyphs(&white, &seed, &[[10, 10, 70, 70]]),
            seed
        );
    }

    #[test]
    fn refine_segmentation_mask_erases_when_blocks_are_missing() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(16, 16, Rgb([255, 255, 255])));
        let pred_mask = GrayImage::from_fn(16, 16, |x, y| {
            if (4..12).contains(&x) && (5..11).contains(&y) {
                Luma([200])
            } else {
                Luma([0])
            }
        });

        let mask = refine_segmentation_mask(&image, &pred_mask, &[]);
        assert_eq!(mask.get_pixel(0, 0)[0], 0);
        assert_eq!(mask.get_pixel(8, 8)[0], 0); // No blocks, must be wiped cleanly
    }

    #[test]
    fn refine_segmentation_mask_clips_outside_blocks() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(32, 32, Rgb([255, 255, 255])));
        let pred_mask = GrayImage::from_fn(32, 32, |x, y| {
            if (8..24).contains(&x) && (10..22).contains(&y) {
                Luma([200])
            } else {
                Luma([0])
            }
        });

        let block = TextRegion {
            x: 10.0,
            y: 11.0,
            width: 4.0, // Limits to roughly [10, 11] to [14, 15]
            height: 4.0,
            detected_font_size_px: Some(4.0),
            ..Default::default()
        };

        let mask = refine_segmentation_mask(&image, &pred_mask, &[block]);
        let without_blocks = refine_segmentation_mask(&image, &pred_mask, &[]);

        // Assert providing bounding blocks saves the mask within bounds
        assert_ne!(mask, without_blocks);
        // Assert pixel INSIDE the block is preserved
        assert_eq!(mask.get_pixel(12, 13)[0], 255);
        // Assert pixel OUTSIDE the block (but inside high-prob region) is cleared
        assert_eq!(mask.get_pixel(20, 13)[0], 0);
        // Assert pixel JUST OUTSIDE the block boundary is cleared
        assert_eq!(mask.get_pixel(15, 13)[0], 0);
    }

    #[test]
    fn extract_text_block_regions_falls_back_to_bbox_without_lines() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(24, 24, Rgb([255, 255, 255])));
        let block = TextRegion {
            x: 4.0,
            y: 5.0,
            width: 10.0,
            height: 8.0,
            ..Default::default()
        };

        let regions = extract_text_block_regions(&image, &block);
        assert_eq!(regions.len(), 1);
        // The 10×8 box gains the 2px OCR margin on every side.
        assert_eq!(regions[0].width(), 14);
        assert_eq!(regions[0].height(), 12);
    }

    #[test]
    fn crop_text_block_bbox_pads_plain_detector_boxes() {
        let mut image = RgbImage::from_pixel(24, 24, Rgb([255, 255, 255]));
        // Ink on the box's top-left corner, where a tight crop would leave it
        // touching the border.
        image.put_pixel(4, 5, Rgb([0, 0, 0]));
        let image = DynamicImage::ImageRgb8(image);
        let block = TextRegion {
            x: 4.0,
            y: 5.0,
            width: 10.0,
            height: 8.0,
            detector: Some("comic-text-bubble-detector".to_string()),
            ..Default::default()
        };

        let crop = crop_text_block_bbox(&image, &block).to_rgb8();
        assert_eq!((crop.width(), crop.height()), (14, 12));
        // The corner glyph pixel sits inset by the margin instead of on the
        // crop border.
        assert_eq!(crop.get_pixel(2, 2).0, [0, 0, 0]);
    }

    #[test]
    fn crop_text_block_bbox_expands_ctd_crop() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(48, 48, Rgb([255, 255, 255])));
        let block = TextRegion {
            x: 10.0,
            y: 12.0,
            width: 12.0,
            height: 8.0,
            line_polygons: Some(vec![[
                [10.0, 12.0],
                [22.0, 12.0],
                [22.0, 20.0],
                [10.0, 20.0],
            ]]),
            source_direction: Some(TextDirection::Horizontal),
            rotation_deg: Some(0.0),
            detected_font_size_px: Some(8.0),
            detector: Some("ctd".to_string()),
            ..Default::default()
        };

        let crop = crop_text_block_bbox(&image, &block);
        assert!(crop.width() > 12);
        assert!(crop.height() > 8);
    }

    #[test]
    fn crop_text_block_exact_adds_no_margin() {
        let mut image = RgbImage::from_pixel(24, 24, Rgb([255, 255, 255]));
        image.put_pixel(4, 5, Rgb([0, 0, 0]));
        let image = DynamicImage::ImageRgb8(image);
        // Same plain detector box the bbox path pads to 14×12 above; carrying a
        // line polygon / detector that the caller would otherwise expand.
        let block = TextRegion {
            x: 4.0,
            y: 5.0,
            width: 10.0,
            height: 8.0,
            detector: Some("ctd".to_string()),
            line_polygons: Some(vec![[[4.0, 5.0], [14.0, 5.0], [14.0, 13.0], [4.0, 13.0]]]),
            ..Default::default()
        };

        // Exact crop is the rect itself — no OCR margin, and no expanded-bounds
        // widening despite the ctd detector / line polygon.
        let exact = crop_text_block_exact(&image, &block).to_rgb8();
        assert_eq!((exact.width(), exact.height()), (10, 8));
        // The corner glyph sits on the crop border, exactly where the rect put it.
        assert_eq!(exact.get_pixel(0, 0).0, [0, 0, 0]);
        // The generic crop would have padded it larger.
        let padded = crop_text_block_bbox(&image, &block);
        assert!(padded.width() > exact.width() && padded.height() > exact.height());
    }

    #[test]
    fn crop_text_block_exact_deskews_without_padding() {
        // Black stripe (80×6) rotated 20° about (60, 60) on a white page, as in
        // the deskew test above.
        let mut image = RgbImage::from_pixel(120, 120, Rgb([255, 255, 255]));
        let (sin, cos) = 20.0f32.to_radians().sin_cos();
        for t in -40..40 {
            for d in -3..3 {
                let x = 60.0 + cos * t as f32 - sin * d as f32;
                let y = 60.0 + sin * t as f32 + cos * d as f32;
                if x >= 0.0 && y >= 0.0 && (x as u32) < 120 && (y as u32) < 120 {
                    image.put_pixel(x as u32, y as u32, Rgb([0, 0, 0]));
                }
            }
        }
        let image = DynamicImage::ImageRgb8(image);
        let block = TextRegion {
            x: 22.0,
            y: 54.0,
            width: 76.0,
            height: 12.0,
            rotation_deg: Some(20.0),
            ..Default::default()
        };

        let crop = crop_text_block_exact(&image, &block).to_rgb8();
        // Output is exactly the rect size: deskewed, but with no extra margin
        // (the deskew path would have padded it to 92×28).
        assert_eq!((crop.width(), crop.height()), (76, 12));
        // The stripe is upright across the midline.
        let mid_y = crop.height() / 2;
        assert!(crop.get_pixel(crop.width() / 2, mid_y)[0] < 100);
    }

    /// Paint `lines` parallel "text lines" of ink rotated by `deg` (clockwise,
    /// screen convention) around `centre` into a fresh mask.
    fn slanted_lines_mask(size: u32, centre: f32, deg: f32, lines: i32) -> GrayImage {
        let mut mask = GrayImage::new(size, size);
        let (sin, cos) = deg.to_radians().sin_cos();
        for line in 0..lines {
            let offset = (line - lines / 2) as f32 * 14.0;
            for t in -60..60 {
                for d in 0..4 {
                    let lx = t as f32;
                    let ly = offset + d as f32;
                    let x = centre + cos * lx - sin * ly;
                    let y = centre + sin * lx + cos * ly;
                    if x >= 0.0 && y >= 0.0 && (x as u32) < size && (y as u32) < size {
                        mask.put_pixel(x as u32, y as u32, Luma([255]));
                    }
                }
            }
        }
        mask
    }

    fn full_block(size: u32) -> TextRegion {
        TextRegion {
            x: 4.0,
            y: 4.0,
            width: size as f32 - 8.0,
            height: size as f32 - 8.0,
            ..Default::default()
        }
    }

    #[test]
    fn estimate_block_rotation_recovers_slanted_lines() {
        let mask = slanted_lines_mask(200, 100.0, 12.0, 3);
        let rect = estimate_block_rotation(&mask, &full_block(200))
            .expect("slanted lines should yield a rotation");
        assert!(
            (rect.angle_deg - 12.0).abs() <= 1.5,
            "angle {} not near 12°",
            rect.angle_deg
        );
        // Tight rect hugs the ink: ~120 long, 3 lines spanning ~32 tall.
        assert!((rect.width - 120.0).abs() < 10.0, "width {}", rect.width);
        assert!((rect.height - 32.0).abs() < 8.0, "height {}", rect.height);
        assert!((rect.cx - 100.0).abs() < 4.0);
        assert!((rect.cy - 100.0).abs() < 4.0);
    }

    #[test]
    fn estimate_block_rotation_leaves_straight_text_alone() {
        let mask = slanted_lines_mask(200, 100.0, 0.0, 3);
        assert_eq!(estimate_block_rotation(&mask, &full_block(200)), None);
        // Near-straight raggedness stays below the reporting threshold too.
        let mask = slanted_lines_mask(200, 100.0, 1.0, 3);
        assert_eq!(estimate_block_rotation(&mask, &full_block(200)), None);
    }

    #[test]
    fn estimate_block_rotation_needs_enough_ink() {
        let mut mask = GrayImage::new(64, 64);
        for x in 20..40 {
            mask.put_pixel(x, 30, Luma([255]));
        }
        assert_eq!(estimate_block_rotation(&mask, &full_block(64)), None);
    }

    #[test]
    fn crop_text_block_deskewed_uprights_a_rotated_stripe() {
        // Black stripe (80×6) rotated 20° about (60, 60) on a white page.
        let mut image = RgbImage::from_pixel(120, 120, Rgb([255, 255, 255]));
        let (sin, cos) = 20.0f32.to_radians().sin_cos();
        for t in -40..40 {
            for d in -3..3 {
                let x = 60.0 + cos * t as f32 - sin * d as f32;
                let y = 60.0 + sin * t as f32 + cos * d as f32;
                if x >= 0.0 && y >= 0.0 && (x as u32) < 120 && (y as u32) < 120 {
                    image.put_pixel(x as u32, y as u32, Rgb([0, 0, 0]));
                }
            }
        }
        let image = DynamicImage::ImageRgb8(image);
        // Block sits around the stripe with a little margin, like a detector
        // box would.
        let block = TextRegion {
            x: 22.0,
            y: 54.0,
            width: 76.0,
            height: 12.0,
            rotation_deg: Some(20.0),
            ..Default::default()
        };

        let crop = crop_text_block_deskewed(&image, &block).to_rgb8();
        let mid_y = crop.height() / 2;
        // The stripe lies flat across the whole midline — including both
        // ends, which an axis-aligned crop of rotated art would miss.
        for x in [4, crop.width() / 2, crop.width() - 5] {
            assert!(
                crop.get_pixel(x, mid_y)[0] < 100,
                "midline at x={x} should be ink"
            );
        }
        // Corners are background again.
        assert!(crop.get_pixel(1, 1)[0] > 200);
        assert!(crop.get_pixel(crop.width() - 2, crop.height() - 2)[0] > 200);

        // A straight block goes through the bbox crop, which adds the OCR
        // margin (2px along the text, 2.16px across it for a 12px font).
        let straight = TextRegion {
            rotation_deg: Some(0.0),
            ..block.clone()
        };
        let plain = crop_text_block_deskewed(&image, &straight);
        assert_eq!(plain.width(), 80);
        assert_eq!(plain.height(), 18);
    }
}
