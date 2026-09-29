//! Clean thick-outlined lettering into plain black-on-white text for OCR.
//!
//! Some pages letter dialogue straight over the artwork in coloured or dark
//! text wrapped in a thick white outline. Both OCR readers struggle with it:
//! PaddleOCR-VL confuses look-alike syllables (디/티, 다/타, 빽/빡) and the
//! Korean line splitter finds no ink at all in mid-luminance colours (purple
//! text is neither dark nor light). Rebuilding such a box as black glyphs on
//! white before recognition fixes most of those misreads.
//!
//! The outline itself tells glyph from background: the fill of each glyph is
//! the first non-white layer nested inside a white layer, while a glyph's
//! counters (the hole of ㅇ) sit one layer deeper again. Background trapped
//! between merged outlines ("pockets") nests like fill; where its colour
//! clearly matches the surrounding background rather than the neighbouring
//! glyphs it is dropped. Speech bubbles (mostly white inside the box) are left
//! alone.
//!
//! Measured through the app's own OCR on five projects (owner-proofread or
//! visually keyed, one held out with the rule frozen): Hangul errors fell by
//! roughly 40 %, with no project getting worse.

use std::collections::VecDeque;

use image::{GrayImage, Luma, Rgb, RgbImage};
use imageproc::distance_transform::euclidean_squared_distance_transform;

/// Near-white: bright and unsaturated.
const WHITE_MIN_LUMA: i32 = 200;
const WHITE_MAX_SATURATION: i32 = 60;
/// Outlined lettering leaves little white inside its box (a thin band around
/// each glyph); a speech bubble is mostly white.
const MAX_WHITE_FRACTION: f64 = 0.5;
/// Components smaller than this share of the window are noise.
const MIN_AREA_FRACTION: f64 = 0.00004;
/// Below this share of fill inside the box it isn't lettering.
const MIN_INK_FRACTION: f64 = 0.01;
/// Pocket test: neighbourhood searched around a component, and how far apart
/// fill and background colours must be before colour is trusted at all.
const POCKET_RADIUS: usize = 40;
const POCKET_MIN_SEPARATION: f64 = 40.0;

/// The window around a text box to clean: the box's axis-aligned bounds
/// padded by 12 % of its longer side plus 8 px, clamped to the image.
/// Returns `(window, box)` as `[x0, y0, x1, y1)` rects in image coordinates.
pub fn outline_window(
    image_width: u32,
    image_height: u32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> ([u32; 4], [u32; 4]) {
    let x0 = (min_x.max(0.0) as u32).min(image_width);
    let y0 = (min_y.max(0.0) as u32).min(image_height);
    let x1 = (max_x.ceil().max(0.0) as u32).clamp(x0, image_width);
    let y1 = (max_y.ceil().max(0.0) as u32).clamp(y0, image_height);
    let pad = ((x1 - x0).max(y1 - y0) as f64 * 0.12) as u32 + 8;
    (
        [
            x0.saturating_sub(pad),
            y0.saturating_sub(pad),
            (x1 + pad).min(image_width),
            (y1 + pad).min(image_height),
        ],
        [x0, y0, x1, y1],
    )
}

/// Clean `window` if the text box `bbox` (window coordinates, `[x0, y0, x1,
/// y1)`) holds outlined lettering: returns the window redrawn as black glyph
/// fill on white, or `None` to keep the original (a speech bubble, or no
/// plausible fill found).
pub fn clean_outlined_text(window: &RgbImage, bbox: [u32; 4]) -> Option<RgbImage> {
    let (width, height) = window.dimensions();
    let [bx0, by0, bx1, by1] = bbox;
    if width == 0 || height == 0 || bx1 <= bx0 || by1 <= by0 || bx1 > width || by1 > height {
        return None;
    }
    let w = width as usize;
    let h = height as usize;
    let white: Vec<bool> = window.pixels().map(is_white).collect();

    let box_pixels = ((bx1 - bx0) * (by1 - by0)) as f64;
    let box_white = (by0..by1)
        .flat_map(|y| (bx0..bx1).map(move |x| (x, y)))
        .filter(|&(x, y)| white[y as usize * w + x as usize])
        .count() as f64;
    if box_white / box_pixels > MAX_WHITE_FRACTION {
        return None;
    }

    let labels = Components::label(&white, w, h);
    let min_area = ((MIN_AREA_FRACTION * (w * h) as f64) as usize).max(6);
    let mut fill: Vec<bool> = labels
        .fill_ids()
        .into_iter()
        .zip(&labels.areas)
        .map(|(is_fill, &area)| is_fill && area >= min_area)
        .collect();
    if !fill.iter().any(|&f| f) {
        return None;
    }
    drop_color_pockets(window, &white, &labels, &mut fill);

    let ink: Vec<bool> = labels.comp.iter().map(|&c| fill[c as usize]).collect();
    let box_ink = (by0..by1)
        .flat_map(|y| (bx0..bx1).map(move |x| (x, y)))
        .filter(|&(x, y)| ink[y as usize * w + x as usize])
        .count() as f64;
    if box_ink / box_pixels < MIN_INK_FRACTION {
        return None;
    }
    let mut out = RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));
    for (i, &is_ink) in ink.iter().enumerate() {
        if is_ink {
            out.put_pixel((i % w) as u32, (i / w) as u32, Rgb([0, 0, 0]));
        }
    }
    Some(out)
}

fn is_white(pixel: &Rgb<u8>) -> bool {
    let [r, g, b] = pixel.0.map(i32::from);
    let luma = (299 * r + 587 * g + 114 * b) / 1000;
    let saturation = r.max(g).max(b) - r.min(g).min(b);
    luma >= WHITE_MIN_LUMA && saturation <= WHITE_MAX_SATURATION
}

/// White (8-connected) and non-white (4-connected) components, with each
/// component's nesting depth: components touching the border are depth 1,
/// then +1 per boundary crossed.
struct Components {
    width: usize,
    height: usize,
    /// Component id per pixel (ids start at 1).
    comp: Vec<u32>,
    /// Per id (index 0 unused).
    is_white: Vec<bool>,
    areas: Vec<usize>,
    depth: Vec<u32>,
    parent: Vec<u32>,
    /// Inclusive pixel bounds `[x0, y0, x1, y1]` per id.
    bounds: Vec<[usize; 4]>,
}

impl Components {
    fn label(white: &[bool], width: usize, height: usize) -> Self {
        let mut comp = vec![0_u32; width * height];
        let mut is_white = vec![false];
        let mut areas = vec![0];
        let mut bounds = vec![[0; 4]];
        let mut queue = VecDeque::new();
        for start in 0..width * height {
            if comp[start] != 0 {
                continue;
            }
            let id = is_white.len() as u32;
            let class = white[start];
            is_white.push(class);
            comp[start] = id;
            queue.push_back(start);
            let mut area = 0;
            let mut b = [usize::MAX, usize::MAX, 0, 0];
            while let Some(i) = queue.pop_front() {
                area += 1;
                let (x, y) = (i % width, i / width);
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                for (dx, dy) in NEIGHBOURS_8 {
                    // White joins diagonally; non-white only orthogonally, so
                    // a one-pixel diagonal gap in an outline still separates.
                    if !class && dx != 0 && dy != 0 {
                        continue;
                    }
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                        continue;
                    }
                    let j = ny as usize * width + nx as usize;
                    if comp[j] == 0 && white[j] == class {
                        comp[j] = id;
                        queue.push_back(j);
                    }
                }
            }
            areas.push(area);
            bounds.push(b);
        }

        let n = is_white.len();
        let mut adjacency = vec![Vec::new(); n];
        let mut link = |a: u32, b: u32| {
            if a != b {
                adjacency[a as usize].push(b);
                adjacency[b as usize].push(a);
            }
        };
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                if x + 1 < width {
                    link(comp[i], comp[i + 1]);
                }
                if y + 1 < height {
                    link(comp[i], comp[i + width]);
                }
            }
        }
        for list in &mut adjacency {
            list.sort_unstable();
            list.dedup();
        }

        let mut depth = vec![0_u32; n];
        let mut parent = vec![0_u32; n];
        let mut frontier: Vec<u32> = (0..width)
            .flat_map(|x| [comp[x], comp[(height - 1) * width + x]])
            .chain((0..height).flat_map(|y| [comp[y * width], comp[y * width + width - 1]]))
            .collect();
        frontier.sort_unstable();
        frontier.dedup();
        for &c in &frontier {
            depth[c as usize] = 1;
        }
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for &c in &frontier {
                for &d in &adjacency[c as usize] {
                    if depth[d as usize] == 0 {
                        depth[d as usize] = depth[c as usize] + 1;
                        parent[d as usize] = c;
                        next.push(d);
                    }
                }
            }
            frontier = next;
        }

        Self {
            width,
            height,
            comp,
            is_white,
            areas,
            depth,
            parent,
            bounds,
        }
    }

    /// Glyph fill: a non-white component directly inside a white one whose
    /// own parent is not fill. Counters (inside a glyph's inner outline) are
    /// not fill. Works whatever depth the outline starts at, since an outline
    /// may touch the window border.
    fn fill_ids(&self) -> Vec<bool> {
        let n = self.is_white.len();
        let mut order: Vec<usize> = (1..n).collect();
        order.sort_by_key(|&c| self.depth[c]);
        let mut fill = vec![false; n];
        for c in order {
            if self.is_white[c] || self.depth[c] <= 1 {
                continue;
            }
            let p = self.parent[c] as usize;
            let g = if self.depth[p] > 1 {
                self.parent[p] as usize
            } else {
                0
            };
            fill[c] = !(g != 0 && !self.is_white[g] && fill[g]);
        }
        fill
    }
}

const NEIGHBOURS_8: [(isize, isize); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Drop background pockets trapped between merged outlines: fill candidates
/// whose colour is closer to the local background than to the neighbouring
/// fill, when those two differ clearly. Compared locally so gradient lettering
/// (blue fading to near-black) keeps its dark letters, and on "core" pixels
/// away from the white, since the outline's soft edge brightens its neighbours.
/// Where fill and background share a colour (black on black) nothing is
/// dropped.
fn drop_color_pockets(window: &RgbImage, white: &[bool], labels: &Components, fill: &mut [bool]) {
    let (w, h) = (labels.width, labels.height);
    let candidates: Vec<usize> = (1..fill.len()).filter(|&c| fill[c]).collect();
    if candidates.len() < 2 {
        return;
    }
    let white_image = GrayImage::from_fn(w as u32, h as u32, |x, y| {
        Luma([u8::from(white[y as usize * w + x as usize]) * 255])
    });
    let to_white: Vec<f64> = euclidean_squared_distance_transform(&white_image)
        .pixels()
        .map(|p| p[0].sqrt())
        .collect();
    let core = |i: usize| to_white[i] >= 2.0;
    let background_core = |i: usize| {
        !white[i]
            && labels.depth[labels.comp[i] as usize] == 1
            && (3.0..=8.0).contains(&to_white[i])
    };
    let candidate_pixel = |i: usize| fill[labels.comp[i] as usize];
    let raw = window.as_raw();
    let colour = |i: usize| [raw[i * 3], raw[i * 3 + 1], raw[i * 3 + 2]];
    let candidate_mask: Vec<bool> = (0..w * h).map(candidate_pixel).collect();

    let mut dropped = Vec::new();
    for &c in &candidates {
        let [cx0, cy0, cx1, cy1] = labels.bounds[c];
        let mut own = Vec::new();
        let mut own_core = Vec::new();
        for y in cy0..=cy1 {
            for x in cx0..=cx1 {
                let i = y * w + x;
                if labels.comp[i] as usize == c {
                    own.push(colour(i));
                    if core(i) {
                        own_core.push(colour(i));
                    }
                }
            }
        }
        let own_colour = median_colour(if own_core.is_empty() { own } else { own_core });

        let x0 = cx0.saturating_sub(POCKET_RADIUS);
        let y0 = cy0.saturating_sub(POCKET_RADIUS);
        let x1 = (cx1 + 1 + POCKET_RADIUS).min(w);
        let y1 = (cy1 + 1 + POCKET_RADIUS).min(h);
        let mut ring = Vec::new();
        let mut others = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * w + x;
                if background_core(i) {
                    ring.push(colour(i));
                }
                if candidate_mask[i] && labels.comp[i] as usize != c && core(i) {
                    others.push(colour(i));
                }
            }
        }
        if ring.is_empty() || others.is_empty() {
            continue;
        }
        let background = median_colour(ring);
        let neighbours = median_colour(others);
        if distance(neighbours, background) <= POCKET_MIN_SEPARATION {
            continue;
        }
        if distance(own_colour, background) < distance(own_colour, neighbours) {
            dropped.push(c);
        }
    }
    for c in dropped {
        fill[c] = false;
    }
}

/// Per-channel median (the mean of the two middle values for even counts).
fn median_colour(pixels: Vec<[u8; 3]>) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (channel, value) in out.iter_mut().enumerate() {
        let mut v: Vec<u8> = pixels.iter().map(|p| p[channel]).collect();
        let n = v.len();
        v.sort_unstable();
        *value = if n % 2 == 1 {
            f64::from(v[n / 2])
        } else {
            (f64::from(v[n / 2 - 1]) + f64::from(v[n / 2])) / 2.0
        };
    }
    out
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PURPLE: Rgb<u8> = Rgb([110, 90, 220]);
    const GREY: Rgb<u8> = Rgb([120, 120, 120]);

    /// A 120x80 window of `background` with an outlined "ㅁ"-like glyph: a
    /// square ring of `ink`, 6 px thick, wrapped in a 6 px white outline on
    /// both sides (so its counter is background inside an inner outline).
    fn outlined_ring(background: Rgb<u8>, ink: Rgb<u8>) -> RgbImage {
        RgbImage::from_fn(120, 80, |x, y| {
            let (x, y) = (x as i32, y as i32);
            // Ring spans x 34..86, y 14..66; strokes 6 px.
            let outer = (34..86).contains(&x) && (14..66).contains(&y);
            let inner = (40..80).contains(&x) && (20..60).contains(&y);
            let ring = outer && !inner;
            let dist_to_ring = if ring {
                0
            } else if !outer {
                (34 - x).max(x - 85).max(14 - y).max(y - 65)
            } else {
                (x - 39).min(80 - x).min(y - 19).min(60 - y)
            };
            if ring {
                ink
            } else if dist_to_ring <= 6 {
                Rgb([255, 255, 255])
            } else {
                background
            }
        })
    }

    #[test]
    fn coloured_outlined_glyph_becomes_black_on_white_with_its_counter_white() {
        let window = outlined_ring(GREY, PURPLE);
        let out = clean_outlined_text(&window, [20, 5, 100, 75]).expect("outlined lettering");
        assert_eq!(out.get_pixel(36, 40), &Rgb([0, 0, 0]), "stroke is ink");
        assert_eq!(
            out.get_pixel(60, 40),
            &Rgb([255, 255, 255]),
            "counter stays white"
        );
        assert_eq!(
            out.get_pixel(5, 5),
            &Rgb([255, 255, 255]),
            "background is white"
        );
    }

    #[test]
    fn a_white_speech_bubble_is_left_alone() {
        let window = outlined_ring(Rgb([255, 255, 255]), Rgb([0, 0, 0]));
        assert!(clean_outlined_text(&window, [20, 5, 100, 75]).is_none());
    }

    #[test]
    fn plain_art_without_lettering_is_left_alone() {
        let window = RgbImage::from_pixel(100, 100, GREY);
        assert!(clean_outlined_text(&window, [10, 10, 90, 90]).is_none());
    }

    #[test]
    fn a_background_pocket_between_outlines_is_dropped_when_its_colour_matches() {
        // Two purple bars whose outlines merge and trap a grey gap between
        // them; the gap is enclosed like fill but coloured like background.
        let window = RgbImage::from_fn(140, 80, |x, y| {
            let bar = ((30..50).contains(&x) || (62..82).contains(&x)) && (20..60).contains(&y);
            let near = (24..88).contains(&x) && (14..66).contains(&y);
            let gap = (56..57).contains(&x) && (26..54).contains(&y);
            if bar {
                PURPLE
            } else if gap {
                GREY
            } else if near {
                Rgb([255, 255, 255])
            } else {
                GREY
            }
        });
        let out = clean_outlined_text(&window, [20, 10, 92, 70]).expect("outlined lettering");
        assert_eq!(out.get_pixel(40, 40), &Rgb([0, 0, 0]));
        assert_eq!(out.get_pixel(70, 40), &Rgb([0, 0, 0]));
        assert_eq!(
            out.get_pixel(56, 40),
            &Rgb([255, 255, 255]),
            "pocket dropped"
        );
    }

    #[test]
    fn outline_window_pads_and_clamps() {
        let (window, bbox) = outline_window(1000, 1000, 10.0, 500.0, 110.0, 550.4);
        assert_eq!(bbox, [10, 500, 110, 551]);
        // pad = 100 * 0.12 + 8 = 20
        assert_eq!(window, [0, 480, 130, 571]);
    }
}
