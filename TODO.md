# To do

## UI

- **General UI refresh.** The interface has too many buttons, dials and
  dropdown menus; simplify it and ease the everyday translate-and-touch-up
  workflow.

## Inpainting

- **Finish half-removed text in plain bubbles.** Inside a detected text box
  that sits in a plain, one-colour bubble (the flat-fill bubble-colour check in
  `crates/koharu-ml/src/flux2_klein/flat_fill.rs`), treat every pixel that
  stands out from the bubble colour as text, so partly segmented lettering is
  fully erased. Example: 926 page 13, where the segmenter found the dots of
  "....!!!!" but missed the white "!!!!" on the black bubble. Today the
  undetected-block fallback in `crates/koharu-ml/src/inpainting/mask.rs` only
  fires when the segmenter found no glyph pixels at all. Validate like flat
  fill: held-out pages, and small drawings inside text boxes must survive.
  This won't help text over art that the detector never boxes (926 page 17
  "…그래..") or hand-lettered SFX; those stay manual touch-ups.
- **Crop-downscale A/B (perf idea #6).** Generate Flux2 crops above ~0.3 MP at
  half resolution and upscale the fill. Planned as a final blind A/B, since it
  risks damaging screentone and fine lines.
