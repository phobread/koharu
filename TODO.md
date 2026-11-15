# To do

Open work first, then known limits, ideas, upstream notes and what's done.
The long notes of earlier versions (every test run, number and dead end) are
in this file's git history up to commit 572aca1a. The test pictures those
notes name (under `C:\kt`, `D:\layout-trial`, `D:\flux3-trial`) are deleted.

## Standing rules (owner)

- Hard problems first, simple ones later; each separate task in its own
  agent.
- Overnight work is for Claude Fable 5.2 only. If its usage runs out or the
  session moves to another model, stop and leave the working tree as it is.
- LaMa only when "Repair with LaMa" is ticked: no automatic switch to it
  ("LaMa's inpainting is really meh").
- Coloured hearts are kept everywhere, dialogue bubbles included (the ♡ is
  dropped from the English there).
- No project-format change unless the owner is there to check that old
  projects still open (favourites use a side file for this reason).
- Upstream: port ideas by hand, never merge (see Upstream).

## Next up

- **Keep coloured hearts on black boxes (queued 2026-11-15, owner: "Yes,
  queue it"; the hearts change and the Ruination 0063 fix it waited for are
  committed and installed).** The rule that keeps coloured hearts out of the
  erase mask (`koharu-ml/src/inpainting/decorations.rs`,
  `MAX_SURROUNDING_DARK_SHARE`) skips dark boxes on purpose: with
  white-on-black lettering the dark pixels are the background, so it can't
  tell lettering from background. A pink heart on a black box is therefore
  erased like lettering when it is under the mask or in a text box with no
  mask. Idea: in a dark box with light lettering, keep coloured shapes that
  sit on the box's own colour, apart from the lettering; still erase the
  coloured parts of the lettering itself: a glow wrapped round the white
  letters (Ruination 0064's green glow) and colour inside outlined letters
  (ringed by the light outline). Test material: BadEnd3's black SFX boxes
  with pink hearts (pages 001, 005, 026, 035; 3000x4000). In the owner's
  project nothing cleans them yet (no box or mask touches them), so test
  with a mask painted over the lettering as the owner would, on a sandbox
  copy, plus a check that 0064's glow still goes and the Ruination,
  LOLExtras and Neeko's Help pages don't change.

- **A view button for the official release (owner, 2026-11-15: "a toggle
  between the official release and the raw korean would be nice").**
  Display only: show the release page in place of the raw, next to Original
  / Cleaned / Translated. The release image is already stored per page, so
  no format change. (BadEnd3 010's Korean SFX turned out to be the owner's
  own empty box + clean-up; undoing them brought the English back.)

## For the owner to check in the app

Checked 2026-11-15: favourites, open/export folders, font picker, 1px brush,
step dropdowns, "Hide the release's lettering here" and repair strokes all
fine. Still to watch:

- Hand strokes are now erased as drawn (+2px): a speck the tracing misses
  stays and needs its own stroke ("fine for now, needs more testing").
- Box text sizes after the fit fixes ("fine for now, needs more testing"):
  some hand-sized boxes fill much more of their bubble, and a fuller early
  line can leave a short last line (BadEnd3 031). Watch for any box
  dropping to 5px.
- The "Text over artwork" switch (Settings → Font defaults, near the
  bottom; only matters if turned on).
- Once installed: the text-box key (T), and "Export all pages" on a big
  project (folder dialog first, "Exporting N pages…" notice, then the files).

## Owner decisions open

- **On/off setting for the Flux2 rough start (owner, 2026-11-14).** On
  Ruination 0064 the owner's old from-noise page shows the shoulder through
  the bubble with a crisp edge; every run with the start blurs it (its edge
  lies wholly under the lettering). A switch would let a page be re-run from
  noise, with the bright-patch risk back. A LaMa start and 4 steps were
  tried and didn't help (see Tried and not kept).
- **Upstream layout model as an extra SFX / caption-frame finder.** Tested
  2026-11-14 (see Upstream): it found 46 of 49 hand-added SFX boxes, with
  masks that follow the letters. Best beside the current detectors, not
  instead of them; an ONNX export loaded through `ort` (as
  `koharu-ml/src/korean_ocr` does) is likely much less work than a candle
  port.
- **A "Release" view for stray release English**, only if stray lines keep
  turning up now that "Hide the release's lettering here" exists: a view
  next to Original / Cleaned / Translated where brushing means "not the
  release's here". Needs a saved mask per page, so a project-format change.

## Known bugs and limits (not being worked on)

### Inpainting

- **Flux2 has trouble over some boxes (owner, 2026-11-10).** No repro: the
  owner doesn't know which boxes or what sets it off. When it comes up,
  save the page and box (and the crop if possible) and note what went wrong.
- Repair strokes over cleaned lettering take 3-5 s (about 1.5 s before the
  redo fix), change only their own rectangle (a sliver beside it needs a
  second stroke) and leave a hard cut at the rectangle's edge between new
  and old fill (nothing visible in tests). Evidence: 2 pages, 8 spots, 2
  steps; not tried on lettering made of separate blobs with no box round it.
- Eraser side not looked at: erasing the mask over a heart to get it back
  re-cleans nearby lettering with the wide margin, and an eraser stroke that
  leaves a few mask pixels makes Flux2 redo that small rectangle (once it
  pasted a pale pink patch over a heart's white inside, LOLExtras 0084).
- A coloured word inside dark dialogue is kept like a heart. A brush stroke
  over a heart that is already in the letter mask paints no new pixels, so
  the heart stays: erase the mask over it first, then brush the whole heart
  (Flux2 redraws it from any part left showing) or use "Repair with LaMa".
- Flux2 leaves grey ghosts of some SFX (LOLExtras 0038 히잇, 0028 핫),
  with old and new builds alike.
- Dark bubbles on Ruination 0063: in 1 of 6 runs a faint teal line (the
  sky's lightning) runs through the left small bubble, and in 1 a small white
  curve shows at its top-left spike. Black bubbles on BadEnd3's 3000x4000
  pages (thicker strokes) don't qualify for the between-letters judgement.

### Lettering

- Vertical text can still have the hair-miss behind the 5px bug (only
  horizontal box text was fixed).
- Some black caption boxes are missed by the bubble detector and keep the
  box layout. Possible fallback: find the box's flat-colour patch on the
  cleaned page (the upstream layout model also finds these frames).
- Text over artwork growth (off by default), if turned on: a caption frame
  or sign the bubble detector misses counts as artwork, so its text fills
  the frame (Ruination 0063, LOLExtras 0037 sign); only other boxes and the
  page edge stop the growth, not a bubble edge; a neighbour beside a box
  caps its growth up and down too; a box with no font detection result
  never grows.

### Official release

- Tuned on BadEnd 1-20 and BadEnd3 only: try it on other chapters. Watch for
  the release's English peeking out next to your text and small Korean bits
  where Korean crosses a dark art line under your box (a repair stroke
  clears them). Telling a merged moan from your own line needs Detect's
  erase mask.
- Deleting a box on a cleaned release page takes about 2.5-3 s (the rule
  runs again).
- "Hide the release's lettering here": the Korean cleaned out under the
  hidden line often needs tweaking by hand (owner, 2026-11-15: "a bit
  iffy", probably the model); two lines closer than about 40px count as one; a sound effect of several big letters needs one click per
  letter; where the release lettered over the Korean, Korean fragments can
  remain; a later OCR run may try to read the empty box.

## Ideas (not started)

- **OCR, black-on-black outlined text:** trapped background pockets can't be
  told from ink by colour (in one font 다 reads as 타 even in the original).
  Astra's idea: read ambiguous lines with and without the pockets and accept
  only agreeing repairs. Modest gain.
- **OCR context correction:** the translation model sees the whole sentence
  and could flag obvious OCR slips (온몸을 타해 → 다해). Only if leftover OCR
  errors keep costing proofreading time.
- Hand-lettered moans and SFX stay out of scope for OCR.
- **Upstream layout extras (not evaluated):** justify alignment, line-height
  and letter/word spacing controls, balanced line breaks with "don't break
  after the/to/of" penalties, hyphenation only as a last resort. Upstream's
  `layout.rs` real code is about 400 lines bigger than ours.

## Upstream

Looked at on 2026-11-11 from a fresh clone (upstream now lives at
[koharu-rs/koharu](https://github.com/koharu-rs/koharu)); read from commit
messages, docs and module headers, none of its code run.

- **Where it is.** The fork split at 0.61.2 (2026-06-17). Upstream rebuilt
  everything on 2026-07-30 (libtorch instead of candle, stable-diffusion.cpp
  for clean-up, a WebGPU canvas, one layout model for detection) and released
  up to 0.83.5 (2026-09-22). Since then `main` has had five code commits and
  no release; a second rewrite is under way on `v3` (2026-10-21 onward) with
  no usable app yet. Neither a rewrite nor an overhaul for now: look again
  when `v3` ships a release.
- **Ideas worth taking** (on `v3` unless noted):
  - Measuring the original lettering (ink size, stroke width, colours,
    tilt: `koharu_layout_highres/measure.rs`, `koharu-leptonica`). Owner:
    translations often run larger than the original, so never a fixed size.
  - Clean-up masks stretched to whole letters (stroke ends the detector
    missed, the inside of outlined letters; commit b9accbfb). Plain image
    processing; aimed at leftover slivers and outlined text.
  - Flat fill that never paints over a balloon's outline (846c7f30); compare
    with our flat-fill rule.
  - Layout model with SFX and panel classes (`mayocream/koharu-layout-highres`,
    RF-DETR Seg 2XL, 186 MB, Apache-2.0; panels would also give a
    panel-by-panel reading order, `order.rs`). Tested 2026-11-14 outside the
    app (`rfdetr` 1.7.0 on the CPU, about 20 s a page) on 11 owner pages:
    - SFX: 46 of 49 hand-added boxes found, almost all labelled SFX (all 10
      traced SFX on LOLExtras 0028, all four on 0115, FWAAASH on 0026,
      게익…쾅 on Neeko's Help 0012). Hearts touching an SFX are often inside
      its mask.
    - Misses and false finds: dense moan clusters (Neeko's Help 0012 bottom
      left), some vertical moans only at 0.2-0.3, printed words on clothes
      and a halo called SFX.
    - Dialogue: 74 of 76 of the app's boxes (the misses: 0063's two small
      black bubbles).
    - Bubbles: finds the black caption frames the app misses (Ruination
      0063) but found no bubbles or panels at all on the dark, colourful
      BadEnd3 026, and misses wild-edged moan bubbles. Trained on
      black-and-white Japanese manga only.
  - "Rooms" for texts in joined balloons (`rooms.rs`): a newer version of
    the seam search we ported. Not urgent.
  - RORem clean-up (`main`; SDXL-based object removal through
    stable-diffusion.cpp). Never tried; likely tight on 6 GB.
- **Not useful here.** Japanese-only OCR models (Baberu, manga-ocr-ar) or
  ones already rejected (Hayai); local translation models, more providers,
  Linux/AMD, interface languages, sending the page image to the translation
  model (owner: redundant). Archive/PDF import and CBZ export only if wanted.

## Done

### Installed 2026-11-16 (f37208d4)

- **Text boxes on a key; the repair brush leaves them alone (2026-11-15,
  owner request).** T shows / hides the text boxes (Settings → Keybinds,
  "Show / hide text boxes"; the eye menu shows the key next to "Text
  boxes"). Picking the repair brush no longer turns the boxes back on; the
  box tool (M) still does. Saved shortcuts from older builds get T added
  unless T is already taken (then it starts unbound). `toggleTextBoxes` in
  `ui/lib/stores/preferencesStore.ts` (settings version 11),
  `useKeyboardShortcuts.ts`, `setMode` in `editorUiStore.ts`,
  `ViewControls.tsx`; 6 new tests.
- **Faster export of many pages (2026-11-15, owner report on BadEnd3:
  3000x4000 pages, 472 MB, slow to show the save dialog).** The server built
  the whole zip before the dialog opened, decoding and PNG-encoding every
  page one after another, then deflating the PNGs again. Now: pages are
  encoded side by side (up to 6 at once, `MAX_EXPORT_WORKERS` in
  `koharu-rpc/src/routes/projects.rs`), PNGs go into the zip uncompressed
  (`zip_files_to_bytes` in `koharu-app/src/archive.rs`), and the export no
  longer fills the decoded-image cache (`load_image_uncached` in
  `blobs.rs`). In the app, an export of several pages asks for the folder
  first and shows "Exporting N pages…" until it is done (`.khr`: "Exporting
  the project…"); a single page keeps the save dialog after the build
  (`exportCurrentProjectAs` in `ui/lib/io/pagesIo.ts`; sticky notices in
  `editorUiStore.ts`). 5 new UI tests, 1 Rust test. Timed on a copy of
  BadEnd3 (36 pages), fresh process each, server side of "Export all
  pages": 14.3 / 13.8 s → 1.3 / 1.2 s, the same 36 files with every PNG
  byte-identical. The folder-first dialog and the notice are unit-tested
  only (native dialog).

### 2026-11-11 to 2026-11-15 (installed 2026-11-14, the first item 2026-11-15)

- **Small black bubbles on Ruination 0063 come out black (2026-11-15,
  697bd8c3).** When a mask covers a dark bubble whole and fewer than 64
  bubble pixels are left around the lettering, the bubble is judged on the
  picture under the lettering instead (dark median, at least 45 % one colour
  and 10 % something else, a 5x5-erosion test that they look like space and
  strokes, and any leftover pixels must agree). The Flux2 start then fills
  from the bubble seen between the letters, only inside the outline, grown
  through art seen through it, so a see-through bubble keeps its art and
  loses only the strokes. `between_letters`, `bubble_inside` and
  `BubbleColour` in `koharu-ml/src/flux2_klein/rough_fill.rs`; the log line
  "Flux2 crop starts from the picture" lists `between_letters`. On 0063, 6
  runs: inside not black 10-94 % → 1-2 %; 0062 and 0064 pixel-identical.
  Checked on 54 pages: all 227 white bubbles fail the first test, the 8
  passes are dark bubbles. A white bubble whose bold black lettering is most
  of what is under its mask, with no white gap over 4px, would pass the
  stroke test; only the leftover-pixels test stops it (unit test).
- **Coloured hearts kept by clean-up (2026-11-14, c2a9c5a2).** Per text box,
  bright coloured shapes and their pale edge are left out of the erase mask
  when the box's lettering is dark (`keep_coloured_decorations`,
  `koharu-ml/src/inpainting/decorations.rs`, called from `glyph_erase_mask`
  in `koharu-app/src/pipeline/engines/support.rs`; Flux2, LaMa and AOT).
  Not when the lettering is coloured, the surroundings are coloured or
  mostly dark (white-on-black boxes: see Next up), or the shape is ringed by
  a dark outline (a letter's fill). Dark lettering crossing a heart is still
  erased. Repair strokes keep them too, except
  within 2px of pixels the stroke newly painted
  (`keep_coloured_decorations_except`; the mask before the stroke travels as
  `previous_segment_mask` in `PipelineRunOptions`, set by `put_mask` in
  `koharu-rpc/src/routes/pages.rs`). Damaged hearts on copies of the owner's
  pages: LOLExtras 0084 36→0, 0100 24→0, 0035 16→0, 0038 16→0, 0104 11→0,
  0028 6→1; Neeko's Help 0012 38→0; One Piece 0022 8→0. Per-box details:
  `RUST_LOG=info,koharu_ml::inpainting::decorations=debug`.
- **Flux2 keeps dark backgrounds (2026-11-13).** Lettering outside every
  bubble with plain surroundings (90 % of a 10px band within 24 of its
  median), and lettering in a dark bubble (no channel above 64; 75 % of the
  surroundings at least 12px inside the bubble map within 48), starts from
  a rough fill of its surroundings: after the first step that region is
  swapped for the fill plus noise at 0.9 and the model finishes from there.
  `plain_surroundings`, `rough_fill`, `bubble_colours` in `rough_fill.rs`;
  `RoughStart`, `ROUGH_START_SIGMA` in `koharu-ml/src/flux2_klein/mod.rs`.
  Ruination 0062's dark 4/5/6/7 panel right in 8 of 8 runs (was 7 of 8
  wrong); 0063's see-through bubble 8 of 8. Light bubbles unchanged.
  Testing note: Flux2's noise sequence is the same after every launch, so a
  result depends on what ran before it in the session; compare old and new
  only as same-order runs right after a fresh start.
- **Repair strokes no longer bring lettering back (2026-11-12).** A stroke
  whose rectangle holds already-cleaned pixels restores the whole connected
  piece of lettering it lies on (up to 256px away, `REDO_CONTEXT_PX`), Flux2
  cleans that piece as a full run would, and only the stroke's rectangle is
  copied back (`koharu-app/src/pipeline/engines/flux2_klein.rs`, helpers in
  `support.rs`). Before, the original came back under every mask pixel in
  the rectangle, so a clipped piece returned and Flux2 kept and recoloured
  it (LOLExtras 0026 FWWASH, Ruination 0045). Magenta bar over 0026's hair
  6 of 6 → 0 of 6; 0045's arc 1 of 3 → 0 of 2. Limits: see Known bugs.
- **Hand strokes erased as drawn (2026-11-12).** A brush stroke that adds to
  the mask now grows by 2px only, not by the box's amount (up to 8px for a
  hand-added box), so tracing an SFX no longer takes the heart beside it
  (LOLExtras 0028). `expand_hand_stroke_mask_for_inpainting` in
  `koharu-ml/src/inpainting/mask.rs`, chosen by `glyph_erase_mask`. Full
  runs and eraser strokes keep the old widening.
- **Box text fits properly (2026-11-12/13; "one word change drops a box to
  5px").** A miss at the floor size no longer ends the search
  (`auto_fit_size`, `koharu-app/src/renderer.rs`); the box line breaker
  packs lines by the same ink width the fit check measures (`LineInk`,
  `INK_PAD` in `koharu-renderer/src/layout.rs`); a box line never starts
  with a dash (`dash_follows`; U+2012-2015, U+2E3A, U+2E3B). On 400 boxes:
  346 the same, 37 bigger (mostly hand-sized boxes left far too small, e.g.
  LOLExtras 0105 18 → 40), 2 smaller; every box with a size set by hand is
  drawn at that size. The bubble-shaped path is unchanged.
- **Text over artwork may grow to the original size (2026-11-12; off by
  default since 2026-11-13, Settings → "Text over artwork").** A box outside
  bubbles whose automatic size is smaller than the lettering it replaces
  gets room around its centre up to the original size, stopping at other
  boxes and the page edge; the stored box doesn't change. Not for boxes in
  a bubble, drawn or resized by hand, with a size set by hand, or tilted.
  `artwork_rooms`, `room_around`, `smallest_growth_reaching` in
  `renderer.rs`; the switch travels as `growOverArtwork` in each render
  request. Open points: see Known bugs.
- **"Even out sizes" (2026-11-12).** Layout tab, under Size: gives the
  selected boxes (all on the page if none) the middle one of their sizes and
  resizes each around the centre of its text; the box becomes hand-sized,
  so its text can cross the bubble's edge (owner's choice). The button shows
  the size it will use. `ui/lib/evenSizes.ts`, button in
  `ui/components/panels/RenderControlsPanel.tsx`. In a test window 36 of 37
  boxes came out exactly at the middle size, one 1px under.
- **Step settings on the toolbar (2026-11-12).** Small arrows beside Detect,
  OCR, Translate and Inpaint open that step's settings in a dropdown (the
  same settings Settings shows). `ui/components/canvas/StepOptions.tsx`,
  placed by `CanvasToolbar.tsx`. Render has none (none asked).
- **Eyedropper works (2026-11-12).** WebView2's own `EyeDropper` cancels
  itself the instant it opens; in the app the button now picks inside the
  window (cross cursor, the next click takes the colour drawn there, Escape
  stops). Only reads inside the app window. `ui/lib/pickColor.ts`, used by
  `ui/components/ui/color-picker.tsx`.
- **Shorter font picker (2026-11-12).** Lists uploaded fonts, fonts
  installed into Windows for this user and ten Google families
  (`PICKER_GOOGLE_FAMILIES` in `renderer.rs`; edit it to change them).
  Windows' own fonts are hidden from the list only: the renderer still falls
  back to them, and a box that uses a hidden font keeps it.
- **Open and export dialogs remember separate folders (2026-11-12).**
  `lastOpenFolder` in the preferences, `ui/lib/io/openFiles.ts`; after a
  chapter folder the next folder pick starts one level up.
- **Smaller brush (2026-11-11).** Brush, eraser and repair brush go down to
  1px; `[` / `]` move by 1 up to 8px and by 4 above. Under 8px on screen the
  pointer is a crosshair; strokes thinner than 4px are drawn as whole
  pixels (a smooth 1-2px line could vanish on the server); the brush panel
  fades and lets a held stroke pass under it.
- **Favourites (2026-11-13).** Star on page thumbnails (bottom left) and
  project covers (top left); a star button in the page list and a
  "Favourites" pill on the home screen show only starred ones. Saved in
  `favourites.json` in the project's folder (no format change; travels in an
  exported `.khr`; missing or corrupt = nothing starred). Starring is not an
  edit: no undo step, no change to the last-edit time. A deleted page's id
  stays in the file, so undoing the delete brings its star back.
  `koharu-app/src/favourites.rs`, `koharu-rpc/src/routes/favourites.rs`,
  `Navigator.tsx`, `WelcomeScreen.tsx`, `ui/lib/io/favourites.ts`.
- **"Hide the release's lettering here" (2026-11-12 night).** For stray
  release English (BadEnd3 026 "Really bad....."): a release line tied to
  none of the owner's boxes is kept, and every clean-up pastes the release's
  pieces back (`Pieces::apply`), so brushing can't remove it
  (`boxed_erasure`). Right-click on the line → the app finds the release's
  whole line under the pointer, puts an empty box over it (6px margin) and
  re-cleans that spot. One undo step; deleting the box brings the line
  back; refuses (HTTP 409) if the page changed meanwhile. `plan_hide`,
  `Hide`, `HidePlan`, `lettering_at` in `koharu-app/src/official.rs`; route
  `POST /pages/{id}/official/hide-lettering`; UI in `Workspace.tsx`,
  `useBlockContextMenu.ts`, `ui/lib/io/hideOfficialLettering.ts`.

### Before 2026-11

- **Official release underneath (2026-10-01).** A page with a release now *is*
  the release except where your boxes are: their bubble part, or the box over
  art. THUD-style effects come over whole (fills, no Korean showing through),
  Korean the release painted out under your box is gone, and a moan the bubble
  finder merged into your bubble gets the release's version. Near-identical
  pages (all-black ones) pair by file order. Re-run File → Add Official
  Release on a project to update its cleaned pages.
- **Official release onomatopoeia (2026-10-01).** A project can carry its
  chapter's official English release (New project: "Raw pages" +
  "Official release" folders; File → Add Official Release...). Pages pair
  by picture, and wherever you have no text the cleaned page keeps the
  release's onomatopoeia, moans and "..." bubbles. The release's narration
  stays out, even when placed far from your box.
- **Bubble-shaped lettering (2026-09-30).** Unlocked boxes inside a speech
  bubble lay their text out in the bubble's shape (ported from upstream
  Koharu), with joined bubbles split at their seams. Used only when it gives
  bigger text than the box.
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
  are generated at half their width and height and scaled back up (−45 % on
  the Flux2 step, blind A/B 6 better / 6 worse / 8 same). Settings → Engines →
  "Faster cleanup of large areas", on by default.
- **Text hearts (2026-09-29):** flat fill outside bubbles (1fb6c709) and the
  "Repair with LaMa" brush switch (9b5f8a21).

### Tried and not kept

- **FLUX.3 Klein for clean-up (2026-11-12).** Run outside the app through
  stable-diffusion.cpp: one win (Ruination 0062's dark panel at strength
  0.8), otherwise no better than FLUX.2 (left the hand-lettered SFX on
  LOLExtras 0028, wiped out 0062's black bubbles) and about 20 s per small
  crop against 1.5 s. Owner: "looks meh". Downloads and pictures deleted.
- **Rough start variants (2026-11-13/14):** the start in light bubbles
  (whited out art seen through, dropped hearts, gaps at outlines); LaMa (on
  the CPU) drawing the start in dark bubbles (no crisper on 0064, brought
  0063's bright shoulder back, pages twice as slow); 4 steps instead of 2
  for crops with a start (same look, 50 % slower); starting at noise 1.0
  (ignored) or 0.72 and later (kept blurry); a fill averaged over the whole
  crop (grey).
- **Other dark-background attempts (2026-11-12):** hiding only the letters
  from the model instead of the box rectangle (no change at 2 steps);
  telling a bad fill from a good one to retry automatically (the band next
  to the letters differs just as much in good runs); strength 0.85 at 4
  steps (lettering comes back in white bubbles).
- **Repair strokes (2026-11-12):** switching smooth-background strokes to
  LaMa automatically (built, never installed, removed at the owner's wish);
  not restoring the original at all (Flux2 keeps the bad fill it is shown);
  a neutral pre-fill under the stroke (smooth patch, broken lines over
  hair); less context (64px: back in 3 of 5, 128px: 1 of 5).

### Dropped

- **TrashHand border clipping (2026-11-12, project gone, never
  reproduced).** If it comes back: worst on TrashHand Relaxed
  (`fonts/custom/TrashHand-Relaxed.ttf`). The font's glyph boxes match its
  shapes, the text sprite is sized from them (`ink_bounds`,
  `koharu-renderer/src/layout.rs`) and the canvas gets the outline width +
  1px on every side (`stroke_clearance`, `koharu-app/src/renderer.rs`). Odd
  about the font: letters go 304 units below the baseline where it declares
  90 (850 units/em), and 41 letters start left of their origin, so anything
  that crops by font metrics or advance widths would cut it.
