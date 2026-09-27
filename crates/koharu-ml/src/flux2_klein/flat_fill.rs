//! Flat-background shortcut for Flux2 inpainting.
//!
//! Text on a plain speech bubble of any colour doesn't need a diffusion model:
//! the pixels under the letters are the bubble's colour. For each planned
//! crop, [`classify_crop`] decides conservatively whether every bubble the
//! crop repaints is flat. If so the crop is filled with each bubble's own
//! colour instead of running Flux2; anything uncertain goes to Flux2.
//!
//! Safeguards against filling artwork:
//! - Only pixels in the paste mask change, the same pixels Flux2 would
//!   repaint, so a fill can never spread past the lettering.
//! - Every pasted pixel must lie inside a detected speech bubble.
//! - The evidence is the bubble's own background visible between and around
//!   the letters (the generation region and a thin ring outside the paste
//!   mask, minus a guard band hugging the glyphs). Nearly all of it must sit
//!   within a tight tolerance of one colour, so gradients, screentone,
//!   outlines and shading all fail.
//! - Anything off-colour right next to the letters means Flux2: a line or
//!   shape that disappears under the text shows up there, while stray specks
//!   elsewhere in the bubble are tolerated.
//! - Too little visible background (for example a block erased whole because
//!   no glyph pixels were segmented) means Flux2.

use image::{GrayImage, Luma, Rgb, RgbImage, RgbaImage};
use imageproc::{distance_transform::Norm, morphology::dilate};

/// Pixels next to the glyphs are skipped: anti-aliasing and JPEG ringing
/// there say nothing about the background.
const GUARD_PX: u8 = 2;
/// Width of the ring sampled outside the paste mask.
const RING_PX: u8 = 8;
/// Minimum background samples per bubble before trusting it.
const MIN_SAMPLES: usize = 400;
/// A sample is "on the colour" when every channel is within this of the
/// bubble's median.
const COLOUR_TOLERANCE: u8 = 12;
/// Share of samples that must be on the colour.
const MIN_UNIFORM_SHARE: f64 = 0.99;
/// Band just outside the guard band where anything off-colour suggests
/// artwork continuing under the letters.
const CONTACT_PX: u8 = GUARD_PX + 4;
/// Off-colour pixels tolerated in the contact band: this many, or this share
/// of the band, whichever is larger.
const MAX_CONTACT_OFF_PX: usize = 6;
const MAX_CONTACT_OFF_SHARE: f64 = 0.003;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum CropFill {
    /// Fill each bubble's pasted pixels with its colour.
    Flat(Vec<BubbleColour>),
    Flux(Rejection),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct BubbleColour {
    pub bubble: u8,
    pub colour: [u8; 3],
    pub samples: usize,
    pub uniform_share: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Rejection {
    /// A pasted pixel lies outside every detected bubble.
    OutsideBubble,
    TooFewSamples {
        bubble: u8,
        samples: usize,
    },
    NotUniform {
        bubble: u8,
        uniform_share: f64,
    },
    /// Something off-colour touches the letters and may continue under them.
    ContactNotUniform {
        bubble: u8,
        off_pixels: usize,
    },
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
    let mut pasted_bubbles = [false; 256];
    for (paste_px, bubble) in paste.pixels().zip(bubbles.pixels()) {
        if paste_px.0[0] == 0 {
            continue;
        }
        if bubble.0[0] == 0 {
            return CropFill::Flux(Rejection::OutsideBubble);
        }
        pasted_bubbles[usize::from(bubble.0[0])] = true;
    }

    let paste_binary = binarize(paste);
    let guard = dilate(&paste_binary, Norm::LInf, GUARD_PX);
    let contact = dilate(&paste_binary, Norm::LInf, CONTACT_PX);
    let ring = dilate(&paste_binary, Norm::LInf, RING_PX);

    let mut colours = Vec::new();
    for bubble in (1..=255u8).filter(|&id| pasted_bubbles[usize::from(id)]) {
        // Background pixels, each flagged when it sits in the contact band.
        let samples: Vec<([u8; 3], bool)> = image
            .enumerate_pixels()
            .filter(|&(x, y, _)| {
                bubbles.get_pixel(x, y).0[0] == bubble
                    && guard.get_pixel(x, y).0[0] == 0
                    && (generation.get_pixel(x, y).0[0] > 0 || ring.get_pixel(x, y).0[0] > 0)
            })
            .map(|(x, y, px)| {
                (
                    [px.0[0], px.0[1], px.0[2]],
                    contact.get_pixel(x, y).0[0] > 0,
                )
            })
            .collect();
        if samples.len() < MIN_SAMPLES {
            return CropFill::Flux(Rejection::TooFewSamples {
                bubble,
                samples: samples.len(),
            });
        }
        let colour = median_colour(samples.iter().map(|(c, _)| c));
        let on_colour = |sample: &[u8; 3]| {
            sample
                .iter()
                .zip(colour)
                .all(|(&channel, median)| channel.abs_diff(median) <= COLOUR_TOLERANCE)
        };
        let uniform_share =
            samples.iter().filter(|(c, _)| on_colour(c)).count() as f64 / samples.len() as f64;
        if uniform_share < MIN_UNIFORM_SHARE {
            return CropFill::Flux(Rejection::NotUniform {
                bubble,
                uniform_share,
            });
        }
        let contact_samples = samples.iter().filter(|(_, near)| *near).count();
        let off_pixels = samples
            .iter()
            .filter(|(c, near)| *near && !on_colour(c))
            .count();
        let allowed =
            MAX_CONTACT_OFF_PX.max((contact_samples as f64 * MAX_CONTACT_OFF_SHARE) as usize);
        if off_pixels > allowed {
            return CropFill::Flux(Rejection::ContactNotUniform { bubble, off_pixels });
        }
        colours.push(BubbleColour {
            bubble,
            colour,
            samples: samples.len(),
            uniform_share,
        });
    }
    CropFill::Flat(colours)
}

/// The crop Flux2 would have produced, painted with each bubble's colour.
/// Pixels outside the paste mask are never composited, so their value is
/// irrelevant.
pub(super) fn flat_crop(colours: &[BubbleColour], bubbles: &GrayImage) -> RgbImage {
    let fallback = colours.first().map_or([255; 3], |c| c.colour);
    let mut lookup = [fallback; 256];
    for c in colours {
        lookup[usize::from(c.bubble)] = c.colour;
    }
    RgbImage::from_fn(bubbles.width(), bubbles.height(), |x, y| {
        Rgb(lookup[usize::from(bubbles.get_pixel(x, y).0[0])])
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

    fn classify(scene: &Scene) -> CropFill {
        classify_crop(
            &scene.image,
            &scene.generation,
            &scene.paste,
            &scene.bubbles,
        )
    }

    fn flat_colours(fill: CropFill) -> Vec<[u8; 3]> {
        match fill {
            CropFill::Flat(colours) => colours.iter().map(|c| c.colour).collect(),
            CropFill::Flux(rejection) => panic!("expected a flat fill, got {rejection:?}"),
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
            let colours = flat_colours(classify(&scene(|_, _| background, ink)));
            assert_eq!(colours, vec![background]);
        }
    }

    #[test]
    fn scan_noise_within_tolerance_still_fills_flat() {
        let noisy = |x: u32, y: u32| {
            let n = ((x * 7 + y * 13) % 9) as u8;
            [246 + n, 246 + n, 246 + n]
        };
        let colours = flat_colours(classify(&scene(noisy, [0, 0, 0])));
        assert_eq!(colours.len(), 1);
    }

    #[test]
    fn gradient_background_goes_to_flux() {
        let gradient = |_: u32, y: u32| {
            let v = (230 - y * 2) as u8;
            [v, v, v]
        };
        assert!(matches!(
            classify(&scene(gradient, [0, 0, 0])),
            CropFill::Flux(Rejection::NotUniform { .. })
        ));
    }

    #[test]
    fn screentone_goes_to_flux() {
        let tone = |x: u32, y: u32| {
            if (x / 2 + y / 2).is_multiple_of(2) {
                [40, 40, 40]
            } else {
                [230, 230, 230]
            }
        };
        assert!(matches!(
            classify(&scene(tone, [0, 0, 0])),
            CropFill::Flux(Rejection::NotUniform { .. })
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
            classify(&s),
            CropFill::Flux(Rejection::NotUniform { .. })
        ));
    }

    #[test]
    fn line_running_under_the_text_goes_to_flux() {
        // A short art line hidden under one letter: only 4 px stubs show on
        // either side, far too few to fail the uniform share.
        let mut s = scene(|_, _| [240, 240, 240], [0, 0, 0]);
        for (x, y, px) in s.image.enumerate_pixels_mut() {
            if (40..42).contains(&y) && (22..46).contains(&x) && s.paste.get_pixel(x, y).0[0] == 0 {
                *px = Rgba([90, 60, 60, 255]);
            }
        }
        assert!(matches!(
            classify(&s),
            CropFill::Flux(Rejection::ContactNotUniform { .. })
        ));
    }

    #[test]
    fn text_outside_any_bubble_goes_to_flux() {
        let mut s = scene(|_, _| [255, 255, 255], [0, 0, 0]);
        s.bubbles = GrayImage::from_fn(W, H, |x, _| Luma([u8::from(x < 60)]));
        assert_eq!(classify(&s), CropFill::Flux(Rejection::OutsideBubble));
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
            classify(&s),
            CropFill::Flux(Rejection::TooFewSamples { .. })
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
        assert_eq!(
            flat_colours(fill.clone()),
            vec![[255, 255, 255], [250, 220, 180]]
        );
        let CropFill::Flat(colours) = fill else {
            unreachable!()
        };
        let crop = flat_crop(&colours, &s.bubbles);
        assert_eq!(crop.get_pixel(10, 10).0, [255, 255, 255]);
        assert_eq!(crop.get_pixel(110, 10).0, [250, 220, 180]);
    }

    /// Classify externally generated cases (`<id>_img.png`, `<id>_gen.png`,
    /// `<id>_paste.png`) with the whole crop treated as one bubble, writing
    /// `results.jsonl` beside them. Used to measure false positives against
    /// ground truth: synthetic text over real artwork.
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
            let bubbles = GrayImage::from_pixel(image.width(), image.height(), Luma([1]));
            let row = match classify_crop(&image, &generation, &paste, &bubbles) {
                CropFill::Flat(colours) if colours.is_empty() => {
                    serde_json::json!({ "id": id, "decision": "empty" })
                }
                CropFill::Flat(colours) => serde_json::json!({
                    "id": id, "decision": "flat", "colour": colours[0].colour,
                    "uniform_share": colours[0].uniform_share,
                }),
                CropFill::Flux(reason) => serde_json::json!({
                    "id": id, "decision": "flux", "reason": format!("{reason:?}"),
                }),
            };
            writeln!(out, "{row}").unwrap();
        }
    }
}
