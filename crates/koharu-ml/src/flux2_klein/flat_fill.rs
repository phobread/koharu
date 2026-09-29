//! Flat-background shortcut for Flux2 inpainting.
//!
//! Text on a plain background of any colour doesn't need a diffusion model:
//! the pixels under the letters are the background's colour. For each
//! planned crop, [`classify_crop`] decides conservatively, per text block,
//! whether the background around its letters is flat. A block is a
//! connected piece of the generation region inside one bubble (or outside
//! every bubble). Only when every block in the crop is flat is the crop
//! filled with those colours instead of running Flux2; anything uncertain
//! is left to Flux2. Filling also keeps typed symbols such as hearts from
//! coming back: Flux2 sees the original page and sometimes redraws them.
//!
//! Safeguards against filling artwork:
//! - Only pixels in the paste mask change, the same pixels Flux2 would
//!   repaint, so a fill can never spread past the lettering.
//! - A block inside a detected speech bubble is judged on the bubble's own
//!   background visible between and around the letters (its generation
//!   region and a thin ring outside the paste mask, minus a guard band
//!   hugging the glyphs).
//! - A block outside bubbles (narration lettered on a panel, text the bubble
//!   model missed) is judged on the ring around its letters alone. The ring
//!   must show the colour on at least three sides and again further out, so
//!   a small flat patch inside artwork doesn't pass, and nothing at all may
//!   be off-colour next to the letters (artwork is common around such text).
//!   Its colour tolerance is also halved: soft shading a few levels off
//!   white passes the bubble tolerance, and filling it flat leaves visible
//!   letter-shaped patches.
//! - Nearly all of the evidence must sit within a tight tolerance of one
//!   colour, so screentone, outlines and most gradients and shading fail.
//!   Structure fainter than the tolerance can still pass.
//! - Anything off-colour right next to the letters means Flux2: a line or
//!   shape that disappears under the text shows up there. Inside bubbles a
//!   few stray specks are tolerated.
//! - Too little visible background (for example a block erased whole because
//!   no glyph pixels were segmented) means Flux2.

use image::{GrayImage, ImageBuffer, Luma, Rgb, RgbImage, RgbaImage};
use imageproc::{
    distance_transform::Norm,
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};

/// Pixels next to the glyphs are skipped: anti-aliasing and JPEG ringing
/// there say nothing about the background.
const GUARD_PX: u8 = 2;
/// Width of the ring sampled outside the paste mask.
const RING_PX: u8 = 8;
/// Minimum background samples per block before trusting it.
const MIN_SAMPLES: usize = 400;
/// A sample is "on the colour" when every channel is within this of the
/// block's median.
const COLOUR_TOLERANCE: u8 = 12;
/// The same for blocks outside bubbles. Plain panels on real pages sit
/// within 3 levels of their colour (a banded dark gradient on 926 p18
/// reaches 8), while faint grey swooshes behind narration on 8.26 p14 put
/// 30% of the ring more than 6 off.
const LOCAL_COLOUR_TOLERANCE: u8 = 6;
/// Share of samples that must be on the colour.
const MIN_UNIFORM_SHARE: f64 = 0.99;
/// Band just outside the guard band where anything off-colour suggests
/// artwork continuing under the letters.
const CONTACT_PX: u8 = GUARD_PX + 4;
/// Off-colour pixels tolerated in the contact band of a block in a bubble:
/// this many, or this share of the band, whichever is larger. Blocks outside
/// bubbles tolerate none: on real pages their flat backgrounds show no
/// specks there, while the tip of a thin art line the paste mask half
/// covers shows as a handful.
const MAX_CONTACT_OFF_PX: usize = 6;
const MAX_CONTACT_OFF_SHARE: f64 = 0.003;
/// Blocks outside bubbles: the ring is split into quadrants around the
/// letters' centroid, and this many must each hold enough samples that are
/// nearly all on the colour.
const MIN_SIDES: usize = 3;
const MIN_SIDE_SAMPLES: usize = 32;
const MIN_SIDE_SHARE: f64 = 0.98;
/// Blocks outside bubbles: the ring from `RING_PX` out to this distance must
/// mostly be the colour as well.
const WIDE_RING_PX: u8 = 16;
const MIN_WIDE_SAMPLES: usize = 200;
const MIN_WIDE_SHARE: f64 = 0.95;

/// What a block's background was judged on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Evidence {
    /// The block lies in this bubble.
    Bubble(u8),
    /// The block is outside bubbles: its own surroundings.
    Local,
}

impl Evidence {
    fn tolerance(self) -> u8 {
        match self {
            Evidence::Bubble(_) => COLOUR_TOLERANCE,
            Evidence::Local => LOCAL_COLOUR_TOLERANCE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct BlockColour {
    pub evidence: Evidence,
    pub colour: [u8; 3],
    pub samples: usize,
    pub uniform_share: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Rejection {
    TooFewSamples {
        evidence: Evidence,
        samples: usize,
    },
    NotUniform {
        evidence: Evidence,
        uniform_share: f64,
    },
    /// Something off-colour touches the letters and may continue under them.
    ContactNotUniform {
        evidence: Evidence,
        off_pixels: usize,
    },
    /// Too few sides of a block outside bubbles show the colour.
    TooFewSides {
        sides: usize,
    },
    /// The wider ring around a block outside bubbles isn't the colour.
    WideRingDiffers {
        samples: usize,
        uniform_share: f64,
    },
}

/// The per-block decisions for one crop.
#[derive(Debug, Clone)]
pub(super) struct CropFill {
    /// Block number (1-based) per crop pixel, 0 outside every block.
    blocks: ImageBuffer<Luma<u32>, Vec<u32>>,
    decisions: Vec<Result<BlockColour, Rejection>>,
}

impl CropFill {
    /// Every block is flat (or there is nothing to paste): Flux2 isn't needed.
    pub(super) fn is_all_flat(&self) -> bool {
        self.decisions.iter().all(Result::is_ok)
    }

    pub(super) fn flat_blocks(&self) -> Vec<BlockColour> {
        self.decisions.iter().filter_map(|d| d.ok()).collect()
    }

    pub(super) fn rejections(&self) -> Vec<Rejection> {
        self.decisions.iter().filter_map(|d| d.err()).collect()
    }

    fn colour_at(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        let block = usize::try_from(self.blocks.get_pixel(x, y).0[0]).ok()?;
        let decision = self.decisions.get(block.checked_sub(1)?)?;
        decision.as_ref().ok().map(|c| c.colour)
    }

    /// The crop Flux2 would have produced when every block is flat: each
    /// block painted with its colour and `image` (the crop as it is now)
    /// everywhere else. Only pasted pixels are composited, but the colour
    /// match compares the ring around them with the original, so pixels
    /// outside every block must not take some block's colour.
    pub(super) fn flat_crop(&self, image: &RgbaImage) -> RgbImage {
        RgbImage::from_fn(self.blocks.width(), self.blocks.height(), |x, y| {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            Rgb(self.colour_at(x, y).unwrap_or([r, g, b]))
        })
    }
}

/// Decide how to fill one crop. All images are crop-local and the same size:
/// `image` is what is on the page now, `generation` the Flux2 region mask,
/// `paste` the pixels that will be replaced, `bubbles` the bubble-ID map.
pub(super) fn classify_crop(
    image: &RgbaImage,
    generation: &GrayImage,
    paste: &GrayImage,
    bubbles: &GrayImage,
) -> CropFill {
    let (width, height) = paste.dimensions();
    let paste = binarize(paste);
    let guard = dilate(&paste, Norm::LInf, GUARD_PX);
    let contact = dilate(&paste, Norm::LInf, CONTACT_PX);

    // Blocks: connected generation/paste pixels sharing a bubble ID.
    let region: ImageBuffer<Luma<u16>, Vec<u16>> = ImageBuffer::from_fn(width, height, |x, y| {
        let inside = generation.get_pixel(x, y).0[0] > 0 || paste.get_pixel(x, y).0[0] > 0;
        Luma([if inside {
            u16::from(bubbles.get_pixel(x, y).0[0]) + 1
        } else {
            0
        }])
    });
    let labels = connected_components(&region, Connectivity::Eight, Luma([0u16]));

    // Only blocks with pasted pixels count; they are renumbered 1.. .
    let label_count = labels.pixels().map(|p| p.0[0]).max().unwrap_or(0) as usize;
    let mut stats = vec![BlockStats::default(); label_count + 1];
    for (x, y, pasted) in paste.enumerate_pixels() {
        if pasted.0[0] > 0 {
            let label = labels.get_pixel(x, y).0[0] as usize;
            stats[label].add(x, y, bubbles.get_pixel(x, y).0[0]);
        }
    }
    let mut block_of_label = vec![0u32; label_count + 1];
    let mut found = Vec::new();
    for (label, stat) in stats.into_iter().enumerate().skip(1) {
        if stat.pixels > 0 {
            found.push((label as u32, stat));
            block_of_label[label] = found.len() as u32;
        }
    }
    let blocks = ImageBuffer::from_fn(width, height, |x, y| {
        Luma([block_of_label[labels.get_pixel(x, y).0[0] as usize]])
    });

    let decisions = found
        .iter()
        .map(|(label, stat)| {
            let block = Block {
                image,
                generation,
                paste: &paste,
                bubbles,
                guard: &guard,
                contact: &contact,
                labels: &labels,
                label: *label,
                stat,
            };
            match stat.bubble {
                0 => block.judge_local(),
                bubble => block.judge_in_bubble(bubble),
            }
        })
        .collect();
    CropFill { blocks, decisions }
}

#[derive(Debug, Clone, Default)]
struct BlockStats {
    pixels: u64,
    sum_x: u64,
    sum_y: u64,
    min: (u32, u32),
    max: (u32, u32),
    /// The bubble ID, shared by every pixel of a block.
    bubble: u8,
}

impl BlockStats {
    fn add(&mut self, x: u32, y: u32, bubble: u8) {
        if self.pixels == 0 {
            self.min = (x, y);
            self.max = (x, y);
            self.bubble = bubble;
        } else {
            self.min = (self.min.0.min(x), self.min.1.min(y));
            self.max = (self.max.0.max(x), self.max.1.max(y));
        }
        self.pixels += 1;
        self.sum_x += u64::from(x);
        self.sum_y += u64::from(y);
    }

    /// The pasted pixels' bounding box grown by `margin`, clipped to the
    /// crop, as `(x0, y0, x1, y1)` with exclusive ends.
    fn window(&self, margin: u8, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let m = u32::from(margin);
        (
            self.min.0.saturating_sub(m),
            self.min.1.saturating_sub(m),
            (self.max.0 + m + 1).min(width),
            (self.max.1 + m + 1).min(height),
        )
    }
}

struct Block<'a> {
    image: &'a RgbaImage,
    generation: &'a GrayImage,
    paste: &'a GrayImage,
    bubbles: &'a GrayImage,
    guard: &'a GrayImage,
    contact: &'a GrayImage,
    labels: &'a ImageBuffer<Luma<u32>, Vec<u32>>,
    label: u32,
    stat: &'a BlockStats,
}

/// A background sample: colour, crop position, and whether it sits in the
/// contact band.
struct Sample {
    colour: [u8; 3],
    x: u32,
    y: u32,
    near: bool,
}

impl Block<'_> {
    /// This block's pasted pixels grown by `radius`, over `window`; the
    /// result is window-local.
    fn grown_paste(&self, window: (u32, u32, u32, u32), radius: u8) -> GrayImage {
        let (x0, y0, x1, y1) = window;
        let own = GrayImage::from_fn(x1 - x0, y1 - y0, |x, y| {
            let (px, py) = (x0 + x, y0 + y);
            let mine = self.paste.get_pixel(px, py).0[0] > 0
                && self.labels.get_pixel(px, py).0[0] == self.label;
            Luma([if mine { 255 } else { 0 }])
        });
        dilate(&own, Norm::LInf, radius)
    }

    fn sample(&self, x: u32, y: u32) -> Sample {
        let [r, g, b, _] = self.image.get_pixel(x, y).0;
        Sample {
            colour: [r, g, b],
            x,
            y,
            near: self.contact.get_pixel(x, y).0[0] > 0,
        }
    }

    /// Judged on the bubble's background in the block's generation region
    /// and the ring around its letters.
    fn judge_in_bubble(&self, bubble: u8) -> Result<BlockColour, Rejection> {
        let (width, height) = self.paste.dimensions();
        let window = self.stat.window(RING_PX, width, height);
        let ring = self.grown_paste(window, RING_PX);
        let in_ring = |x: u32, y: u32| {
            x >= window.0
                && y >= window.1
                && x < window.2
                && y < window.3
                && ring.get_pixel(x - window.0, y - window.1).0[0] > 0
        };
        let mut samples = Vec::new();
        for y in 0..height {
            for x in 0..width {
                if self.bubbles.get_pixel(x, y).0[0] != bubble
                    || self.guard.get_pixel(x, y).0[0] > 0
                {
                    continue;
                }
                let in_region = self.generation.get_pixel(x, y).0[0] > 0
                    && self.labels.get_pixel(x, y).0[0] == self.label;
                if in_region || in_ring(x, y) {
                    samples.push(self.sample(x, y));
                }
            }
        }
        judge(Evidence::Bubble(bubble), &samples)
    }

    /// Judged on the ring around the letters alone, which must show the
    /// colour on several sides and again further out.
    fn judge_local(&self) -> Result<BlockColour, Rejection> {
        let (width, height) = self.paste.dimensions();
        let window = self.stat.window(WIDE_RING_PX, width, height);
        let ring = self.grown_paste(window, RING_PX);
        let wide = self.grown_paste(window, WIDE_RING_PX);
        let (mut samples, mut wide_samples) = (Vec::new(), Vec::new());
        for y in window.1..window.3 {
            for x in window.0..window.2 {
                if self.guard.get_pixel(x, y).0[0] > 0 {
                    continue;
                }
                let (wx, wy) = (x - window.0, y - window.1);
                if ring.get_pixel(wx, wy).0[0] > 0 {
                    samples.push(self.sample(x, y));
                } else if wide.get_pixel(wx, wy).0[0] > 0 {
                    wide_samples.push(self.sample(x, y).colour);
                }
            }
        }
        let fill = judge(Evidence::Local, &samples)?;
        let on_colour = |c: &[u8; 3]| is_on_colour(c, fill.colour, Evidence::Local.tolerance());

        let centre = (
            self.stat.sum_x as f64 / self.stat.pixels as f64,
            self.stat.sum_y as f64 / self.stat.pixels as f64,
        );
        let mut sides = [(0usize, 0usize); 4];
        for s in &samples {
            let side = usize::from(f64::from(s.x) >= centre.0)
                + 2 * usize::from(f64::from(s.y) >= centre.1);
            sides[side].0 += 1;
            sides[side].1 += usize::from(on_colour(&s.colour));
        }
        let good_sides = sides
            .iter()
            .filter(|&&(n, on)| n >= MIN_SIDE_SAMPLES && on as f64 / n as f64 >= MIN_SIDE_SHARE)
            .count();
        if good_sides < MIN_SIDES {
            return Err(Rejection::TooFewSides { sides: good_sides });
        }

        let wide_on = wide_samples.iter().filter(|c| on_colour(c)).count();
        let wide_share = wide_on as f64 / wide_samples.len().max(1) as f64;
        if wide_samples.len() < MIN_WIDE_SAMPLES || wide_share < MIN_WIDE_SHARE {
            return Err(Rejection::WideRingDiffers {
                samples: wide_samples.len(),
                uniform_share: wide_share,
            });
        }
        Ok(fill)
    }
}

fn is_on_colour(sample: &[u8; 3], colour: [u8; 3], tolerance: u8) -> bool {
    sample
        .iter()
        .zip(colour)
        .all(|(&channel, median)| channel.abs_diff(median) <= tolerance)
}

/// The shared test: enough samples, nearly all of one colour, and nothing
/// off-colour touching the letters.
fn judge(evidence: Evidence, samples: &[Sample]) -> Result<BlockColour, Rejection> {
    if samples.len() < MIN_SAMPLES {
        return Err(Rejection::TooFewSamples {
            evidence,
            samples: samples.len(),
        });
    }
    let colour = median_colour(samples.iter().map(|s| &s.colour));
    let tolerance = evidence.tolerance();
    let uniform_share = samples
        .iter()
        .filter(|s| is_on_colour(&s.colour, colour, tolerance))
        .count() as f64
        / samples.len() as f64;
    if uniform_share < MIN_UNIFORM_SHARE {
        return Err(Rejection::NotUniform {
            evidence,
            uniform_share,
        });
    }
    let contact_samples = samples.iter().filter(|s| s.near).count();
    let off_pixels = samples
        .iter()
        .filter(|s| s.near && !is_on_colour(&s.colour, colour, tolerance))
        .count();
    let allowed = match evidence {
        Evidence::Bubble(_) => {
            MAX_CONTACT_OFF_PX.max((contact_samples as f64 * MAX_CONTACT_OFF_SHARE) as usize)
        }
        Evidence::Local => 0,
    };
    if off_pixels > allowed {
        return Err(Rejection::ContactNotUniform {
            evidence,
            off_pixels,
        });
    }
    Ok(BlockColour {
        evidence,
        colour,
        samples: samples.len(),
        uniform_share,
    })
}

fn binarize(mask: &GrayImage) -> GrayImage {
    GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        Luma([if mask.get_pixel(x, y).0[0] > 0 {
            255
        } else {
            0
        }])
    })
}

fn median_colour<'a>(samples: impl Iterator<Item = &'a [u8; 3]>) -> [u8; 3] {
    let mut histograms = [[0usize; 256]; 3];
    let mut len = 0usize;
    for sample in samples {
        len += 1;
        for (histogram, &channel) in histograms.iter_mut().zip(sample) {
            histogram[usize::from(channel)] += 1;
        }
    }
    let half = len.div_ceil(2);
    histograms.map(|histogram| {
        let mut seen = 0;
        histogram
            .iter()
            .position(|count| {
                seen += count;
                seen >= half
            })
            .unwrap_or(0) as u8
    })
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    const W: u32 = 120;
    const H: u32 = 80;

    /// A crop with one bubble covering everything, a text block in the
    /// middle (generation region) and three glyph bars inside it (paste).
    struct Scene {
        image: RgbaImage,
        generation: GrayImage,
        paste: GrayImage,
        bubbles: GrayImage,
    }

    fn scene(background: impl Fn(u32, u32) -> [u8; 3], ink: [u8; 3]) -> Scene {
        let in_block = |x: u32, y: u32| (20..100).contains(&x) && (15..65).contains(&y);
        let in_glyph = |x: u32, y: u32| {
            in_block(x, y)
                && (25..60).contains(&y)
                && [30, 55, 80].iter().any(|&g| (g..g + 8).contains(&x))
        };
        let image = RgbaImage::from_fn(W, H, |x, y| {
            let [r, g, b] = if in_glyph(x, y) {
                ink
            } else {
                background(x, y)
            };
            Rgba([r, g, b, 255])
        });
        let generation =
            GrayImage::from_fn(W, H, |x, y| Luma([if in_block(x, y) { 255 } else { 0 }]));
        // The engine's paste mask is the glyphs grown by a couple of pixels.
        let paste = dilate(
            &GrayImage::from_fn(W, H, |x, y| Luma([if in_glyph(x, y) { 255 } else { 0 }])),
            Norm::LInf,
            2,
        );
        let bubbles = GrayImage::from_pixel(W, H, Luma([1]));
        Scene {
            image,
            generation,
            paste,
            bubbles,
        }
    }

    fn outside_bubbles(mut s: Scene) -> Scene {
        s.bubbles = GrayImage::new(W, H);
        s
    }

    fn classify(scene: &Scene) -> CropFill {
        classify_crop(
            &scene.image,
            &scene.generation,
            &scene.paste,
            &scene.bubbles,
        )
    }

    fn flat_colours(fill: &CropFill) -> Vec<[u8; 3]> {
        assert!(
            fill.is_all_flat(),
            "expected a flat fill, got {:?}",
            fill.rejections()
        );
        fill.flat_blocks().iter().map(|c| c.colour).collect()
    }

    fn only_rejection(fill: &CropFill) -> Rejection {
        assert!(fill.flat_blocks().is_empty(), "{:?}", fill.flat_blocks());
        match fill.rejections()[..] {
            [rejection] => rejection,
            ref other => panic!("expected one rejection, got {other:?}"),
        }
    }

    #[test]
    fn plain_bubbles_of_any_colour_fill_flat() {
        for (background, ink) in [
            ([255, 255, 255], [0, 0, 0]),
            ([12, 12, 14], [250, 250, 250]),
            ([236, 196, 210], [60, 20, 30]),
            ([120, 170, 230], [255, 255, 255]),
        ] {
            let fill = classify(&scene(|_, _| background, ink));
            assert_eq!(flat_colours(&fill), vec![background]);
            assert_eq!(fill.flat_blocks()[0].evidence, Evidence::Bubble(1));
        }
    }

    #[test]
    fn scan_noise_within_tolerance_still_fills_flat() {
        let noisy = |x: u32, y: u32| {
            let n = ((x * 7 + y * 13) % 9) as u8;
            [246 + n, 246 + n, 246 + n]
        };
        let colours = flat_colours(&classify(&scene(noisy, [0, 0, 0])));
        assert_eq!(colours.len(), 1);
    }

    #[test]
    fn gradient_background_goes_to_flux() {
        let gradient = |_: u32, y: u32| {
            let v = (230 - y * 2) as u8;
            [v, v, v]
        };
        assert!(matches!(
            only_rejection(&classify(&scene(gradient, [0, 0, 0]))),
            Rejection::NotUniform { .. }
        ));
    }

    fn screentone(x: u32, y: u32) -> [u8; 3] {
        if (x / 2 + y / 2).is_multiple_of(2) {
            [40, 40, 40]
        } else {
            [230, 230, 230]
        }
    }

    #[test]
    fn screentone_goes_to_flux() {
        assert!(matches!(
            only_rejection(&classify(&scene(screentone, [0, 0, 0]))),
            Rejection::NotUniform { .. }
        ));
    }

    #[test]
    fn outlined_text_goes_to_flux() {
        // White halo around black letters that the paste mask doesn't cover.
        let mut s = scene(|_, _| [30, 30, 30], [0, 0, 0]);
        let halo = dilate(&s.paste, Norm::LInf, 5);
        for (x, y, px) in s.image.enumerate_pixels_mut() {
            if halo.get_pixel(x, y).0[0] > 0 && s.paste.get_pixel(x, y).0[0] == 0 {
                *px = Rgba([255, 255, 255, 255]);
            }
        }
        assert!(matches!(
            only_rejection(&classify(&s)),
            Rejection::NotUniform { .. }
        ));
        assert!(matches!(
            only_rejection(&classify(&outside_bubbles(s))),
            Rejection::NotUniform {
                evidence: Evidence::Local,
                ..
            }
        ));
    }

    /// A short art line hidden under one letter: only 4 px stubs show on
    /// either side, far too few to fail the uniform share.
    fn line_under_a_letter(s: &mut Scene) {
        for (x, y, px) in s.image.enumerate_pixels_mut() {
            if (40..42).contains(&y) && (22..46).contains(&x) && s.paste.get_pixel(x, y).0[0] == 0 {
                *px = Rgba([90, 60, 60, 255]);
            }
        }
    }

    #[test]
    fn line_running_under_the_text_goes_to_flux() {
        let mut s = scene(|_, _| [240, 240, 240], [0, 0, 0]);
        line_under_a_letter(&mut s);
        assert!(matches!(
            only_rejection(&classify(&s)),
            Rejection::ContactNotUniform { .. }
        ));
        assert!(matches!(
            only_rejection(&classify(&outside_bubbles(s))),
            Rejection::ContactNotUniform {
                evidence: Evidence::Local,
                ..
            }
        ));
    }

    #[test]
    fn text_on_a_flat_panel_outside_bubbles_fills_flat() {
        for background in [[0, 0, 0], [255, 255, 255], [33, 34, 38]] {
            let fill = classify(&outside_bubbles(scene(|_, _| background, [250, 250, 250])));
            assert_eq!(flat_colours(&fill), vec![background]);
            assert_eq!(fill.flat_blocks()[0].evidence, Evidence::Local);
        }
    }

    #[test]
    fn faint_shading_fills_flat_only_in_a_bubble() {
        // A soft grey band, 10 levels under white, across half the text.
        let shade = |_: u32, y: u32| if y < 40 { [255; 3] } else { [245; 3] };
        assert!(classify(&scene(shade, [0, 0, 0])).is_all_flat());
        assert!(matches!(
            only_rejection(&classify(&outside_bubbles(scene(shade, [0, 0, 0])))),
            Rejection::NotUniform {
                evidence: Evidence::Local,
                ..
            }
        ));
    }

    #[test]
    fn text_over_artwork_outside_bubbles_goes_to_flux() {
        assert!(matches!(
            only_rejection(&classify(&outside_bubbles(scene(screentone, [0, 0, 0])))),
            Rejection::NotUniform {
                evidence: Evidence::Local,
                ..
            }
        ));
    }

    #[test]
    fn flat_patch_inside_artwork_outside_bubbles_goes_to_flux() {
        // Flat just around the letters, screentone from 10 px out.
        let mut s = outside_bubbles(scene(|_, _| [255, 255, 255], [0, 0, 0]));
        let patch = dilate(&s.paste, Norm::LInf, 10);
        for (x, y, px) in s.image.enumerate_pixels_mut() {
            if patch.get_pixel(x, y).0[0] == 0 {
                let [r, g, b] = screentone(x, y);
                *px = Rgba([r, g, b, 255]);
            }
        }
        assert!(matches!(
            only_rejection(&classify(&s)),
            Rejection::WideRingDiffers { .. }
        ));
    }

    #[test]
    fn text_against_the_crop_edge_outside_bubbles_goes_to_flux() {
        // A letter flush with the left, top and bottom edges: background
        // shows on one side only.
        let letter = |x: u32, _: u32| x < 12;
        let image = RgbaImage::from_fn(W, H, |x, y| {
            if letter(x, y) {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        let paste = GrayImage::from_fn(W, H, |x, y| Luma([if letter(x, y) { 255 } else { 0 }]));
        let generation = GrayImage::from_fn(W, H, |x, _| Luma([if x < 20 { 255 } else { 0 }]));
        let fill = classify_crop(&image, &generation, &paste, &GrayImage::new(W, H));
        assert!(matches!(
            only_rejection(&fill),
            Rejection::TooFewSides { sides: 2 }
        ));
    }

    #[test]
    fn block_erased_whole_goes_to_flux() {
        // Undetected-glyph fallback: the whole block is pasted, leaving only
        // a thin sliver of background inside the bubble.
        let mut s = scene(|_, _| [255, 255, 255], [0, 0, 0]);
        s.paste = s.generation.clone();
        s.bubbles = GrayImage::from_fn(W, H, |x, y| {
            Luma([u8::from((17..103).contains(&x) && (12..68).contains(&y))])
        });
        assert!(matches!(
            only_rejection(&classify(&s)),
            Rejection::TooFewSamples { .. }
        ));
    }

    #[test]
    fn each_bubble_keeps_its_own_colour() {
        let mut s = scene(
            |x, _| {
                if x < 60 {
                    [255, 255, 255]
                } else {
                    [250, 220, 180]
                }
            },
            [0, 0, 0],
        );
        s.bubbles = GrayImage::from_fn(W, H, |x, _| Luma([if x < 60 { 1 } else { 2 }]));
        let fill = classify(&s);
        assert_eq!(flat_colours(&fill), vec![[255, 255, 255], [250, 220, 180]]);
        let crop = fill.flat_crop(&s.image);
        assert_eq!(crop.get_pixel(30, 30).0, [255, 255, 255]);
        assert_eq!(crop.get_pixel(85, 30).0, [250, 220, 180]);
    }

    #[test]
    fn one_uncertain_block_sends_the_whole_crop_to_flux() {
        // Left block on a flat bubble, right block over screentone outside
        // bubbles.
        let background = |x: u32, y: u32| {
            if x < 60 {
                [240, 240, 240]
            } else {
                screentone(x, y)
            }
        };
        let mut s = scene(background, [0, 0, 0]);
        s.generation = GrayImage::from_fn(W, H, |x, y| {
            let block = (20..48).contains(&x) || (72..100).contains(&x);
            Luma([if block && (15..65).contains(&y) {
                255
            } else {
                0
            }])
        });
        s.paste = GrayImage::from_fn(W, H, |x, y| {
            let block = s.generation.get_pixel(x, y).0[0] > 0;
            Luma([if block && s.paste.get_pixel(x, y).0[0] > 0 {
                255
            } else {
                0
            }])
        });
        s.bubbles = GrayImage::from_fn(W, H, |x, _| Luma([u8::from(x < 60)]));
        let fill = classify(&s);
        assert!(!fill.is_all_flat());
        assert_eq!(fill.flat_blocks().len(), 1);
        assert!(matches!(
            fill.rejections()[..],
            [Rejection::NotUniform {
                evidence: Evidence::Local,
                ..
            }]
        ));
    }

    #[test]
    fn a_speck_next_to_the_letters_is_tolerated_only_in_a_bubble() {
        let mut s = scene(|_, _| [255, 255, 255], [0, 0, 0]);
        // 2x2 px, 4 px right of the first letter's paste mask.
        for (x, y) in [(44, 40), (45, 40), (44, 41), (45, 41)] {
            s.image.put_pixel(x, y, Rgba([120, 120, 120, 255]));
        }
        assert!(classify(&s).is_all_flat());
        assert!(matches!(
            only_rejection(&classify(&outside_bubbles(s))),
            Rejection::ContactNotUniform {
                evidence: Evidence::Local,
                off_pixels: 4,
            }
        ));
    }

    #[test]
    fn nothing_to_paste_needs_no_flux() {
        let mut s = scene(|_, _| [255, 255, 255], [0, 0, 0]);
        s.paste = GrayImage::new(W, H);
        assert!(classify(&s).is_all_flat());
    }

    /// Classify externally generated cases (`<id>_img.png`, `<id>_gen.png`,
    /// `<id>_paste.png`) with no bubbles (the rule for text outside bubbles),
    /// writing `results.jsonl` beside them. Used to measure false positives
    /// against ground truth: synthetic text over real artwork.
    #[test]
    #[ignore = "needs KOHARU_FLAT_FILL_CASES pointing at generated cases"]
    fn classify_generated_cases() {
        use std::io::Write;

        let dir = std::path::PathBuf::from(
            std::env::var("KOHARU_FLAT_FILL_CASES").expect("KOHARU_FLAT_FILL_CASES"),
        );
        let mut ids: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                name.strip_suffix("_img.png").map(str::to_owned)
            })
            .collect();
        ids.sort();
        let mut out = std::fs::File::create(dir.join("results.jsonl")).unwrap();
        for id in ids {
            let open = |suffix: &str| image::open(dir.join(format!("{id}_{suffix}.png"))).unwrap();
            let image = open("img").to_rgba8();
            let generation = open("gen").to_luma8();
            let paste = open("paste").to_luma8();
            let bubbles = GrayImage::new(image.width(), image.height());
            let fill = classify_crop(&image, &generation, &paste, &bubbles);
            let row = if fill.is_all_flat() {
                match fill.flat_blocks().first() {
                    Some(block) => serde_json::json!({
                        "id": id, "decision": "flat", "colour": block.colour,
                        "uniform_share": block.uniform_share,
                    }),
                    None => serde_json::json!({ "id": id, "decision": "empty" }),
                }
            } else {
                serde_json::json!({
                    "id": id, "decision": "flux",
                    "reason": format!("{:?}", fill.rejections()),
                })
            };
            writeln!(out, "{row}").unwrap();
        }
    }
}
