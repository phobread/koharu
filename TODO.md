# To do

## Done recently

- **Bubble-shaped lettering (2026-09-30).** Unlocked boxes inside a speech
  bubble lay their text out in the bubble's shape (ported from upstream
  Koharu), with joined bubbles split at their seams. Used only when it gives
  bigger text than the box; on hand-resized bubble lines it lands closer to
  the finished size (10.9 0.87x -> 1.04x, BadEnd 0.87x -> 0.99x). Some black
  caption boxes are missed by the bubble detector and keep the box layout
  (possible fallback: find the box's flat-colour patch on the cleaned page).
- **UI batch (2026-09-29, 374fdc58):** project name and back arrow in the menu
  bar, box selection and Delete/Backspace deletion as one undo step, process
  only what's missing (current, selected or unfinished pages) with step ticks
  and "Page X of Y" progress, decluttered toolbar and tabs, one Export entry
  with an export folder in Settings, mouse back/forward, project covers on the
  home page, Free up space for unused project images, open/close fade.
- **Box editing (2026-09-30):** the side panel shows the border width the
  renderer really uses (85a98866); Tab / Shift+Tab steps through a page's text
  boxes (17541483); arrow keys move the caret in the box editor again
  (0c7b7348).
- **Exports keep the original file names** (d5ad58c0).
- **OCR of thick-outlined lettering (2026-09-30, 0874959f).** Coloured or dark
  text with a thick white outline over artwork is redrawn black-on-white before
  both OCR readers. Hangul errors: Dmon 22→9, Domina 32→7, BadEnd 106→65,
  10.9 34→23, held-out BadEnd cont 14→8.
- **Faster Flux2 cleanup of large areas (2026-09-30).** Crops above 0.3 MP
  are generated at half their width and height and scaled back up: the Flux2
  step on Dmon + 10.9 + BadEnd went 761 s → 418 s (−45 %). Blind A/B: 6 better,
  6 worse, 8 same (half area: 7/10/13). Settings → Engines → "Faster cleanup
  of large areas", on by default.
- **Text hearts (2026-09-29):** flat fill outside bubbles (1fb6c709) and the
  "Repair with LaMa" brush switch (9b5f8a21).

## OCR

- **Black-on-black outlined text** still has trapped background pockets that
  can't be told from ink by colour (in one font 다 reads as 타 even in the
  original image). Idea (Astra): read ambiguous lines with and without the
  pockets and accept only agreeing repairs. Modest gain, not started.
- **Context correction (idea).** The translation model sees the whole sentence
  and could flag obvious OCR slips (온몸을 타해 → 다해). Only if leftover OCR
  errors keep costing proofreading time.
- Hand-lettered moans and SFX remain out of scope.

## Lettering

- **Text over artwork is too small.** The owner enlarged 24 of 25 such lines
  (fork auto ≈ half the final size). Let free-standing text grow past the
  detected box.
- **Other upstream layout extras (not evaluated).** Upstream's `layout.rs` is
  3.5k lines vs our 1.4k, but ~2.1k of it is tests; its real code is ~400
  lines bigger. Besides the balloon mode it adds: justify alignment,
  line-height and letter/word spacing controls, balanced line breaks with
  "don't break after the/to/of" penalties, hyphenation only as a last resort,
  and CJK punctuation/emphasis layout (not needed for English output). Worth a
  look once the bubble port has settled.

## Inpainting

- **LaMa before Flux2 (idea).** Clean the text area with LaMa before Flux2
  generates, so Flux2 never sees a typed heart and can't redraw it. Changes
  every crop; needs its own A/B. Only if the manual "Repair with LaMa" stroke
  gets tedious.
