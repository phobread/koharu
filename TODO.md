# To do

## Owner decisions, 2026-11-12 midday (after the overnight report)

- **Text over artwork growing:** the owner dislikes the result and will turn
  it off in Settings. Off by default since 2026-11-13 (owner: "yeah"); the
  switch in Settings turns it on.
- **"Even out sizes":** fine as built; the owner adjusts by hand after.
- **LaMa only when "Repair with LaMa" is ticked.** The automatic switch to
  LaMa on smooth backgrounds is to come out again ("LaMa's inpainting is
  really meh"). Wanted instead: make Flux2 itself stop bringing lettering
  back on a repair stroke.
- **Stray release English:** try option 3 (a "Hide the release's lettering
  here" action).
- **Favourites and the second half of the 5px bug:** go ahead, the owner is
  there to check. Favourites: planned as a side file in the project folder
  (no format change) unless the owner asks for the format change.
- **FLUX.3:** dropped; downloads deleted.
- **How to work:** hard problems first, simple ones later; each separate
  task in its own agent.

## Owner decisions, 2026-11-11 night (apply to the entries below)

- **Overnight work is for Claude Fable 5.2 only.** If the Fable usage runs
  out or the session is moved to another model, stop: no work, working tree
  left as it is.
- **Even font sizes:** an "Even out sizes" action for the selected boxes (the
  whole page if none are selected) sets them to their middle size; each box
  grows or shrinks around its centre. Inside a speech bubble the box is NOT
  held to the bubble's edge for now; the owner wants to try it that way.
- **Text over artwork too small:** let those boxes grow around their centre
  up to the size of the original lettering, stopping at other boxes and the
  page edge. On by default, with a switch in Settings.
- **Step settings on the toolbar:** a small arrow next to Translate opens a
  dropdown with model, target language and the custom prompt. Same idea for
  the other steps: e.g. the Inpaint arrow has the engine, steps and the
  engine's other options. Settings stays as it is; these are shortcuts.
- **Favourites:** a star on page thumbnails and project covers plus a
  "favourites only" filter, saved with the project. No next/previous keys.
  If saving it needs a scene-format change, hold it for the owner instead of
  doing it unattended (use `project.toml` or another side file if possible).
- **Upstream layout model test:** installing Python packages (PyTorch) into
  a throwaway folder on D: is allowed.
- **Stray English in official-release pages:** example is BadEnd3 0026,
  where the release's "Really bad..." survives. Owner's idea: be able to
  switch the view between the raw and the release and pick where to clean
  (paint on the release, or select release bubbles to clean). Open to other
  ideas: diagnose 0026 first, then propose.
- **Hearts damaged next to a traced SFX:** LOLExtras 0028, 0035, 0038, 0084,
  0100, 0104 (and Neeko's Help 0012).
- **TrashHand border clipping:** no longer reproducible, the project is
  gone. Dropped; the entry was removed 2026-11-12. If it comes back, what
  had been checked (cause never found): worst on TrashHand Relaxed
  (`fonts/custom/TrashHand-Relaxed.ttf`); the font's glyph boxes match its
  real shapes, the text sprite is sized from those boxes (`ink_bounds`,
  `koharu-renderer/src/layout.rs`) and the canvas gets the outline width
  + 1px on every side (`stroke_clearance`, `koharu-app/src/renderer.rs`).
  Odd about the font: letters go 304 units below the baseline where it
  declares 90 (850 units/em), and 41 letters start left of their origin, so
  anything that crops by font metrics or advance widths would cut it.
- **Eyedropper:** clicking it does nothing at all.
- **"Flux2 has trouble over some boxes":** part of a broader problem, to be
  looked at separately, not tonight.

## Done recently

- **Text over artwork may grow to the original size (2026-11-12, installed
  2026-11-14; "Text over artwork is too small").** When the automatic
  size of a box that is not in a speech bubble comes out smaller than the
  lettering it replaces, the renderer now gives the text more room around
  the box's centre, as little as it takes to reach the original size, and
  never past another text box or the page edge. The stored box does not
  change; the editor shows the box around the text as drawn, as it already
  does for bubble text. Left alone: boxes in a bubble, boxes drawn or
  resized by hand, boxes with a size set by hand, tilted boxes, and text
  that is already as big as the original (it is never made smaller). Off by
  default since 2026-11-13; switch in Settings → "Text over artwork". Code: `artwork_rooms`,
  `room_around`, `smallest_growth_reaching` in
  `crates/koharu-app/src/renderer.rs` (6 unit tests); the switch travels
  with each render request as `growOverArtwork`
  (`koharu-rpc/src/routes/pipelines.rs`, `ui/lib/io/renderDefaults.ts`,
  `ui/lib/stores/preferencesStore.ts`, `ui/components/SettingsDialog.tsx`).
  `ui/openapi.json` and `ui/lib/api/schemas/startPipelineRequest.ts` got
  the new field by hand, not from the generator: run
  `bun run generate:openapi` and `bun run generate:api` before committing
  to confirm they match. Tried on all four copied projects by rendering
  with the switch off and then on (96 pages): 8 boxes grew, 392 stayed the
  same. LOLExtras 0037 61 → 106px (original 106) and 0029 110 → 121;
  Ruination 0062 25 → 38 and 26 → 38, 0063 30 → 50, 21 → 36 and 43 → 62,
  0064 43 → 55. Where it stopped short of the original, a neighbouring box
  or the page edge was in the way. Pictures (left off, right on):
  `C:\kt\grow_cmp_0037.png`, `grow_cmp_0029.png`, `grow_cmp_0062.png`,
  `grow_cmp_0063.png`, `grow_cmp_0064.png`. The Settings switch itself was
  not clicked in a window (the owner was back at the PC); a unit test
  checks that it reaches the render request. See Lettering for what this
  first version leaves open.
- **"Even out sizes" button (2026-11-12, installed 2026-11-14; owner request
  of 2026-11-10, "Even font sizes across boxes").** Layout tab, under Size.
  It gives the selected boxes (every box on the page when none is selected)
  the middle one of their current sizes, and resizes each box around the
  centre of its text so the text fits at that size. The button shows the
  size it will use ("Even out sizes → 30") and is greyed out when there is
  nothing to even out. Boxes with no text drawn are skipped. A resized box
  becomes a hand-sized (locked) box, so it is no longer held to its bubble,
  as the owner chose: text that grew can cross a bubble's edge. Undo: one
  step for the button, and one for the re-render that follows it, as with
  any style change. `ui/lib/evenSizes.ts`, button in
  `ui/components/panels/RenderControlsPanel.tsx`; 13 tests. How it works:
  a box is scaled by the same factor as its font (plus 3% and the margins
  that don't scale), so nothing has to be measured. Tried in a test window
  with real clicks on the LOLExtras copies of 0036, 0027 and 0100: 36 of 37
  boxes were drawn at exactly the middle size and no text moved off its
  centre. One box came out 1px under (27 for 28): its lines broke
  differently in the new box, the line-breaker mismatch described under
  the 5px bug. Before/after pictures: `C:\kt\even_36_compare.png`,
  `C:\kt\even_100_compare.png`.
- **Step settings on the toolbar (2026-11-12, installed 2026-11-14; owner
  requests of 2026-11-11, "Clean-up settings closer to hand" and
  "Translation settings closer to hand").** Detect, OCR, Translate and
  Inpaint on the toolbar each have a small arrow that opens that step's
  settings in a dropdown. Detect: its four engines (detector, segmenter,
  bubble segmenter, font detector). OCR: its engine. Translate: the whole
  Settings → Translation pane (model, language, custom prompt). Inpaint:
  the engine and, for Flux.2, 2 or 4 steps, "Skip Flux on plain
  backgrounds" and "Faster cleanup of large areas". A change is saved at
  once and is the same setting Settings shows; Settings itself is
  unchanged. `ui/components/canvas/StepOptions.tsx`, placed by
  `CanvasToolbar.tsx`; four tests. Tried in a test window with real mouse
  clicks: all four arrows open, picking "2 steps" saved it, and the
  Translate dropdown shows the translation settings. Not looked at: how
  the dropdowns sit in a narrow window. Render has no arrow (none asked).
- **Eyedropper works (2026-11-12, installed 2026-11-14; owner report of
  2026-11-09, "nothing happens when I click").** Cause: the button used the
  browser's own `EyeDropper`. The app's window (WebView2) has it, but there
  it cancels itself the instant it opens, and a cancel is ignored silently.
  Now, in the app, the button starts a pick inside the window: the cursor
  turns into a cross, the next click on the page takes the colour drawn
  there (the page picture plus whatever layers are showing over it), and
  the colour panel stays open. Escape, or a click on the colour panel,
  stops it. It only reads inside the app window, not other programs.
  `ui/lib/pickColor.ts`, used by `ui/components/ui/color-picker.tsx`; in a
  normal browser the browser's eyedropper is still used. Tried in a test
  window on the LOLExtras copy with real mouse clicks: six picks matched
  the page's pixels, and the click did not draw or select anything.
- **Shorter font picker (2026-11-12, installed 2026-11-14; owner request of
  2026-11-09).** The picker used to list all 1,820 Google families and
  ~430 Windows fonts, and downloaded every Google font it scrolled past.
  It now lists only uploaded fonts (`fonts/custom`), fonts installed into
  Windows for this user (`%LOCALAPPDATA%\Microsoft\Windows\Fonts`, e.g. CC
  Wild Words) and ten Google families: Comic Neue, Bangers, Patrick Hand,
  Caveat, Pangolin, Kalam, Indie Flower, Architects Daughter, Shantell
  Sans, Noto Sans (`PICKER_GOOGLE_FAMILIES` in `koharu-app/src/renderer.rs`,
  edit that list to change them). Windows' own fonts are hidden from the
  list only: the renderer still falls back to them, and a box that already
  uses a hidden font keeps it and shows its name in the picker. On the test
  copy the list went to 24 families (10 uploaded, 10 Google, CC Wild Words
  and the three CaskaydiaCove terminal fonts, which are installed per user
  too); re-rendered pages kept their sizes. Not seen on screen yet.
- **Open and export dialogs remember separate folders (2026-11-12, installed
  2026-11-14).** Opening pages, a folder or a `.khr` now starts where the
  last pages came from, not in the export folder (`lastOpenFolder` in the
  preferences, `ui/lib/io/openFiles.ts`). After picking a chapter folder the
  next folder pick starts one level up, beside it. Unit-tested only: the
  native dialog can't be checked without the owner, so look at it after the
  next install.
- **Smaller brush (2026-11-11, installed 2026-11-14).** The brush, eraser and
  repair brush go down to 1px: the slider moves in steps of 1, and `[` / `]`
  move by 1 up to 8px and by 4 above that. When the brush ring is under 8px
  on screen the pointer shows as a crosshair. Repair and eraser strokes
  thinner than 4px are drawn as whole pixels: a smooth 1-2px line was partly
  see-through, and the server keeps a mask pixel only above half white, so
  such a stroke could vanish. The brush size panel now fades and lets the
  pointer through while a stroke is held, so a stroke can pass under it
  (starting a stroke under the panel is still blocked). Tests pass; not yet
  looked at in the running app (needs a build).
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
  release's onomatopoeia, moans and "..." bubbles. On BadEnd 1-20 every
  copied piece was onomatopoeia; the release's narration stays out, even
  when placed far from your box.
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

## Official release

- **Try it on other chapters.** Tuned on BadEnd 1-20 and BadEnd3 page 1. Watch
  for the release's English peeking out next to your text (a line of theirs
  the rule didn't tie to your box) and for small Korean bits where Korean
  crosses a dark art line under your box (a repair stroke clears them).
  Telling a merged moan from your own line needs Detect's erase mask.
- Deleting a box on a cleaned page with a release takes ~2.5-3 s (the rule
  runs again).
- **Stray English left behind (owner report, 2026-11-09).** The owner calls
  this the "raw and translation overlay". It has trouble removing stray
  English: bits of the release's lettering stay on the cleaned page where
  they should have gone. This is the "English peeking out" case above, now
  seen in real use. Lettering is tied to the owner's boxes in
  `koharu-app/src/official.rs` (per bubble part inside bubbles; outside
  bubbles only groups touching a box over artwork or cleanup's erase area).
  Collect example pages before changing the rule.
  **Diagnosed 2026-11-12 on a copy of BadEnd3 026 ("Really bad....."), no
  code changed.** What happens there:
  - The release split the Korean line 위험...♥ into two English lines and
    put them in two places: "That would be...." beside the Korean, and
    "Really bad....." 470px further down, where the raw page has no
    lettering at all.
  - A line of the release's is the owner's only if it touches one of their
    boxes (or the erased lettering of a box). Nothing ties the second line
    to a box, so it is treated as the release's own and kept.
  - The owner brushed over it (the stroke is in the page's erase mask), and
    it still came back: every clean-up ends by pasting the release's
    pieces over its result (`Pieces::apply`), and a stroke that touches no
    box is read as "a Korean effect brushed out, the release's version of
    it comes over" (`boxed_erasure`). So brushing can't remove the
    release's lettering, by design.
  - What does work today: draw an empty box over the stray line and run
    Inpaint again. The owner already did this for "That would be...." (the
    empty box at 413,1861). Tried on the copy for "Really bad.....": gone
    (picture `C:\kt\b26_emptybox.png`).
  Options, for the owner to choose (none built):
  1. *A brush stroke removes release lettering where the raw page has
     none.* Smallest change, and it makes the brush do what the owner
     expected. Risk: where the release drew its English right on top of
     the Korean, the raw's lettering is hidden and the rule could mistake
     Detect's own mask for a brush stroke and drop an effect the release
     should keep. Needs a check on BadEnd 1-20 before it is trusted.
  2. *The owner's idea: a "Release" view next to Original / Cleaned /
     Translated, and brushing there means "not the release's here".*
     Clearest to use, and no guessing. It needs its own saved mask per
     page, which is a change to the project file format (version bump).
  3. *Make the empty-box trick a named action:* right-click → "Hide the
     release's lettering here" draws the empty box and re-cleans that
     spot. No format change, no rule change, works for whole lines.
  My pick: 3 now (cheap, safe), 2 if stray lines keep turning up.
  **Option 3 built 2026-11-12 night (installed 2026-11-14).** On a page with
  a release underneath, right-click on the stray line in the page view and
  pick "Hide the release's lettering here". Nothing is drawn by hand: the
  app finds the release's line under the pointer (a click within about 1%
  of the page width of a letter counts, the whole line is taken), puts an
  empty text box over it with a 6px margin and re-cleans that spot. One
  undo step takes back both the box and the picture. Deleting the box
  brings the line back.
  - Where the raw page has nothing under the line, the spot goes back to
    the raw page without the GPU (3-5 s on 026). If the owner brushed over
    the line first, the configured inpainter cleans just that region (8 s
    with Flux2 loaded).
  - Nothing under the pointer: a notice, nothing changes. Lettering that
    would stay even with a box on it (a bubble tied to a moan the release
    re-lettered): the action changes nothing and says so.
  - If the page changes while the action works, it refuses (HTTP 409)
    instead of overwriting newer work.
  - Code: `plan_hide`, `Hide`, `HidePlan`, `cleanup_inputs`, `lettering_at`
    in `koharu-app/src/official.rs` (`piece_mask` output unchanged), route
    `POST /pages/{id}/official/hide-lettering` in
    `koharu-rpc/src/routes/pages.rs`, UI in `Workspace.tsx`,
    `useBlockContextMenu.ts`, `ui/lib/io/hideOfficialLettering.ts`. No
    format change: the box is an ordinary empty text box.
  - Proven on the BadEnd3 026 copy: "Really bad....." gone, picture changed
    only inside the new box, a later full-page Flux2 clean-up keeps it gone
    and leaves every other piece of the release (96,406 px before and
    after), undo / redo / deleting the box pixel-exact. Pictures
    `C:\kt\hide_026_*.png`.
  - Open: the right-click menu was only unit-tested, not tried in a window.
    Right-clicking the bare picture on a release page now opens a menu
    (it opened nothing before). Two lines closer than about 40px count as
    one; a sound effect of several big letters needs one click per letter.
    Where the release lettered its English on top of the Korean, hiding it
    can leave Korean fragments (same as the hand-made empty box). A later
    OCR run may try to read the empty box. The brushed-over case was run
    once only.

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

- **Bug: one word change can drop a whole box to 5px (found 2026-11-08).**
  SlaveMarket 0019 box 7 (resized box, 3px border) went from ~28px to 5px
  when "illicit" was deleted. `fit_font_size` (`koharu-app/src/renderer.rs`)
  tries the floor size first (6px for resized boxes). If that fails, it
  assumes no bigger size fits and searches only below the floor. The floor
  failed by a hair: greedy line breaking packs lines by advance width, but the
  fit check measures ink width plus a 2px pad, and the 6px first line came out
  ~283.9px in a 284px box. The same mismatch can make the normal binary search
  settle a little small on other boxes. Fix: don't let a floor failure end the
  search (or probe downward from the top, as `fit_balloon_font_size` does), and
  make the line breaker and the fit check measure the same width. Workaround
  until then: give the box a font size, or nudge its width.
  **First half fixed 2026-11-12 (installed 2026-11-14):** a miss at the floor
  no longer ends the search; the fit tries the largest size, then the floor,
  then looks above the floor before it ever goes below it (`auto_fit_size`
  in `renderer.rs`, three unit tests on the search itself). Old against new
  code on 12 copied pages (LOLExtras and Ruination, 123 boxes): no box
  moved by more than 1px. The real case couldn't be replayed (SlaveMarket
  is no longer on disk), so watch for a 5px box after the next install.
  **Second half fixed 2026-11-12 afternoon (installed 2026-11-14, owner to
  look at the pictures):** the line breaker for horizontal text in a box
  now packs each line by the same width the fit check measures. What
  differed: lines were packed by advance width including the trailing
  space, and checked by ink width (which can overhang the advance, e.g.
  Bangers' "A" and dashes) plus a 2px pad, measured from the leftmost ink
  of all lines. `LineInk` and `INK_PAD` in `koharu-renderer/src/layout.rs`
  (4 tests there, 1 in `renderer.rs`; all 5 fail with the change off).
  Vertical text and the bubble-shaped path are unchanged (the bubble path
  never had the mismatch). Old against new on the four copied projects (96
  pages, 400 boxes, text growth off): 346 identical, 37 bigger (21 by 1px;
  the big ones are hand-sized boxes the old search left far too small,
  e.g. LOLExtras 0105 18 → 40, 0115 23 → 38, 0083 12 → 21, Neeko 0012
  40 → 55), 2 smaller (LOLExtras 0068 46 → 45, Ruination 0045 30 → 28, both
  with tidier breaks), 15 same size with moved line breaks. Every box with
  a size set by hand is now drawn at that size (37 of 37; one was 1px
  under before). Crops, left before and right after: `C:\kt\lb_compare`
  (25 pictures), list in `C:\kt\lb_compare.log`. For the owner to judge:
  some hand-sized boxes now fill much more of their bubble (crops 01-07);
  a fuller early line can leave a short line later (BadEnd3 031, "LIKE A"
  alone); a box line may start with a dash ("—♡ TORMENT" on LOLExtras
  0100), which the bubble breaker forbids and the box breaker never did.
  **Dash fixed 2026-11-13 afternoon (installed 2026-11-14):** a box line no
  longer starts with a dash. A dash stays with the word before it, so the
  break moves one word back; only when the word before the dash is alone
  on its line and the dash does not fit beside it does the dash go down
  (rather than out of the box). `dash_follows` in
  `koharu-renderer/src/layout.rs`, used by both box breakers (by ink and
  by advance); dashes covered: U+2012 to U+2015 and the two long ones
  (U+2E3A, U+2E3B). The bubble path is unchanged. 2 tests (the layout one
  fails with the rule off). Old against new on the four copied projects
  (400 boxes): 398 identical, 2 changed, both on LOLExtras 0100 ("BUT,
  BUUUT" / "—♡" is now "BUT," / "BUUUT—♡"; "STUFF ME MORE" /
  "—♡ TORMENT" is now "STUFF ME" / "MORE—♡", and the heart that
  hung alone on the last line moved up beside its dash). Crops
  `C:\kt\lb_dash_compare`.
  Not tested with the owner's own font and border (the runs used the
  request defaults, Bangers). Vertical text can still have the same
  hair-miss.
- **Text over artwork: what the first version leaves open (2026-11-12).**
  The growth itself is under "Done recently". Still to judge or do:
  - A caption frame or sign that the bubble detector doesn't see counts as
    artwork, so its text grows to the original size and can fill the frame
    edge to edge (Ruination 0063, `C:\kt\grow_cmp_0063_zoom.png`; LOLExtras
    0037 sign). If that looks wrong, the growth needs a limit that reads
    the picture (stop where the background changes), or a smaller target.
  - Only other text boxes and the page edge stop the growth. A speech
    bubble with no box near its edge does not.
  - A neighbour beside a box also caps how far it grows up and down (the
    box grows by the same amount on all sides).
  - It needs a measured original size (`fontPrediction.fontSizePx`), so a
    box with no font detection result never grows.
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
- **Pages to inspect (owner, 2026-11-10).**
  - Ruination 0062, 0063 and 0064: removing black backgrounds.
  - Neeko's Help 0012: tricky hearts.
- **Flux2 has trouble over some boxes (owner report, 2026-11-10).** No repro
  yet: the owner doesn't know which boxes or what sets it off. Keep it in
  mind. When it comes up again, save the page and box (and the crop if
  possible) and note what the cleanup did wrong.
- **Bug: a repair stroke brings back lettering it didn't touch (owner
  report, 2026-11-11).** A long-standing one. LOLExtras 0026, the Korean
  FWWASH SFX in the bottom-right corner (hand-drawn, so cleaned by hand):
  the first strokes left a small bit at the bottom; a new stroke over just
  that bit made a larger part above it come back. Same on Ruination 0045.
  Reproduced 2026-11-11 on an isolated copy of 0026 (a 16px stroke over the
  leftover bit, Flux2, 2 steps): the bit stayed, grew upward to the top of
  the stroke's rectangle and turned red. Cause (corrected the same evening):
  the leftover bit is already inside the page's erase mask, so the stroke
  adds no new mask pixels. The app treats that as "redo": it puts the
  original page back under every mask pixel in the stroke's rectangle
  (`restore_masked_from_source`, `koharu-app/src/pipeline/engines/
  flux2_klein.rs`), which brings back the piece of the SFX stroke that the
  rectangle covers, and Flux2, shown only that clipped piece in a 160x192
  crop, keeps it as an object and recolours it. A first fix attempt
  (restore only newly added pixels) changed nothing because this stroke
  adds none; it was taken out, and its "ruled out" conclusion was wrong.
  LaMa on the same stroke removed the fill (the outline stayed only where
  the test stroke missed it). **Fixed for plain backgrounds 2026-11-12 (never
  installed, replaced below):** a Flux2 repair stroke whose surroundings are smooth
  (plain colour or a soft gradient, measured in a 24px band around the
  stroke with the mask left out) is filled by LaMa instead
  (`stroke_surroundings_are_smooth` in `koharu-app/src/pipeline/engines/
  support.rs`, used by `put_mask` in `koharu-rpc/src/routes/pages.rs`). On
  the 0026 copy a stroke covering the scrap removed it in 0.5 s and nothing
  came back; a stroke over artwork still runs Flux2; full-page runs are
  untouched. Still open: the same thing over artwork (Ruination 0045), where
  the surroundings aren't smooth and Flux2 still redraws what it is shown.
  Ideas left for that: on a redo stroke don't put the original back at all,
  or redo the whole connected piece of lettering with a proper crop.
  **Replaced 2026-11-12 afternoon (installed 2026-11-14): the LaMa switch is
  out again, Flux2 handles it itself.** The owner wants LaMa only when
  "Repair with LaMa" is ticked, so `stroke_surroundings_are_smooth` and its
  use in `put_mask` were removed (`koharu-rpc/src/routes/pages.rs` is back
  to its committed text). Correction to the diagnosis above: there is no
  separate "redo" branch. Every stroke that removes no mask pixels put the
  original back under all mask pixels in its rectangle; on lettering that
  was already cleaned this brought a clipped piece back, so ordinary
  tracing strokes that overlap earlier ones were hit too. The fix, in
  `koharu-app/src/pipeline/engines/flux2_klein.rs` (helpers in
  `support.rs`, 8 unit tests): when the stroke's rectangle holds cleaned
  pixels, the original is put back under the whole connected piece of
  lettering the stroke lies on (up to 256px from the stroke,
  `REDO_CONTEXT_PX`), Flux2 cleans that piece as a full-page run would,
  and only the stroke's rectangle is copied into the cleaned page. A
  stroke on lettering never cleaned takes the old path unchanged. Replays
  on the copies, each case run twice or more with a restart: 0026 scrap
  (plain background) clean with both stroke sizes; three spots over the
  hair on 0026, where the old path brought a magenta bar back in 6 of 6
  runs, 0 of 6 now with the hair lines intact; Ruination 0045 leftover arc
  0 of 2 (old path 1 of 3). No regression: the 0028 "hat" stroke and a new
  stroke over artwork give the same picture as before bit for bit, and so
  does a full-page run on 0006. Sheets (old run 1, old run 2, new run 1,
  new run 2, each before | after | changed): `C:\kt\redo_compare_*.png`.
  Tried and not kept: not restoring at all (Flux2 keeps the bad fill it is
  shown); a neutral pre-fill under the stroke (fast, nothing returns, but
  a smooth patch and broken lines over hair); less context (64px: back in
  3 of 5, 128px: 1 of 5). Costs and limits: such strokes take 3 to 5 s
  instead of about 1.5 s; a stroke only changes its own rectangle, so a
  sliver sticking out beside it needs a second stroke; there is a hard cut
  at the rectangle's edge between new and old fill (nothing visible in the
  tests, judge by eye); evidence is 2 pages and 8 spots at 2 steps; not
  tried with the real brush in a window, on official-release pages, or on
  lettering made of separate blobs with no text box around them.
- **Bug: clean-up changes things next to the stroke (owner report,
  2026-11-11).** Tracing a Korean SFX by hand removes or deforms the
  hand-drawn heart beside it; the owner wants those hearts kept (both pages
  of Neeko's Help). **Fixed 2026-11-12 (installed 2026-11-14).** Cause, found
  on a copy of LOLExtras 0028 (the 핫 SFX traced by hand, small heart 8px
  away): before clean-up the erase mask is widened, and a stroke that falls
  inside a text box is widened by that box's amount. A hand-added box has no
  measured letter size, so it gets the maximum, 8px, which took the heart's
  right lobe. (Outside a box a stroke grew with its own size, up to 6px.)
  Now a brush stroke that adds to the mask is erased as drawn, widened by
  2px only (`expand_hand_stroke_mask_for_inpainting` in
  `koharu-ml/src/inpainting/mask.rs`, chosen by `glyph_erase_mask` in
  `koharu-app/src/pipeline/engines/support.rs`; Flux2, LaMa and AOT).
  Full-page runs and eraser strokes keep the old widening. Re-test on 0028:
  lettering gone, heart untouched, no changed pixel more than 2px from the
  stroke (pictures `C:\kt\t_hat_fixed.png` before, `t_hat_fix2.png` after).
  The other side of it: a stroke now has to cover the lettering itself; a
  speck the tracing misses stays (it used to be swallowed by the widening).
  Not looked at: the eraser side (erasing the mask over a heart to get it
  back still re-cleans nearby lettering with the wide margin), and hearts
  lost by the automatic full-page run inside text bubbles (seen on LOLExtras
  0035), which is a different path.
- **Flux2 cleared a dark background (owner report, 2026-11-11).** Ruination
  0062: first runs on boxes 3 and 14 (top left) and on the joined 4/5/6/7
  box removed the black overlay behind the text instead of keeping it.
  Re-running fixed it. LaMa is slightly better there but much worse
  elsewhere, above all on small boxes. Use these boxes in the FLUX.3 trial.
  **Looked into 2026-11-12, reproduced, not fixed.** Full-page Flux2 runs
  on a copy of the page (the owner's current mask), scored against the
  owner's good result:
  - The 4/5/6/7 panel came out wrong in 7 of 8 runs at 2 steps (the
    owner's setting) and in 7 of 8 at 4 steps: part of the fill is painted
    in the pale colour of the art outside the panel, as if the dark overlay
    weren't there. Once it brightened most of the panel.
  - Boxes 3 and 14 were right in all 16 of those runs, but in an earlier
    set of 5 runs at 4 steps one came out teal and one brownish (the
    reported "turned clear").
  - It is the model's fill that is wrong, not the flat fill (the log says
    every crop on this page is generated) and not the paste (the colour
    matching only corrects small, smooth differences; it leaves a gross one
    alone on purpose).
  - The "random" start of each fill is the same sequence after every app
    start: the eight runs of two separate sessions matched pixel for pixel.
    So a page fails or passes depending on how many fills came before it,
    which is why a re-run "fixes" it.
  - Tried and taken out: for lettering outside a bubble, hiding only the
    letters from the model instead of the whole box rectangle. At 2 steps
    it changed nothing (4 of 5 wrong); at 4 steps the panel was right in 4
    of 5, but that is too few runs to call, and it would change every piece
    of lettering over artwork.
  - Tried and dropped: telling a bad fill from a good one by comparing the
    band next to the letters with the page (to retry automatically). On
    the panel the band differs just as much in the good run, because the
    model also removes the faint grey lettering the mask doesn't cover.
  - Strength 0.85 (4 steps) is worse: lettering comes back in the white
    bubbles.
  Ideas left: start the fill from a rough pre-fill (LaMa or the
  surrounding colour) so the darkness survives; give Flux2 a second try
  with a different start when the result is far from the pre-fill. Pictures
  in `C:\kt`: `r62_4567_runs.png`, `r62_4567_exp.png`, `r62_tl_st4.png`.
  **Fixed for lettering outside bubbles 2026-11-13 (installed 2026-11-14).**
  Lettering outside every bubble whose surroundings are one plain colour
  (90 % of a 10px band within 24 of its median colour) now starts from a
  rough fill of those surroundings: the fill runs its first step as before,
  then the regenerated region around that lettering is swapped for the
  rough fill plus noise at noise level 0.9 (after step 1 of 2, step 2 of
  4) and the model finishes from there. New file
  `koharu-ml/src/flux2_klein/rough_fill.rs` (`plain_surroundings`,
  `rough_fill`, 12 unit tests); `RoughStart` and `ROUGH_START_SIGMA` in
  `koharu-ml/src/flux2_klein/mod.rs`, used by every generated crop (full
  page and repair strokes); nothing else changes when no lettering
  qualifies.
  - Result in the app on the `C:\kt` copy of 0062 (2 steps): the 4/5/6/7
    panel right in 8 of 8 runs in a row (was 7 of 8 wrong), boxes 3 and 14
    unchanged. Picture: `C:\kt\r62_fix_panel.png` (top: old start, bottom:
    new build).
  - No regression found on LOLExtras 0026, 0028, 0035, Neeko's Help 0012
    and Ruination 0045, 0063, 0064 (2 to 4 full-page runs each, scored
    against the owner's cleaned pages).
  - What was learned on the way: started from a rough fill at noise 1.0 the
    model ignores it; started at 0.72 or later it keeps it as it is
    (blurry). A rough fill averaged from the whole crop came out grey (the
    pale art outside the panel leaked in), so it uses only a band around
    the lettering. Inside small black bubbles the lettering leaves almost
    no bubble to fill from, and the start made them grey in 8 of 8 runs, so
    bubbles are left out. Over artwork of two colours (the LOLExtras 0026
    SFX across hair and white) the rough fill left a purple smear, hence
    the plain-surroundings test (0026 scores 0.26, the dark panels 0.99 to
    1.00). Only the lettering at first, not the whole regenerated box, left
    pale patches at the box's edge in 3 of 4 runs. A box that also holds
    lettering in a bubble (a black bubble the map only partly covers,
    Ruination 0063) is left out too.
  - No automatic retry was added: with the new start no run of the panel
    failed.
  - The "same random sequence after every launch" side finding was left as
    it is (it makes A/B tests exact: same page order, same noise).
  **Extended to dark bubbles 2026-11-13 night (installed 2026-11-14).** The
  see-through black bubble on Ruination 0063 (right side, lower half of the
  "3") showed the shoulder behind it at full brightness in 4 of 6 runs.
  Lettering in a dark bubble (no channel of its colour above 64) now gets
  the same start when 75 % of its surroundings at least 12px inside the
  bubble map (one map cell is about 12.5px on a 2000px page, so nearer the
  edge a pixel may be outline or page) are within 48 of their median
  colour; the rough fill then uses only surrounding pixels in that bubble
  close to that colour, so the outline and stray marks don't grey it, while
  dim art seen through it carries on (`bubble_colours`, `BUBBLE_*` and
  `DARK_BUBBLE_MAX` in `rough_fill.rs`, 17 unit tests now).
  - Result: the 0063 bubble right in 8 of 8 runs (the shoulder stays dim
    and fades out under the lettering). A/B on the same noise, 3 runs each
    of Ruination 0045, 0062, 0063, 0064, LOLExtras 0026, 0028, 0035,
    Neeko's Help 0012: 0064's see-through bubble lost a big pale patch in
    the one run that had it; otherwise the dark bubbles come out a little
    flatter black (less of the faint green glow on 0064's SFX bubbles),
    outlines intact. Light-bubble pages are pixel for pixel as before.
    Picture: `C:\kt\r63_fix_bubble.png` (source | owner | before | after,
    0063 and 0064).
  - Tried and taken out: the same start in light bubbles. It stopped one
    bad run on LOLExtras 0035 (teal hair painted into three see-through
    white bubbles) but in every run whited out the art seen through the
    bubble, dropped the hearts in the mask that Flux2 otherwise redraws,
    and on Neeko's Help 0012 left gaps where lettering touches the bubble
    outline (the start fills those pixels with the bubble's white).
  - Still open: the two small black SFX bubbles top left on Ruination 0063
    come out grey or teal in 6 of 6 runs (before and after this change;
    the owner's page has them black). The lettering and its widened mask
    nearly fill them, so too little bubble is left well inside the map to
    judge the colour; they start as before.
  - Owner, 2026-11-14: on 0064 their own page (an old-build run from
    noise) is better than the new start: the shoulder seen through the
    bubble has a crisp edge there, while every run with the start shows it
    as a soft blur (its edge lies wholly under the lettering, and the start
    blends across it). Tried and taken out the same night, 3 runs each on
    the same noise of Ruination 0045, 0062, 0063, 0064 (picture
    `C:\kt\r64_lama_try.png`: source | owner, then blend / LaMa / 4 steps):
    - LaMa (on the CPU; beside Flux2 on the 6 GB card it would spill) drawing
      the start in dark bubbles: no crisper on 0064, and its own fill there
      has a dark gash that came through in 1 of 3 runs; on 0063 it brought
      the bright shoulder partly back in 3 of 3 (once a hard grey block);
      3-18 s per crop, pages took twice as long (32-48 s against 16-20 s).
    - 4 steps instead of 2 for crops with a start (two steps after the
      swap instead of one): looks the same as 2 steps, pages 50 % slower.
    Left: an on/off setting for the start, so a page can be re-run from
    noise as before (bright patch risk back), not built.
- **Try FLUX.3 Klein for clean-up (owner request, 2026-11-11; trial done
  2026-11-12). Closed: the owner looked at the pictures the same day
  ("looks meh") and dropped it. The tool and model downloads (8.4 GB) were
  deleted; only the pictures in `D:\flux3-trial\results` (4 MB) remain.**
  A 5.6B model (ours is FLUX.2 Klein 4B); upstream's `v3` branch runs it
  through stable-diffusion.cpp and inpaints it the same way as FLUX.2. Our
  app can't load it (the candle code only knows the FLUX.2 transformer), so
  the first look is outside the app: stable-diffusion.cpp's `sd-cli` with the
  Q4 file (3.9 GB) and the Qwen3-VL 4B text encoder (2.5 GB), in
  `D:\flux3-trial` (delete the folder when the trial is over). Compare with
  FLUX.2 on the pages under "Pages to inspect". Load check 2026-11-11: it
  runs on the 6 GB card with `--offload-to-cpu` (parts take turns on the
  GPU: 2.1 GB text encoder, 3.4 GB model, then the VAE); a 512x512 image at
  16 steps sampled in 14 s. First comparisons 2026-11-12 (pictures in
  `D:\flux3-trial\results`, upstream's settings and its prompt "Remove the
  text and reconstruct the background."): the 0026 leftover fragment stays
  at strength 0.8 and goes at 1.0; the hand-lettered SFX on LOLExtras 0028
  is NOT removed at either strength (the crop comes back unchanged), and
  without the reference picture the model invents shapes and damages the
  bubble. Ruination 0062 (2026-11-12, `C_0062_tl_compare.png` and
  `D_0062_4567_compare.png`): on the dark 4/5/6/7 panel FLUX.3 at strength
  0.8 is clean and keeps the panel dark, better than a fresh FLUX.2 run; at
  1.0 it brightens one corner. On the black bubbles (boxes 3 and 14) it
  wipes out the whole bubble at both strengths, worse than FLUX.2. Overall:
  one win (the dark panel at 0.8), otherwise no better than FLUX.2, and
  slower (about 20 s per small crop against 1.5 s). Not worth porting on
  this evidence. If it wins after all, getting it into
  the app means either porting its transformer to candle or shipping
  stable-diffusion.cpp.

## Interface

- **Favourite / bookmark pages and projects (owner request, 2026-11-11).**
  Mark pages inside a project and projects on the home page, and be able to
  find them again (a star on the thumbnail in `ui/components/Navigator.tsx`
  and on the project covers; a "favourites only" filter or favourites sorted
  first). To decide: where the mark is saved (with the project, so it
  survives a reinstall, or in the app's preferences) and whether page
  bookmarks need jump-to-next / previous keys.
  **Held 2026-11-12, not built.** The owner chose "saved with the project".
  A page (`Page`) and a project (`ProjectMeta`, both in
  `koharu-core/src/scene.rs`) have no spare field to put a star in, and the
  project file is written field by field in a fixed order, so adding one is
  a file-format change: `SCENE_FORMAT_VERSION` goes up and the old layout
  has to be frozen in `session.rs::mod compat` (AGENTS.md, "Hard rules").
  That was the stop condition for doing it unattended. Two ways on:
  (a) do the format change with the owner there to check old projects still
  open; (b) keep the stars in the app's own settings instead, keyed by
  project and page. (b) needs no format change and survives restarts, but
  the stars don't travel inside an exported `.khr` and are lost if the
  app's settings are reset.
  **Built 2026-11-13 morning as a side file (installed 2026-11-14, not seen
  in a window).** No format change. Stars live in `favourites.json` inside
  the project's own folder (`{version, project, pages: [page ids]}`), so
  they survive a reinstall and travel inside an exported `.khr` (the
  export zips the whole folder). A missing or corrupt file means "nothing
  starred". Stars are not edits: no undo step, and starring does not
  change a project's last-edit time or its place in the list.
  - Page list: star at the bottom-left of each thumbnail (shown when set,
    on hover when not); a star button in the Navigator header shows only
    starred pages, under their real page numbers. The filter resets when
    the list is opened afresh.
  - Home screen: star at the top-left of each cover; a "Favourites" pill
    next to the project count shows only starred projects (stays on until
    the app restarts).
  - No next / previous keys (not asked for). The Page Manager dialog shows
    no stars.
  - Code: `koharu-app/src/favourites.rs`, `koharu-rpc/src/routes/favourites.rs`
    (`PUT /projects/{id}/favourite`, `GET /projects/current/favourites`,
    `PUT /pages/{id}/favourite`), `favourites` added to the `GET /projects`
    answer in `routes/projects.rs`; UI `Navigator.tsx`, `WelcomeScreen.tsx`,
    `ui/lib/io/favourites.ts`, `favouriteProjectsOnly` in `editorUiStore.ts`.
  - Proven on the isolated copies over HTTP (34 checks): `scene.bin`,
    `history.log` and `project.toml` byte-identical before and after
    starring, stars survive a restart, a project with no file or a corrupt
    file lists and opens normally.
  - To look at in the window: whether the star reads well over artwork on
    the small thumbnail, and that it does not crowd the delete button.
  - A deleted page's id stays in the file on purpose, so undoing the
    delete brings its star back.

## Upstream

Looked at on 2026-11-11 from a fresh clone outside the repo (upstream now
lives at [koharu-rs/koharu](https://github.com/koharu-rs/koharu)). Read from
commit messages, docs and module headers; none of upstream's code or models
was run, and its numbers come from Japanese manga, not Korean pages.

- **Where upstream is.** The fork split at 0.61.2 (2026-06-17). Upstream
  rebuilt everything on 2026-07-30 (libtorch instead of candle,
  stable-diffusion.cpp for clean-up, a WebGPU canvas with layers, one layout
  model for detection) and released up to 0.83.5 (2026-09-22), the state the
  bubble-shaped lettering was ported from. Since then `main` has had five
  real code commits and no release. A second rewrite is under way on the `v3`
  branch (2026-10-21 onward): the app, pipeline and canvas crates are deleted
  there and the project storage changes again, so `v3` is not a usable app.
- **Rewrite or overhaul: neither, for now.** `main` is being abandoned and
  `v3` has no app. Look again when `v3` ships a release. Keep porting ideas
  by hand.
- **Ideas worth taking** (all on `v3` unless noted):
  - Measure the original lettering (ink size, stroke width, colours, tilt:
    `koharu_layout_highres/measure.rs`, `koharu-leptonica`). Use: tell the
    fitter how far a box over artwork may grow ("Text over artwork is too
    small"). Owner: translations often run larger than the original, so it
    must not become a fixed size; add a toggle if auto does badly.
  - Clean-up masks stretched to whole letters (stroke ends the detector
    missed, the inside of outlined letters; commit b9accbfb). Plain image
    processing, so it could work on our masks. Aimed at leftover slivers and
    outlined text; not shown to help the hearts.
  - Flat fill that never paints over a balloon's outline (846c7f30). Compare
    with our flat-fill rule.
  - Layout model with onomatopoeia and panel classes
    (`mayocream/koharu-layout-highres`, RF-DETR Seg 2XL). Would cover SFX and
    the official-release "merged moan" case, and gives panels for a
    panel-by-panel reading order (`order.rs`, written for right-to-left
    Japanese). Runs on libtorch upstream, so using it in the app means a
    hand port to candle. Test it on about ten of the owner's pages outside
    the app before anything else.
    **Tested 2026-11-14 outside the app, no code changed.** Model
    `mayocream/koharu-layout-highres` (186 MB, 46.6M parameters, Apache-2.0,
    input 1920x1920), run with the `rfdetr` 1.7.0 Python package on the CPU
    (about 20 s a page, 8 threads) and the pre/post-processing of the
    model's own Hugging Face demo. 11 pages from the `C:\kt` copies:
    LOLExtras 0006, 0026, 0028, 0035, 0038, 0084, 0115; Ruination 0062,
    0063; Neeko's Help 0012; BadEnd3 026. Compared with the boxes the app
    detected and the boxes the owner added by hand (a box with no detector
    name). The trial folder (`D:\layout-trial`: throwaway Python, model,
    scripts, side-by-side pictures) was deleted the same day after the owner
    had seen the results; re-running means reinstalling `rfdetr` 1.7.0.
    - SFX, the main hope: the owner's hand-added boxes were nearly all
      found, almost all labelled SFX (counted by box overlap: 46 of 49; that
      count is loose on busy pages). Clean cases: all 10 traced SFX on
      LOLExtras 0028, all four big SFX on 0115, FWAAASH on 0026, 게익…쾅 on
      Neeko 0012. Its SFX masks follow the letter shapes, outlined letters
      included, so they could start a clean-up mask instead of tracing by
      hand. Hearts touching an SFX are often inside its mask (the owner
      wants those kept).
    - Misses and false finds: dense moan clusters (Neeko 0012 bottom left)
      come out as overlapping low-score text boxes with the hearts counted
      as text; some vertical moans only reach 0.2-0.3 (0115 응오오옷, Neeko
      앗앗); printed words on clothes and a halo were called SFX (Neeko 0012
      hoodie, Ruination 0063).
    - Dialogue: matched 74 of 76 of the app's own boxes. The 2 misses are the
      small black SFX bubbles top left on Ruination 0063.
    - Bubbles: finds the black caption frames the app's bubble finder misses
      (all four on Ruination 0063, the "caption frame counts as artwork"
      case) and skips some false bubbles of the app's (fur cuff on 0115). But
      on the dark, colourful BadEnd3 026 it found no bubbles and no panels at
      all (the app found all 7 black bubbles), and its text scores there were
      low (0.2-0.5). It also misses the wild-edged moan bubbles on Neeko 0012.
      Trained on black-and-white Japanese manga only.
    - Getting it into the app: the app already runs ONNX models through `ort`
      (`koharu-ml/src/korean_ocr`), so an ONNX export of this model loaded
      the same way is likely much less work than a candle port.
    - Best fit: as an extra SFX finder (and caption-frame finder) beside the
      current detectors, not as a replacement for them.
  - "Rooms" for texts in joined balloons (`rooms.rs`): a newer version of
    the seam search we ported. Not urgent.
  - RORem clean-up (`main`; SDXL-based object removal through
    stable-diffusion.cpp). Never tried here; likely tight on 6 GB.
  - Language and instructions in the processing selector (`main`): a design
    reference for "Translation settings closer to hand".
- **Not useful here.** New OCR models are Japanese-only (Baberu,
  manga-ocr-ar) or already rejected (Hayai); local translation models, more
  providers, Linux/AMD support, interface languages, sending the page image
  to the translation model (owner: redundant). Archive/PDF import and CBZ
  export only if wanted.
