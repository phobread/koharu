//! Flat caption frames the bubble detector missed.
//!
//! The bubble segmenter finds speech bubbles, but some flat caption frames
//! (black narration boxes with white lettering) slip past it, so the text in
//! them keeps its box and comes out smaller than the frame allows. For a text
//! box on no detected bubble, [`add_flat_frames`] looks at the cleaned page
//! under it: when the box sits on one flat colour that fills a closed patch
//! with straight sides and a clear edge all round, and the patch is not much
//! bigger than the box, the patch is painted into the bubble map as one more
//! bubble. Everything that reads the map then treats it like a bubble.
//!
//! Text over artwork stays as it is: the cleaned page under such a box is not
//! one flat colour, or the colour runs on into a patch that is open to the
//! page edge, too big, curved or ragged, or fading out instead of ending at
//! an edge.
//!
//! `RUST_LOG=koharu_renderer::text::frames=debug` logs each box's verdict.

use image::{GrayImage, Luma, RgbaImage};
use imageproc::distance_transform::Norm;
use imageproc::morphology::{dilate, open};
use imageproc::region_labelling::{Connectivity, connected_components};

use super::latin::LayoutBox;

/// Most a pixel may differ from the frame's colour (largest channel
/// difference) and still be part of the frame.
const FLAT_TOLERANCE: u8 = 24;
/// Share of a text box that must be the frame's colour on the cleaned page.
const MIN_FLAT_SHARE: f32 = 0.9;
/// Largest frame, in text-box areas. A caption frame is drawn round its
/// lettering; a much bigger flat patch is a plain panel or background, and
/// text laid out to fill it would come out far too big.
const MAX_FRAME_TO_BOX: f32 = 8.0;
/// The colour is followed this much further before giving up on a patch, so
/// that a thin leak (a panel border touching the frame) can be cut off.
const FLOOD_TO_FRAME: f32 = 4.0;
/// Least share of a frame's outline on straight level or upright sides
/// (rectangles, and frames overlapping each other like steps; art drawn
/// over a frame's corner bends part of its outline). On the owner's pages
/// the caption frames measured 0.74-0.99, flat patches of artwork 0.07-0.49.
const MIN_STRAIGHT: f32 = 0.65;
/// Least share of the text box that lies in the frame.
const MIN_BOX_INSIDE: f32 = 0.95;
/// A pixel differs clearly from the frame from this difference on.
const EDGE_CONTRAST: u8 = 48;
/// How far outside the frame (pixels) such a pixel may be: a thin rim or
/// the art the frame is drawn over.
const EDGE_REACH: u8 = 3;
/// Least share of the frame's outline with a clearly different pixel
/// within `EDGE_REACH`: the frame ends at an edge rather than fading out.
const MIN_EDGE_SHARE: f32 = 0.8;
/// Most of the frame that may be holes (leftover specks, a heart).
const MAX_HOLE_SHARE: f32 = 0.1;
/// Text boxes narrower or lower than this are not looked at.
const MIN_SIDE: u32 = 8;

/// A caption frame found under a text box.
#[derive(Clone, Debug)]
struct FlatFrame {
    /// The frame's colour (median of the text box on the cleaned page).
    colour: [u8; 3],
    /// Bounding rectangle of the frame in page pixels: x, y, width, height.
    bounds: (u32, u32, u32, u32),
    /// Frame pixels (holes filled), row by row inside `bounds`.
    pixels: Vec<bool>,
}

/// Why a text box got no frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoFrame {
    /// The box is tiny or off the page.
    TooSmall,
    /// The cleaned page under the box is not one flat colour.
    NotFlat,
    /// The colour runs on much further than a frame would (background).
    Unbounded,
    /// The patch is open to the page edge.
    PageEdge,
    /// The patch is much bigger than the box.
    TooBig,
    /// The patch's outline is mostly curved or ragged, not straight sides.
    Shape,
    /// The box is not inside the patch.
    BoxOutside,
    /// The patch fades out instead of ending at an edge.
    NoEdge,
    /// The patch has big holes (something drawn inside it).
    Holes,
}

/// For each text box in `boxes` (page pixels) that sits on no bubble in the
/// ID map `bubbles` but on a flat frame in the cleaned `page`, paints that
/// frame into `bubbles` under a new id (the next unused one). Returns the id
/// of the frame each box ended up on, `None` for boxes left as they are.
/// Boxes that already touch a bubble are skipped; a box inside a frame found
/// for an earlier box shares it. Nothing changes when the map and the page
/// differ in size or no id is left.
pub fn add_flat_frames(
    bubbles: &mut GrayImage,
    page: &RgbaImage,
    boxes: &[LayoutBox],
) -> Vec<Option<u8>> {
    let mut found = vec![None; boxes.len()];
    if bubbles.dimensions() != page.dimensions() {
        return found;
    }
    // Highest id in use, looked up once the first frame is found.
    let mut highest: Option<u8> = None;
    let mut painted: Vec<u8> = Vec::new();
    for (slot, &seed) in found.iter_mut().zip(boxes) {
        if let Some(id) = bubble_under(bubbles, seed) {
            if painted.contains(&id) {
                *slot = Some(id);
            }
            continue;
        }
        let frame = match find_flat_frame(page, bubbles, seed) {
            Ok(frame) => frame,
            Err(why) => {
                tracing::debug!(?seed, ?why, "no caption frame under the box");
                continue;
            }
        };
        let in_use =
            *highest.get_or_insert_with(|| bubbles.pixels().map(|p| p.0[0]).max().unwrap_or(0));
        let Some(id) = in_use.checked_add(1) else {
            break;
        };
        highest = Some(id);
        tracing::debug!(
            ?seed,
            id,
            colour = ?frame.colour,
            bounds = ?frame.bounds,
            "caption frame under the box"
        );
        paint_frame(bubbles, &frame, id);
        painted.push(id);
        *slot = Some(id);
    }
    found
}

/// The bubble covering most of `seed`, if any covers some of it.
fn bubble_under(bubbles: &GrayImage, seed: LayoutBox) -> Option<u8> {
    let (x0, y0, x1, y1) = clamp_box(seed, bubbles.width(), bubbles.height())?;
    let mut counts = [0u32; 256];
    for y in y0..y1 {
        for x in x0..x1 {
            counts[bubbles.get_pixel(x, y).0[0] as usize] += 1;
        }
    }
    (1..=255u8)
        .filter(|&id| counts[id as usize] > 0)
        .max_by_key(|&id| counts[id as usize])
}

fn paint_frame(bubbles: &mut GrayImage, frame: &FlatFrame, id: u8) {
    let (fx, fy, fw, _) = frame.bounds;
    for (i, _) in frame
        .pixels
        .iter()
        .enumerate()
        .filter(|(_, inside)| **inside)
    {
        let (x, y) = (fx + i as u32 % fw, fy + i as u32 / fw);
        let pixel = bubbles.get_pixel_mut(x, y);
        if pixel.0[0] == 0 {
            pixel.0[0] = id;
        }
    }
}

/// `seed` as whole pixels inside a `width` x `height` page:
/// `(x0, y0, x1, y1)`, end exclusive. `None` when nothing is left.
fn clamp_box(seed: LayoutBox, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let x0 = seed.x.round().max(0.0) as u32;
    let y0 = seed.y.round().max(0.0) as u32;
    let x1 = ((seed.x + seed.width).round().max(0.0) as u32).min(width);
    let y1 = ((seed.y + seed.height).round().max(0.0) as u32).min(height);
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

fn difference(a: [u8; 3], b: [u8; 3]) -> u8 {
    (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0)
}

fn rgb(page: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = page.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

/// The caption frame under `seed`, or why there is none.
fn find_flat_frame(
    page: &RgbaImage,
    bubbles: &GrayImage,
    seed: LayoutBox,
) -> Result<FlatFrame, NoFrame> {
    let (width, height) = page.dimensions();
    let (bx0, by0, bx1, by1) = clamp_box(seed, width, height).ok_or(NoFrame::TooSmall)?;
    if bx1 - bx0 < MIN_SIDE || by1 - by0 < MIN_SIDE {
        return Err(NoFrame::TooSmall);
    }
    let box_area = ((bx1 - bx0) * (by1 - by0)) as usize;

    // The frame's colour: the median of the box, channel by channel.
    let mut histograms = [[0u32; 256]; 3];
    for y in by0..by1 {
        for x in bx0..bx1 {
            let p = rgb(page, x, y);
            for c in 0..3 {
                histograms[c][p[c] as usize] += 1;
            }
        }
    }
    let mut colour = [0u8; 3];
    for c in 0..3 {
        let mut seen = 0;
        for (value, &count) in histograms[c].iter().enumerate() {
            seen += count as usize;
            if seen * 2 >= box_area {
                colour[c] = value as u8;
                break;
            }
        }
    }
    let flat = |x: u32, y: u32| {
        bubbles.get_pixel(x, y).0[0] == 0 && difference(rgb(page, x, y), colour) <= FLAT_TOLERANCE
    };
    let flat_count = (by0..by1)
        .flat_map(|y| (bx0..bx1).map(move |x| (x, y)))
        .filter(|&(x, y)| flat(x, y))
        .count();
    if (flat_count as f32) < MIN_FLAT_SHARE * box_area as f32 {
        return Err(NoFrame::NotFlat);
    }

    // Follow the colour out from the box (4-connected), giving up once it
    // covers far more than a frame would, or more than a frame may and it
    // has reached the page edge (a plain background, the common case).
    let frame_cap = (box_area as f32 * MAX_FRAME_TO_BOX) as usize;
    let cap = (frame_cap as f32 * FLOOD_TO_FRAME) as usize;
    let mut at_page_edge = false;
    let mut seen = vec![false; (width * height) as usize];
    let mut region = Vec::new();
    for y in by0..by1 {
        for x in bx0..bx1 {
            let i = (y * width + x) as usize;
            if !seen[i] && flat(x, y) {
                seen[i] = true;
                region.push(i as u32);
            }
        }
    }
    let mut next = 0;
    while next < region.len() {
        if region.len() > cap {
            return Err(NoFrame::Unbounded);
        }
        if at_page_edge && region.len() > frame_cap {
            return Err(NoFrame::PageEdge);
        }
        let i = region[next];
        next += 1;
        let (x, y) = (i % width, i / width);
        at_page_edge |= x == 0 || y == 0 || x + 1 == width || y + 1 == height;
        let neighbours = [
            (x > 0).then(|| (x - 1, y)),
            (x + 1 < width).then(|| (x + 1, y)),
            (y > 0).then(|| (x, y - 1)),
            (y + 1 < height).then(|| (x, y + 1)),
        ];
        for (nx, ny) in neighbours.into_iter().flatten() {
            let j = (ny * width + nx) as usize;
            if !seen[j] && flat(nx, ny) {
                seen[j] = true;
                region.push(j as u32);
            }
        }
    }

    // Crop to the patch and the box with a margin, cut thin leaks off with
    // an opening and keep the piece under the box.
    let (mut rx0, mut ry0, mut rx1, mut ry1) = (bx0, by0, bx1, by1);
    for &i in &region {
        let (x, y) = (i % width, i / width);
        rx0 = rx0.min(x);
        ry0 = ry0.min(y);
        rx1 = rx1.max(x + 1);
        ry1 = ry1.max(y + 1);
    }
    const MARGIN: u32 = 6;
    let ox = rx0.saturating_sub(MARGIN);
    let oy = ry0.saturating_sub(MARGIN);
    let cw = (rx1 + MARGIN).min(width) - ox;
    let ch = (ry1 + MARGIN).min(height) - oy;
    let mut crop = GrayImage::new(cw, ch);
    for &i in &region {
        crop.put_pixel(i % width - ox, i / width - oy, Luma([255]));
    }
    let radius = leak_radius(width, height, bx1 - bx0, by1 - by0);
    let opened = open(&crop, Norm::LInf, radius);
    let labels = connected_components(&opened, Connectivity::Four, Luma([0u8]));
    let mut counts = std::collections::HashMap::<u32, usize>::new();
    for y in by0..by1 {
        for x in bx0..bx1 {
            let label = labels.get_pixel(x - ox, y - oy).0[0];
            if label != 0 {
                *counts.entry(label).or_insert(0) += 1;
            }
        }
    }
    let label = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(label, _)| label)
        .ok_or(NoFrame::NotFlat)?;
    let mut piece = GrayImage::from_fn(cw, ch, |x, y| {
        Luma([if labels.get_pixel(x, y).0[0] == label {
            255
        } else {
            0
        }])
    });
    let holes = fill_holes(&mut piece);

    // Where it is and how big, against the box.
    let (mut px0, mut py0, mut px1, mut py1) = (cw, ch, 0, 0);
    let mut area = 0usize;
    for (x, y, p) in piece.enumerate_pixels() {
        if p.0[0] != 0 {
            area += 1;
            px0 = px0.min(x);
            py0 = py0.min(y);
            px1 = px1.max(x + 1);
            py1 = py1.max(y + 1);
        }
    }
    if area == 0 {
        return Err(NoFrame::NotFlat);
    }
    let inside = (by0..by1)
        .flat_map(|y| (bx0..bx1).map(move |x| (x, y)))
        .filter(|&(x, y)| piece.get_pixel(x - ox, y - oy).0[0] != 0)
        .count();
    let bounds = (ox + px0, oy + py0, px1 - px0, py1 - py0);
    if bounds.0 == 0
        || bounds.1 == 0
        || bounds.0 + bounds.2 == width
        || bounds.1 + bounds.3 == height
    {
        return Err(NoFrame::PageEdge);
    }
    let to_box = area as f32 / box_area as f32;
    if to_box > MAX_FRAME_TO_BOX {
        return Err(NoFrame::TooBig);
    }
    if (inside as f32) < MIN_BOX_INSIDE * box_area as f32 {
        return Err(NoFrame::BoxOutside);
    }
    if holes as f32 > MAX_HOLE_SHARE * area as f32 {
        return Err(NoFrame::Holes);
    }

    // Pixels outside the patch that differ clearly from it (or belong to a
    // bubble), spread by the distance an outline may sit from the patch.
    let clear = GrayImage::from_fn(cw, ch, |x, y| {
        let (gx, gy) = (ox + x, oy + y);
        let differs = piece.get_pixel(x, y).0[0] == 0
            && (bubbles.get_pixel(gx, gy).0[0] != 0
                || difference(rgb(page, gx, gy), colour) >= EDGE_CONTRAST);
        Luma([if differs { 255 } else { 0 }])
    });
    let clear_near = dilate(&clear, Norm::LInf, EDGE_REACH);
    let outline = outline_pixels(&piece);
    let edge_share = share(&outline, |x, y| clear_near.get_pixel(x, y).0[0] != 0);
    let straight = straight_outline(&piece, side_run(width, height));
    let straightness = share(&outline, |x, y| straight[(y * cw + x) as usize]);
    tracing::debug!(
        ?seed,
        to_box,
        straightness,
        edge = edge_share,
        "flat patch under the box"
    );
    if straightness < MIN_STRAIGHT {
        return Err(NoFrame::Shape);
    }
    if edge_share < MIN_EDGE_SHARE {
        return Err(NoFrame::NoEdge);
    }

    let pixels = (0..bounds.3)
        .flat_map(|y| (0..bounds.2).map(move |x| (x, y)))
        .map(|(x, y)| piece.get_pixel(px0 + x, py0 + y).0[0] != 0)
        .collect();
    Ok(FlatFrame {
        colour,
        bounds,
        pixels,
    })
}

/// Opening radius that cuts a leak along a panel border or another thin line
/// off a frame without eating into a small text box.
fn leak_radius(page_width: u32, page_height: u32, box_width: u32, box_height: u32) -> u8 {
    let by_page = page_width.max(page_height) / 400;
    let by_box = box_width.min(box_height) / 4;
    by_page.clamp(2, 10).min(by_box.max(1)) as u8
}

/// Shortest straight stretch of outline that counts as a side: 1 % of the
/// page's long side (20 px on a 2000 px page).
fn side_run(page_width: u32, page_height: u32) -> u32 {
    (page_width.max(page_height) / 100).clamp(8, 40)
}

/// The pixels of `mask` with a 4-neighbour outside it (or on its border).
fn outline_pixels(mask: &GrayImage) -> Vec<(u32, u32)> {
    let (w, h) = mask.dimensions();
    let on = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && x < w as i64
            && y < h as i64
            && mask.get_pixel(x as u32, y as u32).0[0] != 0
    };
    mask.enumerate_pixels()
        .filter(|(_, _, p)| p.0[0] != 0)
        .filter(|&(x, y, _)| {
            let (x, y) = (x as i64, y as i64);
            !(on(x - 1, y) && on(x + 1, y) && on(x, y - 1) && on(x, y + 1))
        })
        .map(|(x, y, _)| (x, y))
        .collect()
}

/// Share of `pixels` for which `test` holds (1 for none).
fn share(pixels: &[(u32, u32)], test: impl Fn(u32, u32) -> bool) -> f32 {
    if pixels.is_empty() {
        return 1.0;
    }
    pixels.iter().filter(|&&(x, y)| test(x, y)).count() as f32 / pixels.len() as f32
}

/// Marks (row by row over `mask`) the outline pixels that lie on a straight
/// level or upright stretch of the outline at least `run` pixels long: the
/// sides of a frame. Curves and slants break into short steps.
fn straight_outline(mask: &GrayImage, run: u32) -> Vec<bool> {
    let (w, h) = mask.dimensions();
    let on = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && x < w as i64
            && y < h as i64
            && mask.get_pixel(x as u32, y as u32).0[0] != 0
    };
    let mut marks = vec![false; (w * h) as usize];
    // A side faces one way: (dx, dy) is the step to the outside, and the
    // stretch runs across it.
    for (dx, dy) in [(0i64, -1i64), (0, 1), (-1, 0), (1, 0)] {
        let (lines, length) = if dy != 0 { (h, w) } else { (w, h) };
        for line in 0..lines {
            let mut start = None;
            for step in 0..=length {
                let (x, y) = if dy != 0 {
                    (step as i64, line as i64)
                } else {
                    (line as i64, step as i64)
                };
                let side = step < length && on(x, y) && !on(x + dx, y + dy);
                match (side, start) {
                    (true, None) => start = Some(step),
                    (false, Some(first)) => {
                        if step - first >= run {
                            for s in first..step {
                                let (mx, my) = if dy != 0 { (s, line) } else { (line, s) };
                                marks[(my * w + mx) as usize] = true;
                            }
                        }
                        start = None;
                    }
                    _ => {}
                }
            }
        }
    }
    marks
}

/// Fills the holes in `mask` (background not connected to its border).
/// Returns how many pixels were filled.
fn fill_holes(mask: &mut GrayImage) -> usize {
    let (w, h) = mask.dimensions();
    let inverted = GrayImage::from_fn(w, h, |x, y| Luma([255 - mask.get_pixel(x, y).0[0]]));
    let labels = connected_components(&inverted, Connectivity::Four, Luma([0u8]));
    let mut outside = std::collections::HashSet::new();
    for x in 0..w {
        outside.insert(labels.get_pixel(x, 0).0[0]);
        outside.insert(labels.get_pixel(x, h - 1).0[0]);
    }
    for y in 0..h {
        outside.insert(labels.get_pixel(0, y).0[0]);
        outside.insert(labels.get_pixel(w - 1, y).0[0]);
    }
    let mut filled = 0;
    for (x, y, label) in labels.enumerate_pixels() {
        let label = label.0[0];
        if label != 0 && !outside.contains(&label) {
            mask.put_pixel(x, y, Luma([255]));
            filled += 1;
        }
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Fine diagonal stripes in two mid colours: busy artwork.
    fn artwork(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            if (x + y) / 3 % 2 == 0 {
                Rgba([200, 120, 90, 255])
            } else {
                Rgba([60, 90, 150, 255])
            }
        })
    }

    fn paint(page: &mut RgbaImage, (x0, y0, x1, y1): (u32, u32, u32, u32), value: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                page.put_pixel(x, y, Rgba([value, value, value, 255]));
            }
        }
    }

    /// Artwork with a black caption frame at 100..300 x 80..220 in a 2 px
    /// white rim, its lettering already cleaned.
    fn caption_page() -> RgbaImage {
        let mut page = artwork(400, 300);
        paint(&mut page, (98, 78, 302, 222), 255);
        paint(&mut page, (100, 80, 300, 220), 8);
        page
    }

    fn text_box(x: f32, y: f32, width: f32, height: f32) -> LayoutBox {
        LayoutBox {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_black_caption_frame_becomes_a_bubble() {
        let page = caption_page();
        let mut bubbles = GrayImage::new(400, 300);
        let seed = text_box(140.0, 120.0, 120.0, 60.0);
        let frame = find_flat_frame(&page, &bubbles, seed).expect("a caption frame");
        assert_eq!(frame.bounds, (100, 80, 200, 140));

        assert_eq!(add_flat_frames(&mut bubbles, &page, &[seed]), vec![Some(1)]);
        for (x, y) in [(100, 80), (299, 219), (200, 150)] {
            assert_eq!(bubbles.get_pixel(x, y).0[0], 1, "({x}, {y})");
        }
        for (x, y) in [(99, 150), (300, 150), (50, 50)] {
            assert_eq!(bubbles.get_pixel(x, y).0[0], 0, "({x}, {y})");
        }
    }

    #[test]
    fn text_over_busy_artwork_gets_no_frame() {
        let page = artwork(400, 300);
        let mut bubbles = GrayImage::new(400, 300);
        let seed = text_box(140.0, 120.0, 120.0, 60.0);
        assert_eq!(
            find_flat_frame(&page, &bubbles, seed).unwrap_err(),
            NoFrame::NotFlat
        );
        assert_eq!(add_flat_frames(&mut bubbles, &page, &[seed]), vec![None]);
        assert!(bubbles.pixels().all(|p| p.0[0] == 0));
    }

    #[test]
    fn a_plain_background_is_no_frame() {
        // Open to the page edge.
        let page = RgbaImage::from_pixel(400, 300, Rgba([250, 250, 250, 255]));
        let seed = text_box(140.0, 120.0, 120.0, 60.0);
        assert_eq!(
            find_flat_frame(&page, &GrayImage::new(400, 300), seed).unwrap_err(),
            NoFrame::PageEdge
        );
        // Far bigger than a frame round the box would be.
        let page = RgbaImage::from_pixel(1000, 1000, Rgba([250, 250, 250, 255]));
        let seed = text_box(480.0, 490.0, 40.0, 20.0);
        assert_eq!(
            find_flat_frame(&page, &GrayImage::new(1000, 1000), seed).unwrap_err(),
            NoFrame::Unbounded
        );
    }

    #[test]
    fn a_flat_panel_much_bigger_than_the_box_is_no_frame() {
        let mut page = artwork(400, 300);
        paint(&mut page, (18, 18, 382, 282), 255);
        paint(&mut page, (20, 20, 380, 280), 8);
        let seed = text_box(150.0, 125.0, 100.0, 50.0);
        assert_eq!(
            find_flat_frame(&page, &GrayImage::new(400, 300), seed).unwrap_err(),
            NoFrame::TooBig
        );
    }

    #[test]
    fn a_patch_that_fades_out_is_no_frame() {
        // Black in the middle, brightening slowly outwards: square, closed,
        // but with no edge where the colour stops counting as the same.
        let page = RgbaImage::from_fn(400, 300, |x, y| {
            let from_centre = (x as i32 - 200).abs().max((y as i32 - 150).abs());
            let v = ((from_centre - 70).max(0) * 3).min(255) as u8;
            Rgba([v, v, v, 255])
        });
        let seed = text_box(160.0, 130.0, 80.0, 40.0);
        assert_eq!(
            find_flat_frame(&page, &GrayImage::new(400, 300), seed).unwrap_err(),
            NoFrame::NoEdge
        );
    }

    #[test]
    fn a_round_patch_is_no_frame() {
        let mut page = artwork(400, 300);
        for (x, y, p) in page.enumerate_pixels_mut() {
            let (dx, dy) = (x as f32 - 200.0, y as f32 - 150.0);
            let r = (dx * dx / (110.0 * 110.0) + dy * dy / (80.0 * 80.0)).sqrt();
            if r <= 1.0 {
                *p = Rgba([8, 8, 8, 255]);
            } else if r <= 1.03 {
                *p = Rgba([255, 255, 255, 255]);
            }
        }
        let seed = text_box(150.0, 120.0, 100.0, 60.0);
        assert_eq!(
            find_flat_frame(&page, &GrayImage::new(400, 300), seed).unwrap_err(),
            NoFrame::Shape
        );
    }

    #[test]
    fn a_box_on_a_detected_bubble_is_left_alone() {
        let page = caption_page();
        let mut bubbles = GrayImage::new(400, 300);
        for y in 130..140 {
            for x in 150..170 {
                bubbles.put_pixel(x, y, Luma([3]));
            }
        }
        let before = bubbles.clone();
        let ids = add_flat_frames(&mut bubbles, &page, &[text_box(140.0, 120.0, 120.0, 60.0)]);
        assert_eq!(ids, vec![None]);
        assert_eq!(bubbles, before);
    }

    #[test]
    fn boxes_in_one_frame_share_it_and_new_ids_follow_the_detected_ones() {
        let page = caption_page();
        let mut bubbles = GrayImage::new(400, 300);
        for y in 10..40 {
            for x in 10..60 {
                bubbles.put_pixel(x, y, Luma([4]));
            }
        }
        let ids = add_flat_frames(
            &mut bubbles,
            &page,
            &[
                text_box(110.0, 90.0, 100.0, 50.0),
                text_box(190.0, 160.0, 100.0, 50.0),
            ],
        );
        assert_eq!(ids, vec![Some(5), Some(5)]);
    }

    #[test]
    fn frames_overlapping_like_steps_count_as_one() {
        // Two caption frames, the second overlapping the first's corner
        // (Ruination 0062 and 0063 have such pairs), one text in each.
        let mut page = artwork(500, 400);
        paint(&mut page, (38, 38, 262, 202), 255);
        paint(&mut page, (198, 178, 462, 362), 255);
        paint(&mut page, (40, 40, 260, 200), 8);
        paint(&mut page, (200, 180, 460, 360), 8);
        let mut bubbles = GrayImage::new(500, 400);
        let ids = add_flat_frames(
            &mut bubbles,
            &page,
            &[
                text_box(70.0, 70.0, 150.0, 90.0),
                text_box(240.0, 220.0, 180.0, 100.0),
            ],
        );
        assert_eq!(ids, vec![Some(1), Some(1)]);
    }

    #[test]
    fn a_panel_border_touching_the_frame_is_cut_off() {
        let mut page = caption_page();
        // A thin black line from the frame's right side to the page edge.
        for y in 148..151 {
            for x in 300..400 {
                page.put_pixel(x, y, Rgba([8, 8, 8, 255]));
            }
        }
        let mut bubbles = GrayImage::new(400, 300);
        let seed = text_box(140.0, 120.0, 120.0, 60.0);
        assert_eq!(add_flat_frames(&mut bubbles, &page, &[seed]), vec![Some(1)]);
        assert_eq!(bubbles.get_pixel(350, 149).0[0], 0);
        assert_eq!(bubbles.get_pixel(299, 149).0[0], 1);
    }
}
