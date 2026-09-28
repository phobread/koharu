//! Completion of lettering the segmenter only half found.
//!
//! The segmentation model misses parts of outlined lettering (black glyphs
//! with a thick white outline over grey or dark art), dot runs and "!".
//! Inpainting then erases the found glyphs and leaves the rest. Inside each
//! text box this pass looks at the pure-black and pure-white ink components
//! and adds the ones that belong to lettering the mask already touches:
//!
//! - components partly in the mask, and outlines around them;
//! - components on the same text row as those, close to them along the row;
//! - runs of equal, evenly spaced dots, and the glyphs on their row.
//!
//! Printed lettering always sits on the opposite extreme (a white outline or
//! a plain bubble around black ink, black around white ink), while drawing
//! lines sit on mid-tone art. Components that aren't surrounded that way are
//! not recruited, which keeps hair strands and shading next to the text.
//! Boxes the segmenter found nothing in are left alone: the inpainting
//! fallback erases those whole when they sit in a bubble.

use std::collections::VecDeque;

use image::{GrayImage, Luma, RgbImage};
use imageproc::{
    distance_transform::{Norm, euclidean_squared_distance_transform},
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};

/// Ink extremes: every RGB channel at or below `DARK`, or at or above `LIGHT`.
const DARK: u8 = 60;
const LIGHT: u8 = 225;
/// Boxes hug the ink; outlines and edge glyphs need a little room.
const PAD: u32 = 6;
const MIN_AREA: u32 = 8;
/// A component is anchored when this share (and count) of it is masked.
const ANCHOR_SHARE: f32 = 0.05;
const ANCHOR_MIN: u32 = 4;
/// Weakly anchored components must also pass the ring test, unless at least
/// this share of them is masked already.
const STRONG_ANCHOR: f32 = 0.5;
/// Share of the ring around a component that must be the opposite extreme.
const RING_MIN: f32 = 0.5;
const RING_PX: u8 = 3;
/// Row matching, relative to the row's height.
const ROW_TOLERANCE: f32 = 0.15;
const MAX_ROW_HEIGHT: f32 = 1.3;
const MAX_ROW_GAP: f32 = 1.2;
/// Mask pixels a box needs before dot runs alone count as lettering.
const MASK_EVIDENCE: u32 = 20;
/// Share of an outline that must lie within the band around accepted glyphs.
const OUTLINE_BAND_SHARE: f32 = 0.9;
const COMPLETION_DILATE_RADIUS: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Polarity {
    Dark,
    Light,
}

struct Component {
    polarity: Polarity,
    /// Crop-local bounds, end-exclusive.
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    /// Own pixels and the same with enclosed holes filled, bounds-sized.
    pixels: Vec<bool>,
    filled: Vec<bool>,
    area: u32,
    ring: f32,
    anchored: bool,
    accepted: bool,
}

impl Component {
    fn width(&self) -> u32 {
        self.x1 - self.x0
    }

    fn height(&self) -> u32 {
        self.y1 - self.y0
    }

    fn centre_y(&self) -> f32 {
        (self.y0 + self.y1) as f32 / 2.0
    }

    fn fill_ratio(&self) -> f32 {
        self.filled.iter().filter(|&&p| p).count() as f32 / (self.width() * self.height()) as f32
    }

    fn contains_bounds(&self, other: &Component) -> bool {
        other.x0 >= self.x0 && other.x1 <= self.x1 && other.y0 >= self.y0 && other.y1 <= self.y1
    }

    /// Whether this component's filled shape covers at least half of `inner`.
    fn encloses(&self, inner: &Component) -> bool {
        if !self.contains_bounds(inner) {
            return false;
        }
        let mut covered = 0;
        for y in 0..inner.height() {
            for x in 0..inner.width() {
                if !inner.pixels[(y * inner.width() + x) as usize] {
                    continue;
                }
                let ox = inner.x0 + x - self.x0;
                let oy = inner.y0 + y - self.y0;
                covered += u32::from(self.filled[(oy * self.width() + ox) as usize]);
            }
        }
        covered * 2 >= inner.area
    }

    /// Long thin strokes (panel and bubble borders, hair) aren't glyphs.
    fn is_thin_curve(&self) -> bool {
        let major = self.width().max(self.height());
        let minor = self.width().min(self.height()).max(1);
        let fill = self.fill_ratio();
        (major >= 4 * minor && fill < 0.35) || (major >= 3 * minor && fill < 0.2)
    }

    fn is_dot(&self, crop_height: u32) -> bool {
        let major = self.width().max(self.height()) as f32;
        let minor = self.width().min(self.height()) as f32;
        major <= 1.6 * minor
            && self.fill_ratio() >= 0.6
            && self.height() as f32 <= 0.25 * crop_height as f32
    }
}

/// Adds the lettering pixels the segmenter missed in each text box. `mask`
/// is the refined segmentation mask (the evidence); `bounds` are the text
/// boxes. Returns the pixels to add, already dilated like the mask.
pub(super) fn complete_text_lines(
    image: &RgbImage,
    mask: &GrayImage,
    bounds: &[[u32; 4]],
) -> GrayImage {
    let (width, height) = mask.dimensions();
    let mut additions = GrayImage::new(width, height);
    if image.dimensions() != mask.dimensions() {
        return additions;
    }
    for &rect in bounds {
        let crop = [
            rect[0].saturating_sub(PAD),
            rect[1].saturating_sub(PAD),
            (rect[2] + PAD).min(width),
            (rect[3] + PAD).min(height),
        ];
        let Some(added) = complete_box(image, mask, rect, crop) else {
            continue;
        };
        let added = dilate(&added, Norm::L1, COMPLETION_DILATE_RADIUS);
        for (x, y, pixel) in added.enumerate_pixels() {
            if pixel[0] > 0 {
                additions.put_pixel(crop[0] + x, crop[1] + y, Luma([255]));
            }
        }
    }
    additions
}

/// Crop-local pixels to add for one box, or `None` when nothing is added.
fn complete_box(
    image: &RgbImage,
    mask: &GrayImage,
    rect: [u32; 4],
    [cx0, cy0, cx1, cy1]: [u32; 4],
) -> Option<GrayImage> {
    let (w, h) = (cx1.saturating_sub(cx0), cy1.saturating_sub(cy0));
    if w < 3 || h < 3 {
        return None;
    }
    let pixel = |x: u32, y: u32| image.get_pixel(cx0 + x, cy0 + y).0;
    let masked = |x: u32, y: u32| mask.get_pixel(cx0 + x, cy0 + y)[0] > 0;
    let is_dark = |x: u32, y: u32| pixel(x, y).iter().all(|&v| v <= DARK);
    let is_light = |x: u32, y: u32| pixel(x, y).iter().all(|&v| v >= LIGHT);

    let mut comps = Vec::new();
    for polarity in [Polarity::Dark, Polarity::Light] {
        let ink = GrayImage::from_fn(w, h, |x, y| {
            let on = match polarity {
                Polarity::Dark => is_dark(x, y),
                Polarity::Light => is_light(x, y),
            };
            Luma([if on { 255 } else { 0 }])
        });
        let labels = connected_components(&ink, Connectivity::Eight, Luma([0]));
        let count = labels.pixels().map(|p| p[0]).max().unwrap_or(0) as usize;
        // area, masked pixels, min x/y, max x/y (inclusive)
        let mut stats = vec![[0, 0, w, h, 0, 0]; count + 1];
        for (x, y, label) in labels.enumerate_pixels() {
            let stat = &mut stats[label[0] as usize];
            if label[0] == 0 {
                continue;
            }
            stat[0] += 1;
            stat[1] += u32::from(masked(x, y));
            stat[2] = stat[2].min(x);
            stat[3] = stat[3].min(y);
            stat[4] = stat[4].max(x);
            stat[5] = stat[5].max(y);
        }
        for (label, &[area, hits, x0, y0, x_max, y_max]) in stats.iter().enumerate().skip(1) {
            if area < MIN_AREA || x0 == 0 || y0 == 0 || x_max + 1 == w || y_max + 1 == h {
                continue;
            }
            let (x1, y1) = (x_max + 1, y_max + 1);
            let pixels: Vec<bool> = (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .map(|(x, y)| labels.get_pixel(x, y)[0] as usize == label)
                .collect();
            let filled = fill_holes(&pixels, x1 - x0, y1 - y0);
            let mut comp = Component {
                polarity,
                x0,
                y0,
                x1,
                y1,
                pixels,
                filled,
                area,
                ring: 0.0,
                anchored: false,
                accepted: false,
            };
            if comp.is_thin_curve() {
                continue;
            }
            comp.ring = ring_share(&comp, w, h, &|x, y| match polarity {
                Polarity::Dark => is_light(x, y),
                Polarity::Light => is_dark(x, y),
            });
            let share = hits as f32 / area as f32;
            // A drawing line the dilated mask merely grazes is not a glyph:
            // weakly anchored components must also look like lettering.
            comp.anchored = hits >= ANCHOR_MIN
                && share >= ANCHOR_SHARE
                && (comp.ring >= RING_MIN || share >= STRONG_ANCHOR);
            comps.push(comp);
        }
    }

    // An outline around an anchored glyph is anchored too.
    for i in 0..comps.len() {
        if comps[i].anchored {
            continue;
        }
        let encloses_anchor = comps
            .iter()
            .any(|a| a.anchored && a.polarity != comps[i].polarity && comps[i].encloses(a));
        comps[i].anchored = encloses_anchor;
    }
    for comp in comps.iter_mut() {
        comp.accepted = comp.anchored;
    }

    let anchored: Vec<usize> = (0..comps.len()).filter(|&i| comps[i].anchored).collect();
    let evidence = (rect[1]..rect[3])
        .flat_map(|y| (rect[0]..rect[2]).map(move |x| (x, y)))
        .filter(|&(x, y)| mask.get_pixel(x, y)[0] > 0)
        .count() as u32
        >= MASK_EVIDENCE;
    if anchored.is_empty() && !evidence {
        return None;
    }

    for row in merge_rows(&comps, &anchored) {
        recruit_row(&mut comps, row, &anchored);
    }
    recruit_dot_runs(&mut comps, h);
    accept_outlines(&mut comps, w, h);

    let mut added = GrayImage::new(w, h);
    let mut any = false;
    for comp in comps.iter().filter(|c| c.accepted) {
        for y in 0..comp.height() {
            for x in 0..comp.width() {
                let (px, py) = (comp.x0 + x, comp.y0 + y);
                if comp.filled[(y * comp.width() + x) as usize] && !masked(px, py) {
                    added.put_pixel(px, py, Luma([255]));
                    any = true;
                }
            }
        }
    }
    any.then_some(added)
}

/// Text rows: the merged vertical extents of `members`.
fn merge_rows(comps: &[Component], members: &[usize]) -> Vec<(u32, u32)> {
    let mut spans: Vec<(u32, u32)> = members
        .iter()
        .map(|&i| (comps[i].y0, comps[i].y1))
        .collect();
    spans.sort_unstable();
    let mut rows: Vec<(u32, u32)> = Vec::new();
    for (a, b) in spans {
        match rows.last_mut() {
            Some(last) if a < last.1 => last.1 = last.1.max(b),
            _ => rows.push((a, b)),
        }
    }
    rows
}

/// Accepts unaccepted lettering-like components centred on `row`, no taller
/// than its glyphs, chaining along the row from the `seeds` in it.
fn recruit_row(comps: &mut [Component], (row_y0, row_y1): (u32, u32), seeds: &[usize]) {
    let row_height = (row_y1 - row_y0) as f32;
    let tolerance = ROW_TOLERANCE * row_height;
    let mut members: Vec<usize> = seeds
        .iter()
        .copied()
        .filter(|&i| comps[i].y0 + 1 >= row_y0 && comps[i].y1 <= row_y1 + 1)
        .collect();
    let mut pending: Vec<usize> = (0..comps.len())
        .filter(|&i| {
            let c = &comps[i];
            !c.accepted
                && c.ring >= RING_MIN
                && c.centre_y() >= row_y0 as f32 - tolerance
                && c.centre_y() <= row_y1 as f32 + tolerance
                && c.height() as f32 <= MAX_ROW_HEIGHT * row_height
        })
        .collect();
    let max_gap = MAX_ROW_GAP * row_height;
    let mut changed = !members.is_empty();
    while changed {
        changed = false;
        pending.retain(|&i| {
            let near = members
                .iter()
                .any(|&m| horizontal_gap(&comps[m], &comps[i]) as f32 <= max_gap);
            if near {
                comps[i].accepted = true;
                members.push(i);
                changed = true;
            }
            !near
        });
    }
}

/// Runs of at least two equal, compact dots on one row with regular gaps
/// ("...", "....."), then the glyphs on their row.
fn recruit_dot_runs(comps: &mut [Component], crop_height: u32) {
    let mut dots: Vec<usize> = (0..comps.len())
        .filter(|&i| {
            !comps[i].accepted && comps[i].ring >= RING_MIN && comps[i].is_dot(crop_height)
        })
        .collect();
    dots.sort_by_key(|&i| comps[i].x0);
    let mut used = vec![false; comps.len()];
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for (k, &d) in dots.iter().enumerate() {
        if used[d] {
            continue;
        }
        let mut run = vec![d];
        for &e in &dots[k + 1..] {
            if used[e] {
                continue;
            }
            let last = &comps[*run.last().unwrap()];
            let next = &comps[e];
            let gap = next.x0 as f32 - last.x1 as f32;
            let ratio = next.area as f32 / last.area as f32;
            if (last.centre_y() - next.centre_y()).abs() <= 0.5 * last.height() as f32
                && (0.5..=2.0).contains(&ratio)
                && gap >= 0.2 * last.width() as f32
                && gap <= 4.0 * last.width() as f32
            {
                run.push(e);
            }
        }
        if run.len() < 2 {
            continue;
        }
        let gaps: Vec<i64> = run
            .windows(2)
            .map(|p| i64::from(comps[p[1]].x0) - i64::from(comps[p[0]].x1))
            .collect();
        let (min_gap, max_gap) = (*gaps.iter().min().unwrap(), *gaps.iter().max().unwrap());
        if max_gap as f32 <= 2.5 * min_gap.max(1) as f32 {
            for &i in &run {
                used[i] = true;
            }
            runs.push(run);
        }
    }
    for &i in runs.iter().flatten() {
        comps[i].accepted = true;
    }
    for run in runs {
        // The row's scale comes from the tallest component straddling the
        // dots' row, e.g. the "?!" after ".." or an outline around the run.
        let row_y0 = run.iter().map(|&i| comps[i].y0).min().unwrap();
        let row_y1 = run.iter().map(|&i| comps[i].y1).max().unwrap();
        let reach = 4 * (row_y1 - row_y0);
        let left = run
            .iter()
            .map(|&i| comps[i].x0)
            .min()
            .unwrap()
            .saturating_sub(reach);
        let right = run.iter().map(|&i| comps[i].x1).max().unwrap() + reach;
        let top = comps
            .iter()
            .filter(|c| {
                !c.accepted && c.y0 <= row_y0 && c.y1 >= row_y1 && c.x1 >= left && c.x0 <= right
            })
            .map(|c| c.y0)
            .fold(row_y0, u32::min);
        recruit_row(comps, (top, row_y1), &run);
    }
}

/// Outlines: components enclosing accepted glyphs of the other polarity, or
/// lying almost entirely within a band around the accepted glyphs.
fn accept_outlines(comps: &mut [Component], w: u32, h: u32) {
    if !comps.iter().any(|c| c.accepted) {
        return;
    }
    for i in 0..comps.len() {
        if comps[i].accepted {
            continue;
        }
        let encloses = comps
            .iter()
            .any(|a| a.accepted && a.polarity != comps[i].polarity && comps[i].encloses(a));
        comps[i].accepted = encloses;
    }

    let mut accepted = GrayImage::new(w, h);
    let mut heights = Vec::new();
    for comp in comps.iter().filter(|c| c.accepted) {
        heights.push(comp.height());
        for y in 0..comp.height() {
            for x in 0..comp.width() {
                if comp.filled[(y * comp.width() + x) as usize] {
                    accepted.put_pixel(comp.x0 + x, comp.y0 + y, Luma([255]));
                }
            }
        }
    }
    heights.sort_unstable();
    let glyph_height = heights[heights.len() / 2] as f64;
    let band = (0.3 * glyph_height).clamp(4.0, 14.0);
    let distance = euclidean_squared_distance_transform(&accepted);
    for comp in comps.iter_mut().filter(|c| !c.accepted) {
        let mut nearest = f64::INFINITY;
        let mut within = 0;
        for y in 0..comp.height() {
            for x in 0..comp.width() {
                if !comp.pixels[(y * comp.width() + x) as usize] {
                    continue;
                }
                let d = distance.get_pixel(comp.x0 + x, comp.y0 + y)[0].sqrt();
                nearest = nearest.min(d);
                within += u32::from(d <= band);
            }
        }
        if nearest <= 1.5 && within as f32 >= OUTLINE_BAND_SHARE * comp.area as f32 {
            comp.accepted = true;
        }
    }
}

/// Share of the ring just outside `comp` that is the opposite extreme.
fn ring_share(comp: &Component, w: u32, h: u32, opposite: &dyn Fn(u32, u32) -> bool) -> f32 {
    let r = u32::from(RING_PX);
    let (wx0, wy0) = (comp.x0.saturating_sub(r), comp.y0.saturating_sub(r));
    let (wx1, wy1) = ((comp.x1 + r).min(w), (comp.y1 + r).min(h));
    let shape = GrayImage::from_fn(wx1 - wx0, wy1 - wy0, |x, y| {
        let (cx, cy) = (wx0 + x, wy0 + y);
        let inside = cx >= comp.x0
            && cx < comp.x1
            && cy >= comp.y0
            && cy < comp.y1
            && comp.filled[((cy - comp.y0) * comp.width() + (cx - comp.x0)) as usize];
        Luma([if inside { 255 } else { 0 }])
    });
    let grown = dilate(&shape, Norm::LInf, RING_PX);
    let (mut ring, mut hits) = (0u32, 0u32);
    for (x, y, pixel) in grown.enumerate_pixels() {
        if pixel[0] == 0 || shape.get_pixel(x, y)[0] > 0 {
            continue;
        }
        ring += 1;
        hits += u32::from(opposite(wx0 + x, wy0 + y));
    }
    if ring == 0 {
        0.0
    } else {
        hits as f32 / ring as f32
    }
}

/// `pixels` with every background region that can't reach the bounds'
/// edge filled. The background is 4-connected, so a thin ring drawn with
/// diagonal steps still encloses its hole.
fn fill_holes(pixels: &[bool], w: u32, h: u32) -> Vec<bool> {
    let index = |x: u32, y: u32| (y * w + x) as usize;
    let mut outside = vec![false; pixels.len()];
    let mut queue = VecDeque::new();
    for y in 0..h {
        for x in 0..w {
            if (x == 0 || y == 0 || x + 1 == w || y + 1 == h) && !pixels[index(x, y)] {
                outside[index(x, y)] = true;
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        let neighbours = [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ];
        for (nx, ny) in neighbours {
            if nx < w && ny < h && !pixels[index(nx, ny)] && !outside[index(nx, ny)] {
                outside[index(nx, ny)] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    outside.iter().map(|&o| !o).collect()
}

fn horizontal_gap(a: &Component, b: &Component) -> u32 {
    a.x0.max(b.x0).saturating_sub(a.x1.min(b.x1))
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    fn fill(image: &mut RgbImage, [x0, y0, x1, y1]: [u32; 4], value: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                image.put_pixel(x, y, Rgb([value; 3]));
            }
        }
    }

    fn mark(mask: &mut GrayImage, [x0, y0, x1, y1]: [u32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
    }

    fn added(additions: &GrayImage, x: u32, y: u32) -> bool {
        additions.get_pixel(x, y)[0] > 0
    }

    /// Black glyph with a white outline on mid-grey art.
    fn outlined_glyph(image: &mut RgbImage, [x0, y0, x1, y1]: [u32; 4]) {
        fill(image, [x0 - 4, y0 - 4, x1 + 4, y1 + 4], 255);
        fill(image, [x0, y0, x1, y1], 0);
    }

    #[test]
    fn completes_an_outlined_word_the_segmenter_half_found() {
        let mut image = RgbImage::from_pixel(200, 80, Rgb([130; 3]));
        // One outline around three glyphs, as outlines merge within a word.
        fill(&mut image, [16, 16, 164, 64], 255);
        for x0 in [20, 70, 120] {
            fill(&mut image, [x0, 20, x0 + 40, 60], 0);
        }
        let mut mask = GrayImage::new(200, 80);
        mark(&mut mask, [120, 20, 160, 60]);
        let additions = complete_text_lines(&image, &mask, &[[10, 10, 190, 70]]);
        assert!(added(&additions, 40, 40)); // first glyph
        assert!(added(&additions, 90, 40)); // second glyph
        assert!(added(&additions, 17, 40)); // outline
        assert!(!added(&additions, 5, 40)); // art outside
    }

    #[test]
    fn recruits_dots_and_marks_on_the_row_of_found_glyphs() {
        let mut image = RgbImage::from_pixel(240, 120, Rgb([255; 3]));
        fill(&mut image, [20, 40, 60, 80], 0); // segmented glyph
        for x0 in [70, 85, 100] {
            fill(&mut image, [x0, 72, x0 + 6, 78], 0); // "..."
        }
        fill(&mut image, [115, 40, 121, 70], 0); // "!" bar
        fill(&mut image, [115, 73, 121, 79], 0); // "!" dot
        fill(&mut image, [20, 12, 26, 18], 0); // a mark above the row
        let mut mask = GrayImage::new(240, 120);
        mark(&mut mask, [20, 40, 60, 80]);
        let additions = complete_text_lines(&image, &mask, &[[10, 6, 230, 110]]);
        assert!(added(&additions, 72, 75));
        assert!(added(&additions, 102, 75));
        assert!(added(&additions, 118, 55));
        assert!(!added(&additions, 22, 14));
    }

    #[test]
    fn leaves_drawing_lines_on_mid_tone_art() {
        let mut image = RgbImage::from_pixel(240, 100, Rgb([130; 3]));
        outlined_glyph(&mut image, [20, 30, 60, 70]);
        // A hair strand on the same row, with no outline.
        fill(&mut image, [80, 45, 110, 53], 0);
        let mut mask = GrayImage::new(240, 100);
        mark(&mut mask, [16, 26, 64, 74]);
        let additions = complete_text_lines(&image, &mask, &[[10, 10, 230, 90]]);
        assert!(!added(&additions, 95, 49));
    }

    /// Opt-in: compares per-box additions with the Python prototype the rule
    /// was tuned with. `KOHARU_LETTERING_PARITY` = a JSON list of
    /// `{source, segment, page, box, rect, added}` entries.
    #[test]
    #[ignore]
    fn matches_the_prototype_on_real_pages() {
        let path = std::env::var("KOHARU_LETTERING_PARITY").expect("KOHARU_LETTERING_PARITY");
        let entries: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut pages: std::collections::HashMap<String, (RgbImage, GrayImage)> =
            Default::default();
        let mut mismatches = Vec::new();
        for entry in &entries {
            let source = entry["source"].as_str().unwrap().to_string();
            let (image, mask) = pages.entry(source.clone()).or_insert_with(|| {
                let open = |path: &str| {
                    image::ImageReader::open(path)
                        .unwrap()
                        .with_guessed_format()
                        .unwrap()
                        .decode()
                        .unwrap()
                };
                let image = open(&source).to_rgb8();
                let segment = open(entry["segment"].as_str().unwrap());
                let mut mask = segment.to_luma8();
                for pixel in mask.pixels_mut() {
                    pixel[0] = if pixel[0] > 127 { 255 } else { 0 };
                }
                (image, mask)
            });
            let rect: Vec<u32> = entry["rect"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u32)
                .collect();
            let rect = [rect[0], rect[1], rect[2], rect[3]];
            let (width, height) = mask.dimensions();
            let crop = [
                rect[0].saturating_sub(PAD),
                rect[1].saturating_sub(PAD),
                (rect[2] + PAD).min(width),
                (rect[3] + PAD).min(height),
            ];
            let added = complete_box(image, mask, rect, crop)
                .map_or(0, |a| a.pixels().filter(|p| p[0] > 0).count() as u64);
            let expected = entry["added"].as_u64().unwrap();
            // Float rounding at the outline band's edge can move a pixel.
            if added.abs_diff(expected) > expected / 1000 {
                mismatches.push(format!(
                    "{} box {}: rust {added} python {expected}",
                    entry["page"], entry["box"]
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} of {} boxes differ:\n{}",
            mismatches.len(),
            entries.len(),
            mismatches.join("\n")
        );
    }

    #[test]
    fn dot_runs_need_mask_evidence_in_the_box() {
        let mut image = RgbImage::from_pixel(160, 60, Rgb([255; 3]));
        for x0 in [30, 50, 70, 90] {
            fill(&mut image, [x0, 26, x0 + 8, 34], 0);
        }
        let rect = [[10, 10, 150, 50]];
        let empty = GrayImage::new(160, 60);
        assert!(
            complete_text_lines(&image, &empty, &rect)
                .pixels()
                .all(|p| p[0] == 0)
        );

        // The segmenter marked a patch next to the dots, none of the dots.
        let mut mask = GrayImage::new(160, 60);
        mark(&mut mask, [110, 22, 120, 38]);
        let additions = complete_text_lines(&image, &mask, &rect);
        assert!(added(&additions, 34, 30));
        assert!(added(&additions, 94, 30));
    }
}
