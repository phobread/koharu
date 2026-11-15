//! Keeps coloured decorations, such as hand-drawn pink hearts, out of a
//! clean-up's erase mask when the lettering beside them is dark.
//!
//! The erase mask of a full clean-up run reaches past the lettering: the
//! segmenter often marks a heart drawn next to the text as text, every mask
//! pixel grows by up to 8px, and a box with no mask under it is erased as a
//! whole rectangle inside its bubble (`mask::expand_glyph_mask`). Hearts
//! drawn beside an SFX or a line of dialogue were erased whole, or cut
//! along the rectangle's edge (LOLExtras 0084, 0100, Neeko's Help 0012).
//!
//! A text box's area (the box and the reach of the mask growth around it)
//! takes part when its lettering is dark:
//! - The erase mask there must hold dark lettering, and no more than
//!   [`MAX_COLOUR_PER_DARK`] times as many saturated pixels. Lettering that
//!   is itself coloured is erased as before.
//! - Little of the area outside the erase mask may be saturated, or the
//!   colour is the background (black text on a pink bubble, narration over
//!   red hair), and keeping it would leave the letters' coloured edges.
//! - Little of it may be dark either, or the dark pixels in the mask are a
//!   dark background around light lettering (Ruination's white-on-black
//!   boxes), not lettering.
//!
//! Saturated shapes that reach the erase mask inside such an area are kept
//! whole, also where they stick out of it into a neighbouring box's area
//! (cutting them there left a straight edge through a heart). A shape mostly
//! ringed by dark pixels is the fill of an outlined letter and is not kept.
//! Around a kept shape, [`KEEP_GROW_PX`] of light pixels (its soft edge on a
//! pale background) stay too; darker pixels there are lettering touching it
//! and are erased.

use image::{DynamicImage, GrayImage, Luma, RgbImage};
use imageproc::{
    distance_transform::Norm,
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};

use crate::{comic_text_detector::expanded_text_block_crop_bounds, types::TextRegion};

/// How far past a box the erase mask can reach: the 8px growth plus a little.
const AREA_MARGIN_PX: u32 = 12;
/// A dark pixel: no channel above this, and nearly grey.
const DARK_MAX_LEVEL: u8 = 130;
const DARK_MAX_CHROMA: u8 = 50;
/// A saturated pixel: channels this far apart, at least this share of the
/// brightest channel (HSV saturation), and that channel at least this
/// bright. A pink heart is about 200 apart, 0.85 and 230; skin tones, a
/// heart's pale edge and the dark mix where a black stroke crosses a heart
/// are not.
const SATURATED_MIN_CHROMA: u8 = 80;
const SATURATED_MIN_SHARE: f32 = 0.4;
const SATURATED_MIN_LEVEL: u8 = 140;
/// Dark pixels an area's erase mask must hold before its lettering counts
/// as dark.
const MIN_DARK_PX: usize = 40;
/// Saturated pixels allowed per dark pixel in an area's erase mask. Hearts
/// can outnumber the letters of a short moan (LOLExtras 0100 "아힛 앙": 887
/// saturated against 754 dark).
const MAX_COLOUR_PER_DARK: usize = 4;
/// Share of the area outside the erase mask that may be saturated. Real
/// hearts beside dialogue stay under 0.15.
const MAX_SURROUNDING_COLOUR_SHARE: f32 = 0.35;
/// Share of the area outside the erase mask that may be dark. Beside dark
/// lettering on LOLExtras, Neeko's Help and One Piece pages it stays under
/// 0.4; around Ruination's white-on-black lettering it is 0.83 to 1.
const MAX_SURROUNDING_DARK_SHARE: f32 = 0.5;
/// A saturated shape whose ring is more dark than this is a letter's fill.
const MAX_RING_DARK_SHARE: f32 = 0.5;
const RING_PX: i32 = 2;
/// Saturated shapes smaller than this are noise.
const MIN_SHAPE_PX: usize = 6;
const KEEP_GROW_PX: u8 = 2;
/// Pixels around a kept shape stay only when some channel is this bright.
const EDGE_MIN_LEVEL: u8 = 160;
/// How far a brush stroke's erase reaches past what it painted: the hand
/// stroke's growth (`mask::expand_hand_stroke_mask_for_inpainting`).
const PAINTED_GROW_PX: u8 = 2;

fn chroma([r, g, b]: [u8; 3]) -> u8 {
    r.max(g).max(b) - r.min(g).min(b)
}

fn level([r, g, b]: [u8; 3]) -> u8 {
    r.max(g).max(b)
}

fn is_dark(p: [u8; 3]) -> bool {
    level(p) <= DARK_MAX_LEVEL && chroma(p) < DARK_MAX_CHROMA
}

fn is_saturated(p: [u8; 3]) -> bool {
    let c = chroma(p);
    let max = level(p);
    c >= SATURATED_MIN_CHROMA
        && max >= SATURATED_MIN_LEVEL
        && f32::from(c) >= f32::from(max) * SATURATED_MIN_SHARE
}

/// `erase` without the coloured decorations next to dark lettering in the
/// areas of `text_blocks`. `image` is the page being cleaned, the same size.
pub fn keep_coloured_decorations(
    erase: &GrayImage,
    image: &DynamicImage,
    text_blocks: &[TextRegion],
) -> GrayImage {
    let mut out = erase.clone();
    if image.width() != erase.width() || image.height() != erase.height() {
        return out;
    }
    let rgb = image.to_rgb8();
    let (width, height) = erase.dimensions();
    let areas: Vec<[u32; 4]> = text_blocks
        .iter()
        .filter_map(|block| block_area(block, width, height))
        .filter(|area| lettering_is_dark(erase, &rgb, *area))
        .collect();
    if areas.is_empty() {
        return out;
    }

    let saturated = GrayImage::from_fn(width, height, |x, y| {
        Luma([if is_saturated(rgb.get_pixel(x, y).0) {
            255
        } else {
            0
        }])
    });
    let labels = connected_components(&saturated, Connectivity::Eight, Luma([0u8]));
    let count = labels.pixels().map(|p| p.0[0]).max().unwrap_or(0) as usize;
    if count == 0 {
        return out;
    }

    // Shapes that reach the erase mask inside a taking-part area.
    let reach = i32::from(KEEP_GROW_PX);
    let mut candidate = vec![false; count + 1];
    for &[x0, y0, x1, y1] in &areas {
        for y in y0..y1 {
            for x in x0..x1 {
                if erase.get_pixel(x, y).0[0] == 0 {
                    continue;
                }
                for dy in -reach..=reach {
                    for dx in -reach..=reach {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx >= 0 && ny >= 0 && nx < width as i32 && ny < height as i32 {
                            let label = labels.get_pixel(nx as u32, ny as u32).0[0] as usize;
                            if label > 0 {
                                candidate[label] = true;
                            }
                        }
                    }
                }
            }
        }
    }
    let mut shape_pixels = vec![Vec::new(); count + 1];
    for (x, y, label) in labels.enumerate_pixels() {
        if candidate[label.0[0] as usize] {
            shape_pixels[label.0[0] as usize].push((x, y));
        }
    }

    // Shapes are walked one at a time, so each ring pixel is counted once.
    let mut counted_for = vec![0u32; (width * height) as usize];
    let mut kept = vec![false; count + 1];
    for (l, pixels) in shape_pixels.iter().enumerate().skip(1) {
        if pixels.len() < MIN_SHAPE_PX {
            continue;
        }
        let label = l as u32;
        let (mut ring, mut ring_dark) = (0usize, 0usize);
        for &(x, y) in pixels {
            for dy in -RING_PX..=RING_PX {
                for dx in -RING_PX..=RING_PX {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let (nx, ny) = (nx as u32, ny as u32);
                    let index = (ny * width + nx) as usize;
                    if labels.get_pixel(nx, ny).0[0] == label || counted_for[index] == label {
                        continue;
                    }
                    counted_for[index] = label;
                    ring += 1;
                    if is_dark(rgb.get_pixel(nx, ny).0) {
                        ring_dark += 1;
                    }
                }
            }
        }
        kept[l] = ring_dark as f32 <= ring as f32 * MAX_RING_DARK_SHARE;
    }
    if !kept.iter().any(|k| *k) {
        return out;
    }

    let shapes = GrayImage::from_fn(width, height, |x, y| {
        Luma([if kept[labels.get_pixel(x, y).0[0] as usize] {
            255
        } else {
            0
        }])
    });
    let grown = dilate(&shapes, Norm::LInf, KEEP_GROW_PX);
    let mut kept_pixels = 0usize;
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        if pixel.0[0] == 0 || grown.get_pixel(x, y).0[0] == 0 {
            continue;
        }
        let shape = shapes.get_pixel(x, y).0[0] > 0;
        if shape || level(rgb.get_pixel(x, y).0) >= EDGE_MIN_LEVEL {
            pixel.0[0] = 0;
            kept_pixels += 1;
        }
    }
    tracing::info!(
        areas = areas.len(),
        shapes = kept.iter().filter(|k| **k).count(),
        kept_pixels,
        "kept coloured decorations out of the erase mask"
    );
    out
}

/// [`keep_coloured_decorations`] for a repair-brush stroke: whatever the
/// stroke painted (`painted`, the mask pixels it added) is erased even where
/// it is a decoration, with the stroke's own [`PAINTED_GROW_PX`] growth.
/// Decorations it didn't paint over stay, also when the stroke redoes a whole
/// piece of lettering whose mask runs over a heart.
pub fn keep_coloured_decorations_except(
    erase: &GrayImage,
    image: &DynamicImage,
    text_blocks: &[TextRegion],
    painted: &GrayImage,
) -> GrayImage {
    let kept = keep_coloured_decorations(erase, image, text_blocks);
    if painted.dimensions() != erase.dimensions() || kept == *erase {
        return kept;
    }
    let painted = GrayImage::from_fn(painted.width(), painted.height(), |x, y| {
        Luma([if painted.get_pixel(x, y).0[0] > 127 {
            255
        } else {
            0
        }])
    });
    let near = dilate(&painted, Norm::LInf, PAINTED_GROW_PX);
    GrayImage::from_fn(erase.width(), erase.height(), |x, y| {
        let on = kept.get_pixel(x, y).0[0] > 0
            || (erase.get_pixel(x, y).0[0] > 0 && near.get_pixel(x, y).0[0] > 0);
        Luma([if on { 255 } else { 0 }])
    })
}

/// The part of the page a box's erase mask can reach, or `None` when the
/// box is off the page.
fn block_area(block: &TextRegion, width: u32, height: u32) -> Option<[u32; 4]> {
    if !(block.x < width as f32
        && block.y < height as f32
        && block.x + block.width > 0.0
        && block.y + block.height > 0.0)
    {
        return None;
    }
    let [x1, y1, x2, y2] = expanded_text_block_crop_bounds(width, height, block);
    let area = [
        x1.saturating_sub(AREA_MARGIN_PX),
        y1.saturating_sub(AREA_MARGIN_PX),
        (x2 + AREA_MARGIN_PX).min(width),
        (y2 + AREA_MARGIN_PX).min(height),
    ];
    (area[0] < area[2] && area[1] < area[3]).then_some(area)
}

/// Whether the erase mask in `area` holds dark lettering with coloured
/// shapes beside it, on surroundings that are neither coloured nor dark.
fn lettering_is_dark(erase: &GrayImage, rgb: &RgbImage, [x0, y0, x1, y1]: [u32; 4]) -> bool {
    let (mut dark, mut colour) = (0usize, 0usize);
    let (mut outside, mut outside_colour, mut outside_dark) = (0usize, 0usize, 0usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = rgb.get_pixel(x, y).0;
            if erase.get_pixel(x, y).0[0] > 0 {
                if is_dark(p) {
                    dark += 1;
                } else if is_saturated(p) {
                    colour += 1;
                }
            } else {
                outside += 1;
                if is_saturated(p) {
                    outside_colour += 1;
                } else if is_dark(p) {
                    outside_dark += 1;
                }
            }
        }
    }
    let taking_part = colour > 0
        && dark >= MIN_DARK_PX
        && colour <= dark * MAX_COLOUR_PER_DARK
        && outside_colour as f32 <= outside as f32 * MAX_SURROUNDING_COLOUR_SHARE
        && outside_dark as f32 <= outside as f32 * MAX_SURROUNDING_DARK_SHARE;
    if colour > 0 {
        tracing::debug!(
            area = ?[x0, y0, x1, y1],
            dark,
            colour,
            outside,
            outside_colour,
            outside_dark,
            taking_part,
            "coloured decorations: box area"
        );
    }
    taking_part
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    const WHITE: [u8; 3] = [252, 252, 252];
    const BLACK: [u8; 3] = [20, 20, 20];
    const PINK: [u8; 3] = [230, 40, 140];

    fn page(w: u32, h: u32, background: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb(background))
    }

    fn fill(image: &mut RgbImage, [x0, y0, x1, y1]: [u32; 4], colour: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                image.put_pixel(x, y, Rgb(colour));
            }
        }
    }

    fn mask(w: u32, h: u32, rects: &[[u32; 4]]) -> GrayImage {
        let mut m = GrayImage::new(w, h);
        for &[x0, y0, x1, y1] in rects {
            for y in y0..y1 {
                for x in x0..x1 {
                    m.put_pixel(x, y, Luma([255]));
                }
            }
        }
        m
    }

    fn block(x: f32, y: f32, width: f32, height: f32) -> TextRegion {
        TextRegion {
            x,
            y,
            width,
            height,
            ..Default::default()
        }
    }

    /// Black strokes with a pink heart beside them, all inside an erased
    /// rectangle (the no-mask box case).
    fn sfx_with_heart() -> (RgbImage, GrayImage, TextRegion) {
        let mut image = page(200, 200, WHITE);
        fill(&mut image, [60, 60, 70, 140], BLACK);
        fill(&mut image, [80, 60, 90, 140], BLACK);
        fill(&mut image, [110, 100, 130, 120], PINK);
        let erase = mask(200, 200, &[[50, 50, 150, 150]]);
        (image, erase, block(58.0, 58.0, 84.0, 84.0))
    }

    #[test]
    fn a_pink_heart_beside_dark_lettering_is_kept() {
        let (image, erase, block) = sfx_with_heart();
        let out = keep_coloured_decorations(&erase, &DynamicImage::ImageRgb8(image), &[block]);
        // The heart and its soft edge stay.
        assert_eq!(out.get_pixel(120, 110)[0], 0);
        assert_eq!(out.get_pixel(108, 110)[0], 0);
        // The lettering and the rest of the rectangle are still erased.
        assert_eq!(out.get_pixel(65, 100)[0], 255);
        assert_eq!(out.get_pixel(100, 70)[0], 255);
        assert_eq!(out.get_pixel(140, 140)[0], 255);
    }

    #[test]
    fn lettering_crossing_a_heart_is_still_erased() {
        let (mut image, erase, block) = sfx_with_heart();
        fill(&mut image, [118, 95, 121, 125], BLACK);
        let out = keep_coloured_decorations(&erase, &DynamicImage::ImageRgb8(image), &[block]);
        assert_eq!(out.get_pixel(119, 110)[0], 255);
        assert_eq!(out.get_pixel(125, 110)[0], 0);
    }

    #[test]
    fn coloured_lettering_is_erased() {
        // Pink letters, no dark ones.
        let mut image = page(200, 200, WHITE);
        fill(&mut image, [60, 60, 70, 140], PINK);
        fill(&mut image, [80, 60, 90, 140], PINK);
        let erase = mask(200, 200, &[[50, 50, 150, 150]]);
        let out = keep_coloured_decorations(
            &erase,
            &DynamicImage::ImageRgb8(image),
            &[block(58.0, 58.0, 84.0, 84.0)],
        );
        assert_eq!(out, erase);
    }

    #[test]
    fn the_fill_of_an_outlined_letter_is_erased() {
        let mut image = page(200, 200, WHITE);
        // A black-outlined pink bar, plus a black letter for dark lettering.
        fill(&mut image, [60, 60, 90, 140], BLACK);
        fill(&mut image, [63, 63, 87, 137], PINK);
        fill(&mut image, [110, 60, 120, 140], BLACK);
        let erase = mask(200, 200, &[[50, 50, 150, 150]]);
        let out = keep_coloured_decorations(
            &erase,
            &DynamicImage::ImageRgb8(image),
            &[block(58.0, 58.0, 84.0, 84.0)],
        );
        assert_eq!(out, erase);
    }

    #[test]
    fn a_coloured_background_is_not_kept() {
        // Black text on a pink bubble: the pink around the letters is the
        // background.
        let mut image = page(200, 200, PINK);
        fill(&mut image, [60, 60, 70, 140], BLACK);
        fill(&mut image, [80, 60, 90, 140], BLACK);
        let erase = mask(200, 200, &[[56, 56, 74, 144], [76, 56, 94, 144]]);
        let out = keep_coloured_decorations(
            &erase,
            &DynamicImage::ImageRgb8(image),
            &[block(58.0, 58.0, 34.0, 84.0)],
        );
        assert_eq!(out, erase);
    }

    #[test]
    fn nothing_outside_the_boxes_changes() {
        // The same heart and lettering, but no text box near them.
        let (image, erase, _) = sfx_with_heart();
        let out = keep_coloured_decorations(
            &erase,
            &DynamicImage::ImageRgb8(image),
            &[block(170.0, 170.0, 20.0, 20.0)],
        );
        assert_eq!(out, erase);
    }

    #[test]
    fn a_heart_reaching_past_the_box_area_is_kept_whole() {
        // The erase mask runs on past the box's area (a neighbouring box's
        // growth); the heart straddles the area's edge at x = 150.
        let mut image = page(220, 200, WHITE);
        fill(&mut image, [60, 60, 70, 140], BLACK);
        fill(&mut image, [80, 60, 90, 140], BLACK);
        fill(&mut image, [140, 100, 170, 120], PINK);
        let erase = mask(220, 200, &[[40, 50, 200, 150]]);
        let out = keep_coloured_decorations(
            &erase,
            &DynamicImage::ImageRgb8(image),
            &[block(58.0, 58.0, 80.0, 84.0)],
        );
        assert_eq!(out.get_pixel(145, 110)[0], 0);
        assert_eq!(out.get_pixel(165, 110)[0], 0);
        // The mask beyond the heart's edge stays.
        assert_eq!(out.get_pixel(185, 110)[0], 255);
    }

    #[test]
    fn only_the_light_edge_around_a_heart_is_kept() {
        let (mut image, erase, block) = sfx_with_heart();
        // Grey anti-aliasing of a letter touching the heart's left side, and
        // the heart's own pale edge on its right.
        fill(&mut image, [108, 100, 110, 120], [150, 150, 150]);
        fill(&mut image, [130, 100, 131, 120], [245, 170, 205]);
        let out = keep_coloured_decorations(&erase, &DynamicImage::ImageRgb8(image), &[block]);
        assert_eq!(out.get_pixel(109, 110)[0], 255);
        assert_eq!(out.get_pixel(130, 110)[0], 0);
    }

    #[test]
    fn light_lettering_on_a_dark_background_is_left_as_it_was() {
        // White letters in a black box, a teal line running under them.
        let mut image = page(200, 200, BLACK);
        fill(&mut image, [60, 60, 70, 140], WHITE);
        fill(&mut image, [80, 60, 90, 140], WHITE);
        fill(&mut image, [40, 100, 160, 103], [40, 200, 170]);
        let erase = mask(200, 200, &[[50, 50, 150, 150]]);
        let out = keep_coloured_decorations(
            &erase,
            &DynamicImage::ImageRgb8(image),
            &[block(58.0, 58.0, 84.0, 84.0)],
        );
        assert_eq!(out, erase);
    }

    #[test]
    fn a_stroke_painted_over_a_heart_erases_it() {
        let (image, erase, block) = sfx_with_heart();
        // The stroke painted the heart's left half.
        let painted = mask(200, 200, &[[110, 100, 120, 120]]);
        let out = keep_coloured_decorations_except(
            &erase,
            &DynamicImage::ImageRgb8(image),
            &[block],
            &painted,
        );
        assert_eq!(out.get_pixel(115, 110)[0], 255);
        assert_eq!(out.get_pixel(121, 110)[0], 255);
        // The rest of the heart stays.
        assert_eq!(out.get_pixel(126, 110)[0], 0);
    }

    #[test]
    fn a_stroke_beside_a_heart_keeps_it() {
        let (image, erase, block) = sfx_with_heart();
        // The stroke painted only the lettering.
        let painted = mask(200, 200, &[[60, 60, 70, 140]]);
        let out = keep_coloured_decorations_except(
            &erase,
            &DynamicImage::ImageRgb8(image),
            &[block],
            &painted,
        );
        assert_eq!(out.get_pixel(120, 110)[0], 0);
        assert_eq!(out.get_pixel(65, 100)[0], 255);
    }
}
