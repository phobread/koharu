# Koharu backend handoff — 2026-09-14

## Final everyday-use handoff — September 15

The user requested removing the startup update announcement, adding a cache
button that **keeps saved projects**, and cleaning up development files.
Automatic update checks at startup and when opening Settings are disabled.
The Runtime settings pane can clear regenerable saved-project thumbnails;
the new endpoint preserves project images, scene data, translations and history,
and skips filesystem links/junctions. API schema/client were regenerated.
Orval drops three pre-existing enum imports in mock files; those imports were
restored after generation so TypeScript passes.

The final installation is `D:\apps\Koharu\KoharuFORK.exe`, with the
**Koharu - Translate (FINAL)** desktop shortcut. Consult
`.maintenance/release.json` for the verified hash and
`.maintenance/README.md` for the final state. The old STABLE installation,
backup copies and obsolete shortcuts were permanently removed. The final app
uses the existing normal app-data root and OpenRouter settings.

Build products, disposable test data, old test executables and completed
runtime installers are removed after verification. Old executable builds and
shortcuts were removed when the user designated the installed build as
definitive. Git history, recovery/test evidence and the source snapshot remain
pending separate explicit authorization. Current source and all saved user
projects, models, fonts and settings remain.

The user wants to continue in a fresh chat and conserve usage. Read AGENTS.md,
then work on one agreed backend item at a time. This note records deferred
work; historical STABLE references below predate its permanent removal.

## Remaining candidates from the upstream review

### 2. Structured translation output (implemented for OpenRouter, September 14)

The user explicitly chose **OpenRouter only** after discussing local versus
remote scope. Implemented strict JSON output with exact block-ID coverage,
OpenRouter routing requirements, and validation before applying results.
Local models and other endpoints retain their existing behavior. The nine-block
disposable M comparison passed with Opus 4.6; both old and new paths succeeded,
so a reduced real-world failure rate or better translation quality is not
claimed. 117 app and 38 LLM unit tests passed. Verification and retained
evidence: `docs/en-US/how-to/verify-openrouter-translation.md`.
No STABLE promotion is authorized.

Original proposal (scope superseded by the user's OpenRouter-only choice):
Adapt upstream's schema-constrained local translation output
to the existing fork translation pipeline. A schema describes the expected
translation structure and segment count; constrained generation reduces
malformed JSON, extra commentary and missing/extra result entries. Continue
validating the parsed result and its mapping to the original blocks.

This is a reliability improvement, not an established translation-quality,
OCR, inpainting or speed improvement. Check how it fits the models/providers
the user actually uses before expanding its scope. Preserve custom prompts,
block order, empty-block handling and existing failure behavior. Do not assume
every provider supports the same schema mechanism.

Reference: retained upstream 0.81.7 source under
`.recovery/upstream-taste-0.81.7/source/koharu-rs-koharu-517a840/`, especially
`crates/koharu-translator/src/local/mod.rs` (`inference_with_json_schema`) and
its prompt/output-schema helpers. Current fork entry point:
`crates/koharu-app/src/pipeline/engines/llm_translate.rs` and `crates/koharu-llm/`.

Acceptance: focused parsing/segment-mapping checks plus a small disposable M
translation comparison using the same model, input, prompts and generation
settings. Establish whether it helps this user's workflow before promotion.

### 5. Pin exact image-model revisions (implemented September 14)

The user clarified the image-processing focus. Pinned 17 Hugging Face
repositories / 47 artifact entries covering inpainting, detection, segmentation,
font detection and OCR, plus the Flux2 prompt-generation tool's encoder/tokenizer.
Optional local translation LLMs and OpenRouter are unchanged. The separate
versioned vendor archive for the Korean OCR verification helper is unchanged.

Twenty existing cached files (5,537,689,055 bytes) were verified against their
current repository revisions and rehashed unchanged after testing. No model
weights were replaced or downloaded. Uncached repositories were pinned from
public metadata only. The Windows blob-only cache now resolves offline without
snapshot links or changes to `refs/main`; package presence checks and actual
loading use the same pins. Missing artifacts and mismatched metadata fail
clearly, with no fallback to newer weights. Custom file paths and the generic
repository downloader retain their existing behavior.

Thirty runtime unit tests passed, as did package coverage and a live 470-byte
config download/cache-reuse check. Details, remaining scope boundaries and
deliberate upgrade instructions: `docs/en-US/how-to/verify-model-pins.md`.
Evidence and before-edit backups: `.recovery/model-pins-2026-09-14/`.
This is reproducibility/cache reliability work, not an image-quality change.
No STABLE promotion is authorized.

## Not pending implementation

- Candidate 1, PaddleOCR token repetition penalty, was tested on 35 M001–003
  blocks. It caused two word regressions and punctuation/spacing losses for
  only a 0.37-second single-run OCR difference. Keep it disabled. An opt-in
  hook exists in development source; default strength 1.0 retains the old
  greedy path. Evidence: `.recovery/ocr-penalty-2026-09-13/findings.md`.
- Candidate 3, expanded archive/PDF importing, was not worth it to the user.
- Candidate 4, the frontend presentation, has been implemented in the main
  fork. It adapts upstream's page rail, colours and inspector around the old
  backend; it does not migrate upstream's WASM canvas/scene engine.
- No wholesale upstream backend/model migration was justified by the retained
  M comparison. Current Flux2 cleanup remains the chosen path.

Other known issues are separate backlog, not newly agreed work: undo stacks
reset on closing a project; auto-render and the initiating edit have separate
history transactions. Do not turn these into an unsolicited persistence rewrite.

## Current app/build state

- Main development app: `target/release/KoharuFORK.exe`, full CUDA Tauri build
  including the frontend integration, cleaned Process menu and OpenRouter
  structured translation and image-model revision pins. SHA256:
  `81D9C405671ECDEFF2A368F8DC89D8EC138A8A9FFCF00198F0D1530CB5A8393B`.
  The full build passed; no Koharu process was running at this handoff.
  The previous development binary and source snapshots are retained under
  `.recovery/model-pins-2026-09-14/before/` (the older frontend/menu build
  remains under `.recovery/structured-translation-2026-09-14/before/`).
- Earlier, the development app was restarted on port 4000 using the user's normal
  `C:\Users\amiru\AppData\Local\Koharu` data root, and the previously open
  **10.9** project (`109`) was reopened. This is real user data, not a test
  workspace. Verify executable/port/data root afresh before interacting.
- Isolated compiled UI preview: `http://127.0.0.1:4878/`, executable and copied
  M project under `.recovery/frontend-integration-2026-09-14/`. Launcher:
  `start-ui-preview.ps1`. This preview still uses the earlier frontend/menu
  build (`252A8C54...`), without the OpenRouter change. Services/PIDs can change;
  do not trust stale IDs.
- Earlier LAB: frontend 4876, backend 4877, under
  `.recovery/frontend-prototype-2026-09-13/`. It is an older prototype.
- Process menu now has six top-level entries: current page, all pages,
  process/export all, Inpainting, Custom pipeline and OCR language. All
  actions remain available; the re-inpaint label now matches its actual
  inpaint-only handler.

## Historical STABLE state

The old STABLE installation, September 7 original, frozen recovery executable
and green shortcut were permanently deleted on September 15 at the user's
request. `D:\apps\Koharu\KoharuFORK.exe` is the definitive build.

Frontend integration: 42 UI files / 265 tests passed, TypeScript passed, full
CUDA Tauri build passed. Packaged UI editing, bold formatting, pane switching
and auto-render passed on disposable M. A fresh manual split/merge/export pass
was not completed after the user asked to conserve usage; automated coverage
passed. Menu cleanup: all 5 focused MenuBar tests, TypeScript and full CUDA
Tauri build passed; the final six-entry menu was observed in the preview.

Evidence: `docs/en-US/how-to/verify-frontend-shell.md`,
`.recovery/frontend-integration-2026-09-14/verification.json`, and
`.recovery/process-menu-2026-09-14/{notes.txt,verification.json}`. Before-edit
snapshots and development executables were later deleted with recovery evidence.

The tree contains extensive pre-existing uncommitted changes. Preserve them;
never push. Scene format is v8 and history format v3; persisted layout changes
require the migrations described in AGENTS.md. Test backend work with isolated
data roots. Runtime/node_modules junctions exist in recovery directories;
do not recursively delete them.

For desktop shipping use the full CUDA Tauri build, and verify/stop the exact
development executable before starting it. The menu build initially failed
at final rename because that executable was running; a successful retry was
completed. Only the definitive installed app remains.

Claude review was unavailable because its OAuth session expired. No Claude
approval is claimed. Avoid repeated login/review retries and broad test or
research passes when a bounded check suffices; the user is hitting usage limits.
