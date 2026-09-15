# Verify Flux2 text-removal compositing

This check covers the Flux2-specific path that generates a broad text-region
fill and pastes it through a tighter glyph mask. It is intended to catch flat or
discoloured rectangles on shaded speech bubbles while confirming that pixels
outside the erase mask remain untouched.

## Automated checks

Run the ML and app suites with the desktop CUDA feature set:

```powershell
bun cargo test --release -p koharu-ml --features cuda --lib
bun cargo test --release -p koharu-app --features cuda --lib
```

The focused ML regressions cover:

- chaining multiple crops without changing unmasked pixels;
- matching a constant generated colour bias from the ring outside the erase
  mask, without feathering source lettering back into the result;
- following a background gradient across the erase mask instead of applying
  one constant offset to the whole block;
- rejecting boundary drawing details that previously became row/column stripes;
- matching curved shading with a smooth local residual field;
- completing partially segmented black/white glyphs without extending into
  drawing lines that cross the text-box boundary.

The app suite covers deletion and mask/image cleanup as one undoable operation,
including overlapping boxes, replacement batches, corrupted blobs and reopen.
The UI suite checks that inpainting waits for queued scene edits and that the
mask-rebuild command does not rerun text detection, OCR or translation.

Ship the result only through the full desktop build:

```powershell
bun run scripts/dev.ts tauri build --no-bundle --features cuda
```

## Real-page check

Use an isolated `KOHARU_DATA_ROOT` and a disposable copy of a project whose page
already has text nodes plus the segment and bubble masks. Verify the process
executable path and listening port; never send test mutations to the protected
STABLE app or an arbitrary port 4000 instance. Start the development app, select the copy,
then record the saved Flux2 settings and submit a job:

```powershell
$env:KOHARU_DATA_ROOT = '<workspace>/isolated-data'
target\release\KoharuFORK.exe --headless --port 4861 --debug

# PUT /api/v1/projects/current
{"id":"<disposable-project-id>"}

# GET /api/v1/config
# Record pipeline.flux2_steps and pipeline.flux2_strength.
# For a two-step comparison, first save their previous values, then:
# PATCH /api/v1/config
{"pipeline":{"flux2Steps":2,"flux2Strength":1.0}}

# POST /api/v1/pipelines
{"steps":["flux2-klein"],"pages":["<page-id>"]}

# To repair stale or partial masks using the text boxes currently kept:
{"steps":["comic-text-detector-seg","speech-bubble-segmentation","flux2-klein","koharu-renderer"],"pages":["<page-id>"]}
```

Poll `GET /api/v1/operations`, selecting the operation by the returned ID. The
array is not chronological.
The pipeline route takes Flux2 settings from saved configuration; it ignores
unknown request fields named `flux2Steps` or `flux2Strength`. Restore the saved
settings after the benchmark.

The UI equivalent is **Process → Rebuild masks and inpaint → Current image**
(or **All images**). It replaces manual segment-mask edits; pipeline operations
remain undoable. Ordinary **Inpaint** preserves the edited mask. Existing stale
masks cannot identify previously deleted boxes retrospectively, so those pages
need this explicit rebuild once.

For the 2026-09-11 regression, retained copies, raw Flux2 crops and a comparison
using identical generated pixels are under `.recovery/inpaint-109-2026-09-11/`.
The opt-in `retained_inpaint_crops` GPU test takes `KOHARU_INPAINT_QA`,
`KOHARU_FLUX_TRANSFORMER` and `KOHARU_FLUX_VAE`. It requires explicitly supplied
source/generation/composite PNG fixtures. On this machine the standalone test
can hit the known cuDNN thread-local teardown abort after writing its outputs;
record that separately and use the full app for the clean end-to-end run.

Compare the decoded source and inpainted images at full resolution. Inspect
every detected text block and verify that:

1. lettering inside a detected bubble is gone;
2. shaded and textured bubble backgrounds do not gain a flat rectangular fill;
3. artwork and bubble borders outside the erased glyph area are unchanged;
4. remaining glyphs are checked against the segment mask. The fallback protects
   blocks outside bubbles only when segmentation missed their glyphs; it does
   not prohibit erasing segmented text outside bubbles.

## Verification record

On 2026-09-07, a disposable copy of project M page `003.jpg` (3000x4000,
eleven blocks) completed on the RTX 4050 in 44.3 seconds. The effective saved
step count was not recorded, so this is not a verified two-step benchmark.
Visual inspection showed remaining glyph fragments, including part of the text
outside bubbles. The earlier claim that this entire block was retained by a
safety rule was incorrect. The comparison images were deleted during cleanup,
so the extent of the visual improvement requires a repeatable comparison.
The automated result was 79 passed and 2 ignored for `koharu-ml`,
and 104 passed for `koharu-app`; the full CUDA Tauri build also completed.
