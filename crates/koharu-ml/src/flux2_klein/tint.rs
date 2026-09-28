//! Rescue for a uniform tint over a flat bubble.
//!
//! On flat bubbles Flux2 sometimes fills the whole region with one wrong
//! tone: a black bubble comes back grey, maroon or mauve, up to ~100 levels
//! off. The regular colour match can't take that back: it drops ring samples
//! more than 96 levels off as drawing edges and caps its offset at 64, so the
//! tone is pasted as a patch the shape of the text.
//!
//! This pass recognises that case per bubble (or per cluster of text
//! outside bubbles) and returns the tint to remove. It only fires beyond the
//! regular correction's range, and only when all of these hold for the
//! group, so drawing edges and real content can't pass as a tint:
//! - the ring around the pasted pixels is one flat colour in the original;
//! - original minus generated agrees tightly around the whole ring, on every
//!   side of the text, and again in a wider ring;
//! - the generated pixels to paste are that colour shifted by the tint.

use image::{GrayImage, Luma, RgbaImage};
use imageproc::{
    distance_transform::Norm,
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};

/// Tints the regular colour match already removes.
const HANDLED_OFFSET: f32 = 64.0;
const NEAR_RING_PX: u8 = 2;
const WIDE_RING_INNER_PX: u8 = 3;
const WIDE_RING_OUTER_PX: u8 = 6;
const MIN_RING_SAMPLES: usize = 100;
const MIN_SIDE_SAMPLES: usize = 16;
const MIN_SIDES: usize = 3;
/// Agreement of ring deltas with their median, per channel.
const DELTA_TOLERANCE: f32 = 12.0;
const MIN_DELTA_AGREEMENT: f32 = 0.8;
/// Flatness of the original ring colour.
const BACKGROUND_TOLERANCE: f32 = 12.0;
const MIN_FLAT_BACKGROUND: f32 = 0.85;
/// Generated pixels to paste must be the tinted background colour.
const FILL_TOLERANCE: f32 = 24.0;
const MIN_TINTED_FILL: f32 = 0.8;
/// Text pieces outside bubbles this close together are judged together.
const LOOSE_GROUP_PX: u8 = 8;

/// Per-pixel tint to add to `generated` (crop-sized, zero where no group
/// was rescued), or `None` when no group qualifies. `original` is the whole
/// page with the crop at `origin`; `paste` and `bubbles` are crop-local.
pub(super) fn uniform_tint(
    original: &RgbaImage,
    generated: &RgbaImage,
    paste: &GrayImage,
    bubbles: &GrayImage,
    origin: (u32, u32),
) -> Option<Vec<[f32; 3]>> {
    let (width, height) = paste.dimensions();
    if generated.dimensions() != (width, height) || bubbles.dimensions() != (width, height) {
        return None;
    }
    let pasted = GrayImage::from_fn(width, height, |x, y| {
        Luma([if paste.get_pixel(x, y)[0] > 0 { 255 } else { 0 }])
    });
    let near = dilate(&pasted, Norm::LInf, NEAR_RING_PX);
    let wide_inner = dilate(&pasted, Norm::LInf, WIDE_RING_INNER_PX);
    let wide_outer = dilate(&pasted, Norm::LInf, WIDE_RING_OUTER_PX);

    // Each pasted component belongs to the bubble most of it lies in.
    let labels = connected_components(&pasted, Connectivity::Eight, Luma([0]));
    let components = labels.pixels().map(|p| p[0]).max().unwrap_or(0) as usize;
    let mut votes = vec![[0u32; 256]; components + 1];
    for (x, y, label) in labels.enumerate_pixels() {
        if label[0] > 0 {
            votes[label[0] as usize][usize::from(bubbles.get_pixel(x, y)[0])] += 1;
        }
    }
    let owner: Vec<u8> = votes
        .iter()
        .map(|counts| {
            let (id, count) = counts
                .iter()
                .enumerate()
                .max_by_key(|(_, count)| **count)
                .unwrap_or((0, &0));
            if *count > 0 { id as u8 } else { 0 }
        })
        .collect();

    // Text outside any detected bubble (e.g. a black panel the bubble model
    // missed) is grouped with its neighbours instead: pieces within
    // LOOSE_GROUP_PX of each other, and the unbubbled pixels around them.
    let loose = connected_components(
        &dilate(&pasted, Norm::LInf, LOOSE_GROUP_PX),
        Connectivity::Eight,
        Luma([0]),
    );
    let group_map: Vec<u32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let bubble = if pasted.get_pixel(x, y)[0] > 0 {
                owner[labels.get_pixel(x, y)[0] as usize]
            } else {
                bubbles.get_pixel(x, y)[0]
            };
            match (bubble, loose.get_pixel(x, y)[0]) {
                (0, 0) => 0,
                (0, label) => 256 + label,
                (id, _) => u32::from(id),
            }
        })
        .collect();
    let group_at = |x: u32, y: u32| group_map[(y * width + x) as usize];

    let mut tint = vec![[0.0f32; 3]; (width * height) as usize];
    let mut any = false;
    let mut candidates: Vec<u32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| pasted.get_pixel(x, y)[0] > 0)
        .map(|(x, y)| group_at(x, y))
        .filter(|&group| group > 0)
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    for group in candidates {
        let is_paste = |x: u32, y: u32| pasted.get_pixel(x, y)[0] > 0 && group_at(x, y) == group;
        let in_group = |x: u32, y: u32| group_at(x, y) == group;
        let original_at = |x: u32, y: u32| {
            let p = original.get_pixel(origin.0 + x, origin.1 + y).0;
            [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
        };
        let generated_at = |x: u32, y: u32| {
            let p = generated.get_pixel(x, y).0;
            [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
        };

        let (mut near_samples, mut wide_deltas, mut paste_pixels) = (Vec::new(), Vec::new(), 0);
        let (mut cx, mut cy) = (0.0f64, 0.0f64);
        for y in 0..height {
            for x in 0..width {
                if pasted.get_pixel(x, y)[0] > 0 {
                    if is_paste(x, y) {
                        paste_pixels += 1;
                        cx += f64::from(x);
                        cy += f64::from(y);
                    }
                    continue;
                }
                if !in_group(x, y) {
                    continue;
                }
                let (o, g) = (original_at(x, y), generated_at(x, y));
                let delta = [o[0] - g[0], o[1] - g[1], o[2] - g[2]];
                if near.get_pixel(x, y)[0] > 0 {
                    near_samples.push((x, y, o, delta));
                } else if wide_inner.get_pixel(x, y)[0] == 0 && wide_outer.get_pixel(x, y)[0] > 0 {
                    wide_deltas.push(delta);
                }
            }
        }
        if near_samples.len() < MIN_RING_SAMPLES
            || wide_deltas.len() < MIN_RING_SAMPLES
            || paste_pixels == 0
        {
            continue;
        }
        let bias = median3(near_samples.iter().map(|s| s.3));
        if bias.iter().all(|b| b.abs() <= HANDLED_OFFSET) {
            continue;
        }
        let agrees =
            |d: &[f32; 3], m: &[f32; 3]| (0..3).all(|c| (d[c] - m[c]).abs() <= DELTA_TOLERANCE);
        let agreement = near_samples.iter().filter(|s| agrees(&s.3, &bias)).count() as f32
            / near_samples.len() as f32;
        if agreement < MIN_DELTA_AGREEMENT || !agrees(&median3(wide_deltas.iter().copied()), &bias)
        {
            continue;
        }
        // Every side of the text must see the same tint.
        let (cx, cy) = (cx / paste_pixels as f64, cy / paste_pixels as f64);
        let mut sides: [Vec<[f32; 3]>; 4] = Default::default();
        for &(x, y, _, delta) in &near_samples {
            let side = usize::from(f64::from(x) >= cx) + 2 * usize::from(f64::from(y) >= cy);
            sides[side].push(delta);
        }
        let populated: Vec<&Vec<[f32; 3]>> = sides
            .iter()
            .filter(|s| s.len() >= MIN_SIDE_SAMPLES)
            .collect();
        if populated.len() < MIN_SIDES
            || populated
                .iter()
                .any(|side| !agrees(&median3(side.iter().copied()), &bias))
        {
            continue;
        }
        let background = median3(near_samples.iter().map(|s| s.2));
        let flat = near_samples
            .iter()
            .filter(|s| (0..3).all(|c| (s.2[c] - background[c]).abs() <= BACKGROUND_TOLERANCE))
            .count() as f32
            / near_samples.len() as f32;
        if flat < MIN_FLAT_BACKGROUND {
            continue;
        }
        let expected = [
            background[0] - bias[0],
            background[1] - bias[1],
            background[2] - bias[2],
        ];
        let mut tinted = 0;
        for y in 0..height {
            for x in 0..width {
                if pasted.get_pixel(x, y)[0] > 0 && is_paste(x, y) {
                    let g = generated_at(x, y);
                    tinted +=
                        usize::from((0..3).all(|c| (g[c] - expected[c]).abs() <= FILL_TOLERANCE));
                }
            }
        }
        if (tinted as f32) < MIN_TINTED_FILL * paste_pixels as f32 {
            continue;
        }

        tracing::info!(group, ?bias, "removing a uniform Flux2 fill tint");
        for y in 0..height {
            for x in 0..width {
                let pasted_here = pasted.get_pixel(x, y)[0] > 0;
                if (pasted_here && is_paste(x, y)) || (!pasted_here && in_group(x, y)) {
                    tint[(y * width + x) as usize] = bias;
                }
            }
        }
        any = true;
    }
    any.then_some(tint)
}

fn median3(values: impl Iterator<Item = [f32; 3]>) -> [f32; 3] {
    let mut channels: [Vec<f32>; 3] = Default::default();
    for value in values {
        for (channel, v) in channels.iter_mut().zip(value) {
            channel.push(v);
        }
    }
    channels.map(|mut channel| {
        if channel.is_empty() {
            return 0.0;
        }
        channel.sort_unstable_by(f32::total_cmp);
        channel[channel.len() / 2]
    })
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    /// A 120x120 crop of one black bubble (id 1) with text pasted in the
    /// middle, and a generated fill of `fill` everywhere.
    fn scene(fill: [u8; 3]) -> (RgbaImage, RgbaImage, GrayImage, GrayImage) {
        let original = RgbaImage::from_pixel(120, 120, Rgba([0, 0, 0, 255]));
        let generated = RgbaImage::from_pixel(120, 120, Rgba([fill[0], fill[1], fill[2], 255]));
        let paste = GrayImage::from_fn(120, 120, |x, y| {
            Luma([if (40..80).contains(&x) && (45..75).contains(&y) {
                255
            } else {
                0
            }])
        });
        let bubbles = GrayImage::from_pixel(120, 120, Luma([1]));
        (original, generated, paste, bubbles)
    }

    #[test]
    fn removes_a_strong_uniform_tint_from_a_flat_bubble() {
        let (original, generated, paste, bubbles) = scene([102, 78, 87]);
        let tint = uniform_tint(&original, &generated, &paste, &bubbles, (0, 0)).unwrap();
        assert_eq!(tint[60 * 120 + 60], [-102.0, -78.0, -87.0]);
    }

    #[test]
    fn leaves_tints_the_regular_match_handles() {
        let (original, generated, paste, bubbles) = scene([40, 28, 31]);
        assert!(uniform_tint(&original, &generated, &paste, &bubbles, (0, 0)).is_none());
    }

    #[test]
    fn leaves_generated_content_that_is_not_a_flat_tint() {
        let (original, mut generated, paste, bubbles) = scene([102, 78, 87]);
        // Flux drew something inside the text area.
        for y in 45..75 {
            for x in 40..70 {
                generated.put_pixel(x, y, Rgba([240, 240, 240, 255]));
            }
        }
        assert!(uniform_tint(&original, &generated, &paste, &bubbles, (0, 0)).is_none());
    }

    #[test]
    fn leaves_rings_that_cross_two_materials() {
        let (mut original, generated, paste, bubbles) = scene([102, 78, 87]);
        // The left half of the ring is a white outline, not the bubble.
        for y in 0..120 {
            for x in 0..60 {
                original.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        assert!(uniform_tint(&original, &generated, &paste, &bubbles, (0, 0)).is_none());
    }

    #[test]
    fn removes_a_tint_from_a_flat_panel_outside_bubbles() {
        let (original, _, paste, _) = scene([0, 0, 0]);
        let white = RgbaImage::from_pixel(120, 120, Rgba([250, 250, 250, 255]));
        let no_bubble = GrayImage::new(120, 120);
        let tint = uniform_tint(&original, &white, &paste, &no_bubble, (0, 0)).unwrap();
        assert_eq!(tint[60 * 120 + 60], [-250.0, -250.0, -250.0]);
    }

    #[test]
    fn leaves_text_over_textured_art_outside_bubbles() {
        let (_, generated, paste, _) = scene([102, 78, 87]);
        let hatched = RgbaImage::from_fn(120, 120, |x, y| {
            let v = if (x + y) % 6 < 3 { 30 } else { 200 };
            Rgba([v, v, v, 255])
        });
        let no_bubble = GrayImage::new(120, 120);
        assert!(uniform_tint(&hatched, &generated, &paste, &no_bubble, (0, 0)).is_none());
    }
}
