# To do

## UI

- **General UI refresh.** The interface has too many buttons, dials and
  dropdown menus; simplify it and ease the everyday translate-and-touch-up
  workflow.
- **Show the project name inside a project.** Once a project is open, its
  name is not visible anywhere.
- **One-click way out of a project.** Leaving a project today takes Ctrl+W or
  File → Close project. Add a visible close (X) or back button, ideally next
  to the project name above.
- **Quicker text-box deletion.** Today a box is removed one at a time through
  its right-click menu or the Delete button in the text-blocks panel. That is
  tedious when clearing onomatopoeia boxes. Add a Delete/Backspace shortcut on
  the canvas that removes every selected box (multi-selection already exists)
  as one undo step, plus a one-click delete control. The Delete key already
  deletes pages in the navigator, so the shortcut must be scoped to whichever
  panel has focus and must not fire while typing in a text box.
- **Process only what's missing.** "Process all pages" reruns every step on
  every page, so finishing the remaining pages redoes OCR and overwrites
  finished work. Make "Process unprocessed pages" the primary action, skipping
  steps a page already has (existing text boxes, an inpainted layer). Keep
  "Process all pages" as the explicit redo. Also allow processing a hand-picked
  set of pages. The navigator already supports multi-select (used by batch
  delete), and `POST /pipelines` already accepts a `pages` list.
- **Clear batch progress.** During a multi-page run (e.g. "Custom pipeline →
  all pages") the progress card should say which page it is on and what it is
  doing, e.g. "Page 4 / 12 · OCR", with a real percentage. The bug part was
  fixed on 2026-09-28: progress ticks now reach the UI during GPU steps (they
  used to wait for the whole run), and all-pages runs say "Processing all
  images". Left for the overhaul: wording and layout, and if runs keep going
  in chunks of pages, showing the stage and page range ("OCR · pages 1–8 of
  23") instead of a page number that cycles.

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
  Same family: outlined lettering. The segmenter masks the black letters but
  not their thick white outline, and Flux2 keeps the outline as if it were
  bubble, leaving a bright text-shaped blob. Example: 926 page 2 block 5
  ("에엥—" on a see-through bubble). Since 2026-09-28 one repair stroke over
  the word fixes it; the automatic mask should include the outline.
- **Rare Flux2 out-of-memory on heavy pages.** Detection/OCR and Flux2 now
  take turns on the GPU, and Flux2 alone peaks at 5.6-5.7 GB of the 6 GB card.
  In test runs on copies of 926 (2026-09-28), page 2 (the heaviest page) failed
  its Flux2 step with `CUDA_ERROR_OUT_OF_MEMORY` 2 times in 16, and once took
  217 s instead of ~65 s (memory spilled to system RAM: GPU busy at 26 W
  instead of 40-60 W); no other page did either. Both failures were on the
  review branch, which ran it 14 of the 16 times; nothing there changes GPU
  memory, and both builds peak the same. A failed page is left uncleaned with
  a warning. Cause (measured, Flux2 only): all three failures hit page 2's
  first crop, a 1360x640 strip where five neighbouring top-row bubbles merge
  into one crop (3400 tokens). Every crop of ~3400+ tokens (also 003's
  1120x816, 009's tall strip) fills the card to ~5.7 GB and runs starved:
  ~38 W instead of ~62 W, 26-53 s instead of 3-8 s. Page 2's strip is the
  first big crop after Flux2 loads, which is likely why it is the one that
  tips over. Done 2026-09-28: merged crops are capped at 640k px (~2500
  tokens), and neighbours that would exceed it run as separate crops that
  leave each other's mask components alone. Flux2-only on 926 pages
  002/003/006/008/009: 287 s -> 183 s, GPU back at ~58 W, no seams, fill
  quality equivalent (differences only inside text areas). Full runs
  (9 pages, detect+OCR+Flux2): 408-415 s -> 330-337 s, no out-of-memory. Still open:
  single bubbles over the cap (926 page 8, 816x928) and a retry after
  out-of-memory.
- **Crop-downscale A/B (perf idea #6).** Generate Flux2 crops above ~0.3 MP at
  half resolution and upscale the fill. Planned as a final blind A/B, since it
  risks damaging screentone and fine lines.
