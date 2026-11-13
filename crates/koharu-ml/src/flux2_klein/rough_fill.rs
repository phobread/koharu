//! Where Flux2 should start a fill from the picture around the lettering
//! instead of from noise.
//!
//! Started from noise, Flux2 sometimes takes a dark caption panel for part of
//! the lettering and paints the art behind the panel instead: on Ruination
//! 0062 (2026-11-13) the 4/5/6/7 panel came out pale in 8 of 8 seeded runs,
//! and inside the see-through black bubble on Ruination 0063 it painted the
//! shoulder behind the bubble at full brightness in 4 of 6 runs. Lettering
//! whose surroundings are one plain colour (`plain_surroundings`) therefore
//! starts from `rough_fill`, the surroundings carried on smoothly through the
//! erased pixels. Outside bubbles that takes a strict test, because over
//! artwork a smooth fill across two colours stays in the result as a smear
//! (LOLExtras 0026). Inside a bubble, only dark bubbles take part, and the
//! bubble's outline and stray marks are left out: only the pixels close to
//! the bubble's own colour are filled from, so a small black bubble isn't
//! filled grey from its white outline. In light bubbles the start did more
//! harm than good (2026-11-13): white over the art seen through a bubble
//! (LOLExtras 0035), hearts next to the lettering that Flux2 otherwise
//! redraws gone, and gaps where the lettering touches the outline (Neeko's
//! Help 0012).

use image::{GrayImage, Luma, Rgb, RgbImage};
use imageproc::{
    distance_transform::Norm,
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};

/// Pixels this close to the erased area aren't trusted as surroundings: the
/// soft edge of a letter reaches past the mask.
const EDGE_GUARD_PX: u8 = 2;
/// Only a band this wide outside the guard counts as the surroundings. Left
/// to use every known pixel, the coarse levels average whole blocks of the
/// picture, and a big hole in a dark caption panel was filled grey from the
/// pale art outside the panel (Ruination 0062).
const SURROUNDINGS_PX: u8 = 8;
/// Erased pixels this close together are one piece of lettering.
const JOIN_PX: u8 = 6;
/// A piece's surroundings are plain when this share of them is within
/// [`PLAIN_TOLERANCE`] of their median colour. Measured on the test pages:
/// dark panels and backgrounds 0.99-1.00 (Ruination 0062, 0063, 0064), an SFX
/// over hair and a white background 0.26 (LOLExtras 0026), an SFX over a
/// gradient 0.82 (Ruination 0045).
const PLAIN_SHARE: f32 = 0.9;
/// Largest per-channel difference from the median that still counts as the
/// same colour.
const PLAIN_TOLERANCE: u8 = 24;
/// A piece needs this many surrounding pixels to be judged at all.
const MIN_SURROUNDING_PIXELS: usize = 64;
/// The start is swapped over this far past the region Flux2 regenerates
/// around a plain piece. One latent token is 16 px.
const START_MARGIN_PX: u8 = 16;
/// Inside a bubble, surrounding pixels this close (per channel) to their
/// median colour are the bubble's own colour; the rest (outline, the page
/// past a coarse bubble map, stray marks) is ignored. Wide enough to keep
/// art seen through a see-through bubble (33 from the bubble's black on
/// Ruination 0063).
const BUBBLE_TOLERANCE: u8 = 48;
/// A bubble's colour is judged only on pixels at least this far inside the
/// bubble map. The map comes in cells of about 12.5 px on a 2000 px page
/// (the segmentation's mask resolution), so nearer its edge a pixel may be
/// the outline or the page past the bubble: around the small black SFX
/// bubbles on Ruination 0063 the median of all surrounding pixels in the map
/// was the sky.
const BUBBLE_MAP_CELL_PX: u8 = 12;
/// A bubble is filled from its own colour when this share of the pixels it
/// is judged on has it. Measured: the see-through bubble on Ruination 0063,
/// black bubbles and small black SFX bubbles on 0062 1.00, small black SFX
/// bubbles on 0063 0.81 (judged on all surrounding pixels in the map: 0.99,
/// 1.00, 0.79 and 0.41).
const BUBBLE_SHARE: f32 = 0.75;
/// Only bubbles whose colour has no channel above this take part. On the
/// test pages dark bubbles reach 26 at most and light ones start at 237.
const DARK_BUBBLE_MAX: u8 = 64;

/// Where a fill should start from the picture, and from what.
pub(super) struct Surroundings {
    /// Where (255) the fill starts from the picture.
    pub weights: GrayImage,
    /// The bubbles that take part, each with its own colour.
    pub bubble_colours: Vec<(u8, [u8; 3])>,
}

/// Where (255) a fill should start from the picture, or `None` when no
/// lettering qualifies. `erase` (nonzero) is what will be pasted,
/// `generation` what Flux2 regenerates, `bubbles` the speech-bubble ID map
/// (0 = no bubble). Taken are the regions Flux2 regenerates around:
/// - lettering outside bubbles whose surroundings are plain;
/// - lettering in a dark bubble when most of its surroundings well inside
///   that bubble share one colour, the bubble's (see [`BUBBLE_MAP_CELL_PX`]),
///   and only inside that bubble.
///
/// The whole regenerated region is taken, not only the lettering: Flux2
/// regenerates a text box's whole rectangle, and where part of it still
/// started from noise, the pale art it painted there spread into the letters
/// next to it (Ruination 0062 in the app, 2026-11-13).
///
/// Lettering outside bubbles in a region that also holds lettering inside a
/// bubble is left alone: that is bubble lettering the bubble map only partly
/// covers, as in a black bubble on Ruination 0063 whose hearts fell outside
/// the map and whose fill then came out as the page behind the bubble.
pub(super) fn plain_surroundings(
    image: &RgbImage,
    erase: &GrayImage,
    generation: &GrayImage,
    bubbles: Option<&GrayImage>,
) -> Option<Surroundings> {
    let (width, height) = image.dimensions();
    if erase.dimensions() != (width, height) || generation.dimensions() != (width, height) {
        return None;
    }
    let bubbles = bubbles.filter(|ids| ids.dimensions() == (width, height));
    let outside = |x: u32, y: u32| bubbles.is_none_or(|ids| ids.get_pixel(x, y).0[0] == 0);
    let lettering = GrayImage::from_fn(width, height, |x, y| {
        Luma([u8::from(erase.get_pixel(x, y).0[0] > 0 && outside(x, y)) * 255])
    });
    if !erase.pixels().any(|p| p.0[0] > 0) {
        return None;
    }
    // The regenerated regions, and which of them hold lettering in a bubble.
    let regions = connected_components(
        &GrayImage::from_fn(width, height, |x, y| {
            Luma([u8::from(generation.get_pixel(x, y).0[0] > 0) * 255])
        }),
        Connectivity::Eight,
        Luma([0u8]),
    );
    let region_count = regions.pixels().map(|p| p.0[0]).max().unwrap_or(0) as usize + 1;
    let mut bubble_lettering = vec![false; region_count];
    for (x, y, p) in erase.enumerate_pixels() {
        if p.0[0] > 0 && !outside(x, y) {
            bubble_lettering[regions.get_pixel(x, y).0[0] as usize] = true;
        }
    }
    let guarded = binary_dilate(erase, EDGE_GUARD_PX);
    let pieces = connected_components(
        &dilate(&lettering, Norm::LInf, JOIN_PX),
        Connectivity::Eight,
        Luma([0u8]),
    );

    // Bounds of each piece, then its surroundings judged inside them.
    let count = pieces.pixels().map(|p| p.0[0]).max().unwrap_or(0) as usize;
    let mut bounds = vec![(u32::MAX, u32::MAX, 0u32, 0u32); count];
    for (x, y, label) in pieces.enumerate_pixels() {
        if label.0[0] > 0 {
            let b = &mut bounds[label.0[0] as usize - 1];
            *b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        }
    }
    let reach = u32::from(EDGE_GUARD_PX + SURROUNDINGS_PX);
    let mut plain = GrayImage::new(width, height);
    let mut any = false;
    for (index, &(x0, y0, x1, y1)) in bounds.iter().enumerate() {
        let label = index as u32 + 1;
        let (bx0, by0) = (x0.saturating_sub(reach), y0.saturating_sub(reach));
        let (bx1, by1) = ((x1 + reach + 1).min(width), (y1 + reach + 1).min(height));
        let (bw, bh) = (bx1 - bx0, by1 - by0);
        let piece = GrayImage::from_fn(bw, bh, |x, y| {
            let (px, py) = (bx0 + x, by0 + y);
            Luma([u8::from(
                pieces.get_pixel(px, py).0[0] == label && lettering.get_pixel(px, py).0[0] > 0,
            ) * 255])
        });
        let in_bubble_region = piece.enumerate_pixels().any(|(x, y, p)| {
            p.0[0] > 0 && bubble_lettering[regions.get_pixel(bx0 + x, by0 + y).0[0] as usize]
        });
        if in_bubble_region {
            continue;
        }
        let around = dilate(&piece, Norm::LInf, EDGE_GUARD_PX + SURROUNDINGS_PX);
        let mut samples = Vec::new();
        for (x, y, near) in around.enumerate_pixels() {
            let (px, py) = (bx0 + x, by0 + y);
            if near.0[0] > 0 && guarded.get_pixel(px, py).0[0] == 0 && outside(px, py) {
                samples.push(image.get_pixel(px, py).0);
            }
        }
        if !is_plain(&mut samples) {
            continue;
        }
        any = true;
        for (x, y, p) in pieces.enumerate_pixels() {
            if p.0[0] == label {
                plain.put_pixel(x, y, Luma([255]));
            }
        }
    }
    let mut weights = GrayImage::new(width, height);
    if any {
        // Add the regenerated regions the plain pieces lie in.
        let mut taken = vec![false; region_count];
        for (x, y, p) in plain.enumerate_pixels() {
            if p.0[0] > 0 {
                taken[regions.get_pixel(x, y).0[0] as usize] = true;
            }
        }
        taken[0] = false;
        for (x, y, region) in regions.enumerate_pixels() {
            if taken[region.0[0] as usize] {
                plain.put_pixel(x, y, Luma([255]));
            }
        }
        let plain = dilate(&plain, Norm::LInf, START_MARGIN_PX);
        for (x, y, p) in plain.enumerate_pixels() {
            if p.0[0] > 0 && outside(x, y) {
                weights.put_pixel(x, y, Luma([255]));
            }
        }
    }

    // Each dark bubble whose lettering sits in the bubble's own colour: its
    // lettering and the regions it lies in, inside that bubble only.
    let bubble_colours = bubbles
        .map(|ids| bubble_colours(image, erase, &guarded, ids))
        .unwrap_or_default();
    if let Some(ids) = bubbles {
        for &(id, _) in &bubble_colours {
            let mut taken = vec![false; region_count];
            let mut marked = GrayImage::new(width, height);
            for (x, y, p) in erase.enumerate_pixels() {
                if p.0[0] > 0 && ids.get_pixel(x, y).0[0] == id {
                    taken[regions.get_pixel(x, y).0[0] as usize] = true;
                    marked.put_pixel(x, y, Luma([255]));
                }
            }
            taken[0] = false;
            for (x, y, region) in regions.enumerate_pixels() {
                if taken[region.0[0] as usize] {
                    marked.put_pixel(x, y, Luma([255]));
                }
            }
            let marked = dilate(&marked, Norm::LInf, START_MARGIN_PX);
            for (x, y, p) in marked.enumerate_pixels() {
                if p.0[0] > 0 && ids.get_pixel(x, y).0[0] == id {
                    weights.put_pixel(x, y, Luma([255]));
                }
            }
        }
    }
    if !any && bubble_colours.is_empty() {
        return None;
    }
    Some(Surroundings {
        weights,
        bubble_colours,
    })
}

/// The dark bubbles in `ids` that hold lettering whose surroundings well
/// inside the bubble mostly share one colour, each with that colour.
fn bubble_colours(
    image: &RgbImage,
    erase: &GrayImage,
    guarded: &GrayImage,
    ids: &GrayImage,
) -> Vec<(u8, [u8; 3])> {
    let (width, height) = image.dimensions();
    let in_bubbles = GrayImage::from_fn(width, height, |x, y| {
        Luma([u8::from(erase.get_pixel(x, y).0[0] > 0 && ids.get_pixel(x, y).0[0] > 0) * 255])
    });
    let mut lettered = [false; 256];
    for (x, y, p) in in_bubbles.enumerate_pixels() {
        if p.0[0] > 0 {
            lettered[ids.get_pixel(x, y).0[0] as usize] = true;
        }
    }
    if !lettered.contains(&true) {
        return Vec::new();
    }
    let around = dilate(&in_bubbles, Norm::LInf, EDGE_GUARD_PX + SURROUNDINGS_PX);
    let mut colours = Vec::new();
    for id in (1..=255u8).filter(|&id| lettered[id as usize]) {
        let elsewhere = GrayImage::from_fn(width, height, |x, y| {
            Luma([u8::from(ids.get_pixel(x, y).0[0] != id) * 255])
        });
        let near_edge = dilate(&elsewhere, Norm::LInf, BUBBLE_MAP_CELL_PX);
        let mut samples = Vec::new();
        for (x, y, near) in around.enumerate_pixels() {
            if near.0[0] > 0
                && near_edge.get_pixel(x, y).0[0] == 0
                && guarded.get_pixel(x, y).0[0] == 0
            {
                samples.push(image.get_pixel(x, y).0);
            }
        }
        if let Some(colour) = bubble_colour(&mut samples)
            && colour.iter().all(|c| *c <= DARK_BUBBLE_MAX)
        {
            colours.push((id, colour));
        }
    }
    colours
}

/// Whether most of `samples` share one colour.
fn is_plain(samples: &mut [[u8; 3]]) -> bool {
    if samples.len() < MIN_SURROUNDING_PIXELS {
        return false;
    }
    let median = median(samples);
    let close = samples
        .iter()
        .filter(|p| close_to(p, &median, PLAIN_TOLERANCE))
        .count();
    close as f32 >= PLAIN_SHARE * samples.len() as f32
}

/// The median colour of a bubble's `samples`, when enough of them are close
/// to it (see [`BUBBLE_SHARE`]).
fn bubble_colour(samples: &mut [[u8; 3]]) -> Option<[u8; 3]> {
    if samples.len() < MIN_SURROUNDING_PIXELS {
        return None;
    }
    let median = median(samples);
    let close = samples
        .iter()
        .filter(|p| close_to(p, &median, BUBBLE_TOLERANCE))
        .count();
    (close as f32 >= BUBBLE_SHARE * samples.len() as f32).then_some(median)
}

/// The per-channel median of `samples` (not empty).
fn median(samples: &mut [[u8; 3]]) -> [u8; 3] {
    std::array::from_fn(|c| {
        samples.sort_unstable_by_key(|p| p[c]);
        samples[samples.len() / 2][c]
    })
}

fn close_to(pixel: &[u8; 3], colour: &[u8; 3], tolerance: u8) -> bool {
    (0..3).all(|c| pixel[c].abs_diff(colour[c]) <= tolerance)
}

/// `image` with every pixel under `erase` (nonzero) replaced by a smooth
/// blend of its surroundings (pull-push interpolation from a narrow band
/// around the erased area). Lettering outside bubbles is filled from the
/// surroundings outside bubbles; lettering in one of `bubble_colours` from
/// the surroundings in that bubble close to its colour. Other pixels are
/// unchanged, and so is lettering with nothing to fill from.
pub(super) fn rough_fill(
    image: &RgbImage,
    erase: &GrayImage,
    bubbles: Option<&GrayImage>,
    bubble_colours: &[(u8, [u8; 3])],
) -> RgbImage {
    let (width, height) = image.dimensions();
    if erase.dimensions() != (width, height) {
        return image.clone();
    }
    let bubbles = bubbles.filter(|ids| ids.dimensions() == (width, height));
    let id_at = |x: u32, y: u32| bubbles.map_or(0, |ids| ids.get_pixel(x, y).0[0]);
    let guarded = binary_dilate(erase, EDGE_GUARD_PX);
    let around = binary_dilate(erase, EDGE_GUARD_PX + SURROUNDINGS_PX);
    let mut filled = image.clone();
    let groups = std::iter::once((0, None)).chain(
        bubble_colours
            .iter()
            .map(|&(id, colour)| (id, Some(colour))),
    );
    for (id, colour) in groups {
        let erased = |x: u32, y: u32| erase.get_pixel(x, y).0[0] > 0 && id_at(x, y) == id;
        if !(0..height).any(|y| (0..width).any(|x| erased(x, y))) {
            continue;
        }
        let known = |x: u32, y: u32| {
            guarded.get_pixel(x, y).0[0] == 0
                && around.get_pixel(x, y).0[0] > 0
                && id_at(x, y) == id
                && colour.is_none_or(|colour| {
                    close_to(&image.get_pixel(x, y).0, &colour, BUBBLE_TOLERANCE)
                })
        };
        let Some(colours) = pull_push(image, known) else {
            continue;
        };
        for y in 0..height {
            for x in 0..width {
                if erased(x, y) {
                    let colour = colours[(y * width + x) as usize];
                    filled.put_pixel(x, y, Rgb(colour.map(|c| c.round().clamp(0.0, 255.0) as u8)));
                }
            }
        }
    }
    filled
}

/// `mask` (nonzero = set) grown by `radius`, as 0/255.
fn binary_dilate(mask: &GrayImage, radius: u8) -> GrayImage {
    let binary = GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        Luma([u8::from(mask.get_pixel(x, y).0[0] > 0) * 255])
    });
    dilate(&binary, Norm::LInf, radius)
}

/// One level of the pyramid: colours averaged over the known part of each
/// pixel, and how much of it is known (0..=1).
struct Level {
    width: usize,
    height: usize,
    colour: Vec<[f32; 3]>,
    weight: Vec<f32>,
}

/// Every pixel of `image` interpolated from the pixels `known` says to use
/// (holes are filled from ever coarser averages), or `None` when there are
/// none.
fn pull_push(image: &RgbImage, known: impl Fn(u32, u32) -> bool) -> Option<Vec<[f32; 3]>> {
    let (width, height) = image.dimensions();
    let mut base = Level {
        width: width as usize,
        height: height as usize,
        colour: Vec::with_capacity((width * height) as usize),
        weight: Vec::with_capacity((width * height) as usize),
    };
    for (x, y, pixel) in image.enumerate_pixels() {
        let known = known(x, y);
        base.weight.push(if known { 1.0 } else { 0.0 });
        base.colour.push(if known {
            pixel.0.map(f32::from)
        } else {
            [0.0; 3]
        });
    }
    if base.weight.iter().all(|w| *w == 0.0) {
        return None;
    }

    // Pull: each coarser pixel averages the known part of its four children.
    let mut levels = vec![base];
    while levels.last().is_some_and(|l| l.width > 1 || l.height > 1) {
        let fine = levels.last().expect("at least the base level");
        let (w, h) = (fine.width.div_ceil(2), fine.height.div_ceil(2));
        let mut coarse = Level {
            width: w,
            height: h,
            colour: vec![[0.0; 3]; w * h],
            weight: vec![0.0; w * h],
        };
        for y in 0..fine.height {
            for x in 0..fine.width {
                let (i, j) = (y * fine.width + x, (y / 2) * w + x / 2);
                let weight = fine.weight[i];
                coarse.weight[j] += weight;
                for c in 0..3 {
                    coarse.colour[j][c] += fine.colour[i][c] * weight;
                }
            }
        }
        for (colour, weight) in coarse.colour.iter_mut().zip(coarse.weight.iter_mut()) {
            if *weight > 0.0 {
                for c in colour.iter_mut() {
                    *c /= *weight;
                }
            }
            *weight = weight.min(1.0);
        }
        levels.push(coarse);
    }

    // Push: what a level doesn't know comes from the level above it.
    for index in (0..levels.len() - 1).rev() {
        let (fine, coarse) = levels.split_at_mut(index + 1);
        let (fine, coarse) = (&mut fine[index], &coarse[0]);
        for y in 0..fine.height {
            for x in 0..fine.width {
                let i = y * fine.width + x;
                let weight = fine.weight[i];
                if weight >= 1.0 {
                    continue;
                }
                let from_above = sample(coarse, x, y);
                for (own, above) in fine.colour[i].iter_mut().zip(from_above) {
                    *own = *own * weight + above * (1.0 - weight);
                }
                fine.weight[i] = 1.0;
            }
        }
    }
    Some(levels.swap_remove(0).colour)
}

/// `coarse` read at the fine pixel `(x, y)` of the level below, bilinearly.
fn sample(coarse: &Level, x: usize, y: usize) -> [f32; 3] {
    let gx = ((x as f32 + 0.5) / 2.0 - 0.5).max(0.0);
    let gy = ((y as f32 + 0.5) / 2.0 - 0.5).max(0.0);
    let x0 = (gx.floor() as usize).min(coarse.width - 1);
    let y0 = (gy.floor() as usize).min(coarse.height - 1);
    let x1 = (x0 + 1).min(coarse.width - 1);
    let y1 = (y0 + 1).min(coarse.height - 1);
    let (tx, ty) = (gx - gx.floor(), gy - gy.floor());
    std::array::from_fn(|c| {
        let top = coarse.colour[y0 * coarse.width + x0][c] * (1.0 - tx)
            + coarse.colour[y0 * coarse.width + x1][c] * tx;
        let bottom = coarse.colour[y1 * coarse.width + x0][c] * (1.0 - tx)
            + coarse.colour[y1 * coarse.width + x1][c] * tx;
        top * (1.0 - ty) + bottom * ty
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(width: u32, height: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> GrayImage {
        GrayImage::from_fn(width, height, |x, y| {
            Luma([u8::from((x0..x1).contains(&x) && (y0..y1).contains(&y)) * 255])
        })
    }

    /// A dark panel (16..80) on a pale page with white lettering inside it.
    fn dark_panel(erase: &GrayImage) -> RgbImage {
        RgbImage::from_fn(erase.width(), erase.height(), |x, y| {
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([250, 250, 250])
            } else if (16..80).contains(&x) && (16..80).contains(&y) {
                Rgb([20, 24, 22])
            } else {
                Rgb([200, 210, 205])
            }
        })
    }

    #[test]
    fn lettering_on_a_dark_panel_is_filled_dark() {
        let erase = block(96, 96, 36, 36, 60, 60);
        let image = dark_panel(&erase);

        let filled = rough_fill(&image, &erase, None, &[]);

        for (x, y, pixel) in filled.enumerate_pixels() {
            if erase.get_pixel(x, y).0[0] > 0 {
                assert!(
                    pixel.0.iter().all(|c| *c < 60),
                    "pixel {x},{y} is {pixel:?}"
                );
            } else {
                assert_eq!(pixel, image.get_pixel(x, y));
            }
        }
    }

    #[test]
    fn a_big_hole_in_a_dark_panel_ignores_the_page_outside_the_panel() {
        // The hole fills most of the panel; the pale page is 12px away.
        let erase = block(256, 256, 72, 72, 184, 184);
        let image = RgbImage::from_fn(256, 256, |x, y| {
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([250, 250, 250])
            } else if (60..196).contains(&x) && (60..196).contains(&y) {
                Rgb([20, 24, 22])
            } else {
                Rgb([200, 210, 205])
            }
        });

        let filled = rough_fill(&image, &erase, None, &[]);

        for (x, y, pixel) in filled.enumerate_pixels() {
            if erase.get_pixel(x, y).0[0] > 0 {
                assert!(
                    pixel.0.iter().all(|c| *c < 40),
                    "pixel {x},{y} is {pixel:?}"
                );
            }
        }
    }

    #[test]
    fn the_soft_edge_of_a_letter_does_not_colour_the_fill() {
        // A one-pixel grey fringe around the mask, as anti-aliasing leaves.
        let erase = block(64, 64, 24, 24, 40, 40);
        let image = RgbImage::from_fn(64, 64, |x, y| {
            if (23..41).contains(&x) && (23..41).contains(&y) {
                Rgb([255, 255, 255])
            } else {
                Rgb([10, 10, 10])
            }
        });

        let filled = rough_fill(&image, &erase, None, &[]);

        assert_eq!(filled.get_pixel(32, 32).0, [10, 10, 10]);
        assert_eq!(filled.get_pixel(24, 24).0, [10, 10, 10]);
    }

    #[test]
    fn a_bubble_next_to_the_lettering_does_not_colour_the_fill() {
        let erase = block(96, 96, 36, 36, 60, 60);
        let mut image = dark_panel(&erase);
        // A white bubble right beside the lettering.
        let ids = GrayImage::from_fn(96, 96, |x, y| {
            Luma([u8::from((62..80).contains(&x) && (36..60).contains(&y))])
        });
        for (x, y, p) in ids.enumerate_pixels() {
            if p.0[0] > 0 {
                image.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }

        let filled = rough_fill(&image, &erase, Some(&ids), &[]);

        assert!(filled.get_pixel(58, 48).0.iter().all(|c| *c < 40));
    }

    #[test]
    fn a_gradient_carries_on_through_the_fill() {
        let erase = block(128, 64, 48, 16, 80, 48);
        let image = RgbImage::from_fn(128, 64, |x, _| {
            let v = (x * 2) as u8;
            Rgb([v, v, v])
        });

        let filled = rough_fill(&image, &erase, None, &[]);

        let left = filled.get_pixel(50, 32).0[0];
        let middle = filled.get_pixel(64, 32).0[0];
        let right = filled.get_pixel(78, 32).0[0];
        assert!(left < middle && middle < right, "{left} {middle} {right}");
        assert!((i16::from(middle) - 128).abs() < 16, "middle is {middle}");
    }

    #[test]
    fn nothing_known_leaves_the_image_alone() {
        let erase = GrayImage::from_pixel(8, 8, Luma([255]));
        let image = RgbImage::from_pixel(8, 8, Rgb([7, 8, 9]));
        assert_eq!(rough_fill(&image, &erase, None, &[]), image);
    }

    #[test]
    fn lettering_on_a_plain_panel_is_marked_with_a_margin() {
        let erase = block(96, 96, 36, 36, 60, 60);
        let image = dark_panel(&erase);

        let plain = plain_surroundings(&image, &erase, &erase, None)
            .expect("the panel is plain")
            .weights;

        assert_eq!(plain.get_pixel(48, 48).0[0], 255);
        // The margin reaches past the lettering...
        assert_eq!(plain.get_pixel(36 - 6 - 16, 48).0[0], 255);
        // ...and no further.
        assert_eq!(plain.get_pixel(36 - 6 - 17, 48).0[0], 0);
    }

    #[test]
    fn the_whole_regenerated_box_around_plain_lettering_is_marked() {
        let erase = block(160, 160, 60, 60, 80, 80);
        let image = RgbImage::from_fn(160, 160, |x, y| {
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([250, 250, 250])
            } else {
                Rgb([20, 24, 22])
            }
        });
        // Flux2 regenerates a box far bigger than the lettering, and a
        // separate box elsewhere that holds none of it.
        let mut generation = block(160, 160, 10, 10, 130, 130);
        for (x, y, p) in block(160, 160, 150, 150, 160, 160).enumerate_pixels() {
            if p.0[0] > 0 {
                generation.put_pixel(x, y, Luma([255]));
            }
        }

        let plain = plain_surroundings(&image, &erase, &generation, None)
            .expect("the panel is plain")
            .weights;

        assert_eq!(plain.get_pixel(12, 12).0[0], 255);
        assert_eq!(plain.get_pixel(128, 128).0[0], 255);
        assert_eq!(plain.get_pixel(145, 70).0[0], 255, "16px past the box");
        assert_eq!(plain.get_pixel(146, 70).0[0], 0);
        assert_eq!(
            plain.get_pixel(155, 155).0[0],
            0,
            "the other box isn't taken"
        );
    }

    #[test]
    fn lettering_sharing_a_box_with_bubble_lettering_is_taken_only_inside_the_bubble() {
        // A black bubble the map only partly covers: its left half is
        // bubble 1, its right half (with a heart in it) isn't in the map.
        let erase = block(160, 96, 40, 30, 120, 66);
        let image = RgbImage::from_fn(160, 96, |x, y| {
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([250, 250, 250])
            } else {
                Rgb([8, 8, 8])
            }
        });
        let ids = bubble_map(160, 96, 20, 10, 80, 86);
        // One box around all of it.
        let generation = block(160, 96, 30, 20, 130, 76);

        let plain = plain_surroundings(&image, &erase, &generation, Some(&ids))
            .expect("the half in the map is a plain bubble");
        assert_eq!(plain.weights.get_pixel(60, 48).0[0], 255);
        assert_eq!(plain.weights.get_pixel(100, 48).0[0], 0);
        // In a box of its own, the same right half would be taken.
        let own = block(160, 96, 82, 20, 130, 76);
        let mut split = block(160, 96, 30, 20, 78, 76);
        for (x, y, p) in own.enumerate_pixels() {
            if p.0[0] > 0 {
                split.put_pixel(x, y, Luma([255]));
            }
        }
        let erase_right = GrayImage::from_fn(160, 96, |x, y| {
            Luma([if x >= 82 {
                erase.get_pixel(x, y).0[0]
            } else {
                0
            }])
        });
        let mut erase_split = erase_right.clone();
        for (x, y, p) in block(160, 96, 40, 30, 76, 66).enumerate_pixels() {
            if p.0[0] > 0 {
                erase_split.put_pixel(x, y, Luma([255]));
            }
        }
        let plain = plain_surroundings(&image, &erase_split, &split, Some(&ids))
            .expect("the right half has a box of its own");
        assert_eq!(plain.weights.get_pixel(100, 48).0[0], 255);
    }

    #[test]
    fn lettering_across_two_colours_is_not_plain() {
        // Hair on the left, white page on the right, an SFX across both.
        let erase = block(128, 96, 40, 30, 88, 66);
        let image = RgbImage::from_fn(128, 96, |x, _| {
            if x < 64 {
                Rgb([70, 50, 150])
            } else {
                Rgb([235, 230, 235])
            }
        });

        assert!(plain_surroundings(&image, &erase, &erase, None).is_none());
    }

    /// Bubble 1 over `x0..x1` x `y0..y1`, nothing elsewhere.
    fn bubble_map(width: u32, height: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> GrayImage {
        GrayImage::from_fn(width, height, |x, y| {
            Luma([u8::from((x0..x1).contains(&x) && (y0..y1).contains(&y))])
        })
    }

    #[test]
    fn lettering_in_a_plain_bubble_is_marked_inside_the_bubble_only() {
        // A black bubble with a white outline on a pale page.
        let erase = block(128, 128, 44, 44, 84, 84);
        let ids = bubble_map(128, 128, 16, 16, 112, 112);
        let image = RgbImage::from_fn(128, 128, |x, y| {
            let inside = |a, b| (a..b).contains(&x) && (a..b).contains(&y);
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([250, 250, 250])
            } else if inside(20, 108) {
                Rgb([10, 10, 10])
            } else if inside(16, 112) {
                Rgb([240, 240, 240])
            } else {
                Rgb([200, 210, 205])
            }
        });

        let plain =
            plain_surroundings(&image, &erase, &erase, Some(&ids)).expect("the bubble is plain");

        assert_eq!(plain.bubble_colours, vec![(1, [10, 10, 10])]);
        assert_eq!(plain.weights.get_pixel(64, 64).0[0], 255);
        assert_eq!(plain.weights.get_pixel(44 - 16, 64).0[0], 255, "the margin");
        assert_eq!(plain.weights.get_pixel(44 - 17, 64).0[0], 0);
        let filled = rough_fill(&image, &erase, Some(&ids), &plain.bubble_colours);
        assert_eq!(filled.get_pixel(64, 64).0, [10, 10, 10]);
    }

    #[test]
    fn the_outline_of_a_small_bubble_does_not_colour_its_fill() {
        // The lettering nearly fills a small black bubble, so its white
        // outline lies in the surroundings; the map overshoots the bubble.
        let erase = block(100, 100, 30, 30, 70, 70);
        let ids = bubble_map(100, 100, 10, 10, 90, 90);
        let image = RgbImage::from_fn(100, 100, |x, y| {
            let inside = |a, b| (a..b).contains(&x) && (a..b).contains(&y);
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([250, 250, 250])
            } else if inside(23, 77) {
                Rgb([8, 8, 8])
            } else if inside(22, 78) {
                Rgb([240, 240, 240])
            } else {
                Rgb([200, 210, 205])
            }
        });

        let plain =
            plain_surroundings(&image, &erase, &erase, Some(&ids)).expect("mostly the bubble");
        let filled = rough_fill(&image, &erase, Some(&ids), &plain.bubble_colours);

        assert_eq!(plain.bubble_colours, vec![(1, [8, 8, 8])]);
        for (x, y, pixel) in filled.enumerate_pixels() {
            if erase.get_pixel(x, y).0[0] > 0 {
                assert!(
                    pixel.0.iter().all(|c| *c < 30),
                    "pixel {x},{y} is {pixel:?}"
                );
            }
        }
    }

    #[test]
    fn a_bubble_its_lettering_nearly_fills_is_left_alone() {
        // A white bubble on a black page; past the lettering there is about
        // as much page as bubble, so the bubble's colour is uncertain.
        let erase = block(100, 100, 30, 30, 70, 70);
        let ids = bubble_map(100, 100, 10, 10, 90, 90);
        let image = RgbImage::from_fn(100, 100, |x, y| {
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([0, 0, 0])
            } else if (24..76).contains(&x) && (24..76).contains(&y) {
                Rgb([250, 250, 250])
            } else {
                Rgb([12, 12, 12])
            }
        });

        assert!(plain_surroundings(&image, &erase, &erase, Some(&ids)).is_none());
    }

    #[test]
    fn a_bubble_of_two_colours_is_left_alone() {
        let erase = block(128, 128, 40, 40, 88, 88);
        let ids = GrayImage::from_pixel(128, 128, Luma([1]));
        let image = RgbImage::from_fn(128, 128, |x, _| {
            if x < 64 {
                Rgb([8, 8, 64])
            } else {
                Rgb([64, 8, 8])
            }
        });

        assert!(plain_surroundings(&image, &erase, &erase, Some(&ids)).is_none());
    }

    #[test]
    fn lettering_in_a_light_bubble_is_left_alone() {
        let erase = block(128, 128, 40, 40, 88, 88);
        let ids = GrayImage::from_pixel(128, 128, Luma([1]));
        let image = RgbImage::from_fn(128, 128, |x, y| {
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([0, 0, 0])
            } else {
                Rgb([250, 250, 250])
            }
        });

        assert!(plain_surroundings(&image, &erase, &erase, Some(&ids)).is_none());
    }

    #[test]
    fn art_seen_through_a_dark_bubble_carries_on_dimmed() {
        // A see-through black bubble with a pale shoulder behind its left
        // side, dimmed by the bubble.
        let erase = block(128, 128, 40, 40, 88, 88);
        let ids = GrayImage::from_pixel(128, 128, Luma([1]));
        let image = RgbImage::from_fn(128, 128, |x, y| {
            if erase.get_pixel(x, y).0[0] > 0 {
                Rgb([250, 250, 250])
            } else if x < 48 {
                Rgb([44, 48, 48])
            } else {
                Rgb([10, 12, 12])
            }
        });

        let plain =
            plain_surroundings(&image, &erase, &erase, Some(&ids)).expect("one colour family");
        let filled = rough_fill(&image, &erase, Some(&ids), &plain.bubble_colours);

        let left = filled.get_pixel(41, 64).0[0];
        let right = filled.get_pixel(86, 64).0[0];
        assert!(left > right + 10, "left {left}, right {right}");
        assert!(right < 20, "right {right}");
    }

    #[test]
    fn only_the_plain_piece_is_marked() {
        // A plain dark panel on the left and an SFX over two colours on the
        // right, far apart.
        let mut erase = block(256, 96, 30, 36, 50, 60);
        for (x, y, p) in block(256, 96, 168, 30, 216, 66).enumerate_pixels() {
            if p.0[0] > 0 {
                erase.put_pixel(x, y, Luma([255]));
            }
        }
        let image = RgbImage::from_fn(256, 96, |x, _| {
            if x < 100 {
                Rgb([20, 24, 22])
            } else if x < 192 {
                Rgb([70, 50, 150])
            } else {
                Rgb([235, 230, 235])
            }
        });

        let plain = plain_surroundings(&image, &erase, &erase, None)
            .expect("the panel is plain")
            .weights;

        assert_eq!(plain.get_pixel(40, 48).0[0], 255);
        assert_eq!(plain.get_pixel(190, 48).0[0], 0);
    }
}
