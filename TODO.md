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

- **Text hearts come back.** Flux2 redraws the ♡/♥ typed at the end of
  Korean lines even when the mask covers them (926 pages 5, 13, 14, 15, 23:
  about half the heart's pixels return). The repair brush can't remove them
  either: a stroke shows Flux2 the original page, so it redraws the heart
  (8 of 8 strokes on 4 hearts, 2026-09-28). Today's workaround is the plain
  Brush in the bubble's colour, which only works on flat bubbles. Hearts drawn
  as part of the artwork sit outside the text boxes and must stay. Ideas:
  flat fill for bubbles whose only off-colour pixels are small decorations
  away from the text (most BadEnd bubbles are black with pink hearts; also
  faster), a repair stroke that hides the original from Flux2 under the
  stroke, or regenerating a crop once when a masked symbol survives.
- **Crop-downscale A/B (perf idea #6).** Generate Flux2 crops above ~0.3 MP at
  half resolution and upscale the fill. Planned as a final blind A/B, since it
  risks damaging screentone and fine lines.
