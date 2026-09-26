# AGENTS.md — Handoff & build guide for this fork

## Everyday-use installation and cleanup (2026-09-15)

The finished personal app is installed at `D:\apps\Koharu\KoharuFORK.exe`,
launched by **Koharu - Translate (FINAL)**. This is the sole retained app build.
`.maintenance/release.json` records its final hash and verification. Automatic upstream update checks are disabled;
Settings → Runtime → Clear cache removes only regenerable project thumbnails.

The user authorized development cleanup. Large build products and disposable
test environments were removed after final verification. On September 15 the
user also made the final build definitive: the old STABLE installation, frozen
recovery executable and old shortcuts were permanently deleted. The definitive
source commit and Windows executable are publicly backed up at
`https://github.com/phobread/koharu/releases/tag/definitive-2026-09-15`.
Local Git history, recovery evidence and the redundant source snapshot were then
deleted. The current source tree and normal app data/models/fonts remain.

## Definitive personal translation build (updated 2026-09-15)

The sole executable is `D:\apps\Koharu\KoharuFORK.exe`, SHA256
`FE7B5460F9D9029AD5696EE0D6CB6D9ED2A9EED29D9CF6B36EC2D981528D86E1`.
It shares the user's normal saved projects and settings. Use isolated data roots
for any future development tests, verify process executable paths before stopping
apps, and never send test mutations to an arbitrary port 4000 instance.

This is a personal fork of [Koharu](https://github.com/mayocream/koharu) (a manga
translation desktop app: Rust + Tauri backend, Next.js UI in `ui/`, local HTTP/RPC/MCP
server shared by GUI and headless modes). It is being edited for personal use on a
**Windows 11** machine with an **NVIDIA RTX 4050 Laptop GPU (6 GB, compute 8.9)**.

The GPU build environment was set up on 2026-06-18 and **re-verified end-to-end with
CUDA 13.3 on 2026-07-18**. The rest of this file is what any agent (Codex, Claude,
etc.) or human needs to keep
building and running the app with CUDA without rediscovering the gotchas.

For full reproducible install steps see
[`docs/en-US/how-to/build-with-cuda-windows.md`](docs/en-US/how-to/build-with-cuda-windows.md).
Project contribution rules are in [`CONTRIBUTING.md`](CONTRIBUTING.md) (note the AI usage policy).

---

## Critical build environment (read before building)

These are non-obvious and were the source of every build failure during setup:

1. **CUDA Toolkit must currently be ≤ 13.3.** The locked `cudarc` crate (0.19.8)
   supports CUDA through 13.3 via an exact `major.minor` match on `nvcc --version`.
   This machine runs **CUDA Toolkit 13.3 Update 1** at
   `C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.3` (`CUDA_PATH` points
   there). Do not install a newer CUDA minor until the resolved `cudarc` version lists it.
2. **cuDNN 9.x (cuda13 variant)** must live in the CUDA toolkit `bin`. cuDNN
   `9.23.2.1_cuda13` was copied into `…\CUDA\v13.3\bin` (and include/lib).
3. **LLVM / libclang** is required to build `koharu-llm` (it binds llama.cpp via
   `bindgen`). Installed at `C:\Program Files\LLVM`; `LIBCLANG_PATH=C:\Program Files\LLVM\bin`.
4. **NVCC compiler flags are required** for CUDA 13's CCCL headers + MSVC. These are set
   **permanently at user scope**, but if a build complains about the MSVC preprocessor
   or "libcu++ requires at least C++ 17", make sure they are present:
   - `NVCC_PREPEND_FLAGS=-Xcompiler=/Zc:preprocessor`
   - `NVCC_APPEND_FLAGS=-std=c++17`
5. **MSVC** (Visual Studio 2022 Community) and **Rust ≥ 1.95** / **Bun ≥ 1.0** are
   installed. `scripts/dev.ts` auto-discovers `nvcc` and `cl.exe` on Windows.
6. **`cuda` is NOT a default feature** of the `koharu` crate — you must pass
   `--features cuda` explicitly, or use `bun run build` / `bun run dev` (the default
   desktop feature path on Windows/Linux is `cuda`).

## Isolated upstream taste test (2026-09-10)

Official **0.81.7** was extracted and tested without merging or rebuilding this
fork. Artifacts and notes are retained under
`.recovery/upstream-taste-0.81.7/`; start with `comparison-notes.md` or open
`comparison.html` in a browser. The viewer compares original M001–003,
upstream RF-DETR/Hayai/LaMa exports, and fresh September 12 fork two-step Flux2
results for all three pages. The older September 7 M003 result remains selectable.
Saved fork OCR text is only a reference, not a fresh automatic OCR baseline.
The September 12 rerun uses an isolated copy of the development executable with
the same SHA256 as current STABLE; the protected installation was not tested.

- September 12 rerun: fresh segment/bubble masks 45.01 s; Flux2 M001/M002/M003
  119.22 / 127.71 / 41.58 s, total 333.52 s including masks. Verified two steps,
  strength 1.0; retained fork boxes 9/15/11, no detection/OCR/translation rerun.
  M002/M003 inspected balloons are cleaner than upstream LaMa; M001 is mixed,
  and punctuation remnants remain. See `flux2-sept12/findings.md` and
  `timings.json` under the taste-test directory. These are different processing
  paths, not an engine-only speed comparison.

- All three 3000×4000 pages completed: detection 3.59 s total, subsequent
  Hayai/LaMa 22.14 s total. These are single-run timings, not a speedup claim.
- Cleanup and OCR were mixed; no consistent quality advantage sufficient to
  justify migration was established. Upstream RORem/Flux2/PaddleOCR and
  translation/typesetting were not tested.
- The new Torch runtime initially mixed bundled cuDNN 9.20 with the system's
  `cudnn_engines_tensor_ir64_9.dll`. A process-local restricted PATH resolved
  this. Do not change the machine's CUDA installation or permanent PATH for it.
- Official test projects use the Windows Documents/Koharu location (here,
  OneDrive/Documents/Koharu), not the fork's LocalAppData project format.
  A completed test-project copy is retained under `upstream-project-after`.
- Original M metadata hashes matched at the end of the test, before the user
  resumed translation. Later user edits can legitimately change those hashes.
- The local comparison server, if needed, is
  `bun run .recovery/upstream-taste-0.81.7/serve-comparison.ts` on 127.0.0.1:4873.
  It serves only the comparison's explicit asset list.

## Build & run

```bash
bun install                       # JS deps (UI)

# Full desktop app (recommended) — produces target/release/KoharuFORK.exe
bun run build                     # = tauri build --no-bundle, with cuda
bun run dev                       # dev loop: tauri dev + fixed-port server

# Direct Rust builds (bypass Tauri wrapper); --features cuda is required
bun cargo build --release -p koharu     --features cuda
bun cargo build --release -p koharu-ml  --features cuda   # vision/OCR ML crate only
```

Run modes (`KoharuFORK.exe`): GUI (default), `--headless --port 4000`, `--cpu` (force CPU),
`--download` (prefetch runtime libs + default models then exit), `--debug` (console logs).
On first run the app extracts bundled CUDA runtime libs (llama.cpp `windows-cuda13-x64`)
to `%LOCALAPPDATA%\koharu\runtime` and downloads default vision/OCR models.

## Verify GPU works

Headless smoke test that exercises the candle CUDA path end to end:

```bash
KoharuFORK.exe --headless --port 4000 --debug
# then via the HTTP API on 127.0.0.1:4000/api/v1 :
#   POST /projects            {"name":"t"}
#   POST /pages/from-paths    {"paths":["<some image.png>"],"replace":false}
#   POST /pipelines           {"steps":["comic-text-detector"]}
#   GET  /operations          # poll until the job is "completed"
```

A completed job + log lines `GPU compute capability: 8.9` / `ggml_cuda_init: found 1 CUDA
devices` confirm GPU inference. The app shuts down cleanly on Ctrl+C.

## Known issue

The **standalone `koharu-ml` dev binaries** (e.g. `comic-text-detector.exe`,
`manga-ocr.exe`) abort at process exit with
`CudnnError(CUDNN_STATUS_INTERNAL_ERROR)` in `cudarc`'s `Cudnn::drop` —
the cuDNN handle is destroyed during thread-local teardown after the CUDA context is
already gone. **Results are written before this**, so it is cosmetic, but it produces a
nonzero exit code. The **full `KoharuFORK.exe` app is NOT affected** (verified: runs
detection on GPU and shuts down cleanly with no panic/abort). If you want to fix the
standalone wart, it lives in upstream `cudarc`/the `mayocream/candle` fork, not in this repo.

## App behavior notes (learned while debugging)

- **Export needs the layer to exist.** `Export rendered/inpainted` calls
  `POST /api/v1/projects/current/export` which returns **400 `no pages have the
  requested layer populated`** unless those pages were actually rendered/inpainted
  (run **Process → Process All** first). `.khr` and source export work on any non-empty
  project. The desktop window loads the UI from `http://127.0.0.1:<port>` (a remote
  origin in the Tauri capability), and `isTauri()` is true there.
- **Debugging the webview:** launch with env
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`, then drive CDP at
  `http://127.0.0.1:9222/json` (Bun has WebSocket built in). The release binary is
  `windows_subsystem=windows`; capture logs with `Start-Process -RedirectStandardError`.

## Saved-project compatibility audit (2026-09-05)

The working tree now uses **scene v8** (`style_ranges` added in v7,
`writing_direction` in v8) and **history log v3**. The July snapshot below is
historical: substantial rich-text/OCR/inpainting changes remain uncommitted.
This audit added tests and documentation only; it did not rebuild or replace
the desktop executable.

- Opus 4.8 reviewed persistence and added three session regression tests;
  Codex reviewed its changes and independently verified them. Claude connected
  with `--model claude-opus-4-8` and network-enabled execution. The wrapper's
  default model was not changed.
- 104 CUDA-feature app unit tests and the independent historical-fixture test
  passed. Twelve disposable copies (four pre-v8 recovery snapshots and eight
  current projects) passed migration, current text-bearing history replay,
  undo/redo, compaction and reopen; their source metadata remained byte-identical.
- Historical v1/v6 fixtures are generated from the actual old git types, not
  the current compat structs. Shared project/image/mask/transform/font/blob
  layouts were checked against the original scene-model commit `0cf9ac6e`.
- Details, limits and repeatable commands:
  [`docs/en-US/how-to/verify-project-compatibility.md`](docs/en-US/how-to/verify-project-compatibility.md).

## Rich-text and vertical-writing verification (2026-09-05)

Rich-text editing, split/merge behavior, vertical punctuation styling, and
synthetic bold/italic rendering were completed and independently reviewed by
Opus 4.8 and Luna Max. Scene v8 and history v3 remain unchanged.

- Splits use exact original translation spans, preserve repeated-word/CJK/emoji
  formatting and direction metadata, and wait for queued saves before reading
  the scene. Vertical fragments retain right-to-left column order. Merges also
  build from the latest saved scene.
- Focused rich-text drafts ignore stale save acknowledgements but accept undo,
  and toolbar selection offsets stay synchronized after replacement edits.
- Vertical punctuation normalization remaps renderer clusters to original UTF-8
  offsets. Bold/italic sprites reserve measured effect clearance and refit inside
  their text boxes instead of clipping.
- 260 UI tests, TypeScript checking, 203 Rust app/core/renderer/PSD tests and the
  full CUDA Tauri build passed. A disposable live project passed held-response
  split timing, focused undo, render, PNG/PSD/KHR export and reopen checks; its
  PNG and PSD composites were pixel-identical. The project was deleted and the
  original eight projects remained unchanged. No app process was left running.
- Details, repeatable commands and the editable-PSD limitation:
  [`docs/en-US/how-to/verify-rich-text.md`](docs/en-US/how-to/verify-rich-text.md).

## Flux2 inpainting background verification (2026-09-07)

Flux2 generates with the broad text-region mask but pastes through the tighter
glyph mask. The September 7 build used four directional colour-matching scans.
The September 11 fix replaces those scans with a robust affine trend and smooth
local residual interpolation; the old scans projected boundary drawing details
into visible grid-like streaks on project 10.9 page 13.

- Text deletion now retires the corresponding segment-mask footprint, restores
  source pixels in that part of the inpainted layer, and invalidates the rendered
  composite in one undoable transaction. Surviving boxes and BrushInpaint pixels
  are protected. Segment brush strokes inside a deleted footprint cannot be
  distinguished from detected glyph pixels; those are cleared with the block.
- Segmentation completes partially detected high-contrast glyph components
  anchored in the model mask. Ordinary inpainting keeps manual mask edits.
- Existing stale masks need **Process → Rebuild masks and inpaint** once. It uses
  the kept boxes and replaces segment-mask edits without redetecting text boxes,
  rerunning OCR, or translating. Pipeline starts wait for queued scene saves.
- Real-page evidence and repair checks are retained under
  `.recovery/inpaint-109-2026-09-11/`. The user subsequently authorized installing
  this verified build into STABLE on September 12. The original September 7
  executable is retained as the backup described above; see `stable-promotion.json`.

Historical September 7 verification:

- A disposable copy of project M, page `003.jpg` (3000x4000, eleven detected
  blocks) completed in 44.3 seconds on the RTX 4050. Correction from the timing
  audit: the effective step count was not recorded. `/pipelines` uses saved
  configuration and ignores the attempted `flux2Steps` request override.
- Remaining glyph fragments were visible; the earlier claim that an entire
  outside-bubble block was deliberately retained was incorrect. Visual quality
  still needs a repeatable comparison with retained artifacts.
- 79 passed and 2 ignored CUDA-feature `koharu-ml` tests, 104 `koharu-app` tests and
  the required full Tauri CUDA build passed. The disposable
  project was removed and the original projects were not changed.
- Details and repeatable checks:
  [`docs/en-US/how-to/verify-flux2-inpainting.md`](docs/en-US/how-to/verify-flux2-inpainting.md).

## Fork state (as of 2026-07-18)

Branch **`KoharuFORK`**, tip `0b2d93e9`; `target/release/KoharuFORK.exe` embeds
`0.61.2-123-g0b2d93e9` (current tip; no app process was left running at handoff).
**Never push — all commits stay local.** On 2026-07-18 the machine was upgraded from
CUDA 13.2 to **CUDA 13.3 Update 1 (`nvcc` V13.3.73)**; 13.2 was uninstalled and its
cuDNN-only leftover directory removed, so `v13.3` is now the sole toolkit and the
machine `CUDA_PATH`/CUDA `PATH` entries point only to it. cuDNN 9.23.2 remains in the
13.3 tree. A full Tauri build rebuilt `cudarc`, candle/flash-attn, ML/app/RPC, and the
desktop executable; 74 ML, 29 non-ignored LLM, and 90 app tests passed. A disposable
headless detector job completed on the RTX 4050 in 4.48s and logged CUDA 13.3 support,
compute capability 8.9, and one CUDA device; its project, process, and installer temp
files were removed afterward. The tracked working tree only has the corresponding
uncommitted Windows CUDA build-guide update. The final 07-17 commits improved OCR crops
for plain detector boxes (`bea29a27`) and removed PaddleOCR-VL's no-op repetition
penalty (`b07dabc9`). Both are built; 74 ML, 29 LLM, and 90 app tests pass, the
app/LLM bins check cleanly, and a headless smoke test served `/meta` from this exact
commit with the RTX 4050 CUDA backend loaded. A real-page OCR A/B remains to
live-verify the crop change and the optional Korean prompt hint.
Earlier on 07-17: op failure-atomicity
(`0a771a55`, History clone-apply-swap + log-tail self-heal — NITS "needs design"
now fully closed), canvas auto-fit on resize (`f69edfed`), flux2-klein
per-bubble tiled inpainting + `pipeline.flux2_strength`/`flux2_steps` config
(`6b8497e1`), Ctrl+W close-project (`dc5c3f82`), Settings Klein quality toggle
+ orval client regen (`2cdb49ed`). Since 07-14: a full-codebase review campaign landed 17 fixes
(`REVIEW-TRIAGE-2026-07-14.md` = verdicts+hashes; `NITS.md` = deferred items,
fixed ones struck through), then follow-ups: rotated-split geometry, sprite
object-URL leak, permissive-CORS removal (deliberate: NO CORS layer — don't
reintroduce on merges), config "[REDACTED]"-to-disk, history.log versioning
("KHLG" header mirroring scene.bin — future Op-layout changes need a frozen
compat decode in history.rs), and the whole config-write race family (backend
mutex in routes/config.rs + SettingsDialog committedConfigRef/intent-queue +
dedicated secret endpoints). The invasive failure-atomicity item is closed;
`NITS.md` now contains only the lower-risk deferred tail. Dev tooling:
`scripts/cdp/` has webview CDP smoke-test scripts
(README has usage). This fork uses fully-local inpainting only.
Remote `upstream` = mayocream/koharu, **merged through `00966bee` (2026-07-08)** — the
repo now uses the **`crates/` layout** (all Rust crates under `crates/`), has the
koharu-secrets crate, and runs harfrust 0.10 / cudarc 0.19.8 / oxfmt 0.56. candle is
still pinned 0.9.2 via the mayocream fork; this path is verified with CUDA 13.3 Update 1.
Fork-only fix in `scripts/dev.ts`: upstream's vswhere MSVC discovery assumes a VS
Installer dir that doesn't exist on this machine — the directory-walk fallback was
restored. **Keep that fallback in any future merge.**

### Hard rules (violating these corrupts saved projects)

- **Scene format is postcard (positional encoding), currently `SCENE_FORMAT_VERSION = 8`.**
  Any change to a persisted koharu-core struct requires bumping the version AND freezing
  the old layout in `crates/koharu-app/src/session.rs::mod compat`.
- **History log is currently `HISTORY_LOG_VERSION = 3`.** Persisted Op-layout
  changes also require a frozen decoder and migration in `history.rs`.
- **Never use serde `double_option` on persisted types.**
- Clearing a translation via the API must send the empty string `""`, not JSON `null`.

### Workflow rules

- **Ship ONLY via the full Tauri build:**
  `bun run scripts/dev.ts tauri build --no-bundle --features cuda`.
  `bun cargo build -p koharu` writes only `koharu.exe`, NOT the launched `KoharuFORK.exe`.
- **Stop the running app first** (`Stop-Process -Name KoharuFORK`) or the final binary
  rename fails with "Access is denied (os error 5)". Compile cache makes the rerun fast.
- After UI edits: `bun run format` (oxfmt). After Rust edits: `bun cargo fmt`.
  UI tests: `bun run test` from `ui/` (vitest) — NOT `bun test`.
- Server binds 127.0.0.1:4000, hops to 4001+ if busy. The app does **not** auto-reopen
  the last project — `PUT /api/v1/projects/current {"id":"<project>"}` after restart.
- **Any commit that changes the HTTP API shape** (routes, request/response/config
  structs) must also regenerate the client in the same commit:
  `bun run generate:api` from `ui/` (orval; regenerates `ui/openapi.json` +
  `ui/lib/api/**`), then `bun run format` (raw orval output violates style).
  Otherwise the NEXT regen picks up your stale diff (bit us at 2cdb49ed).
- Commit trailer: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

### Engine lineup (current)

- Detector `comic-text-bubble-detector` → seg `comic-text-detector-seg` (glyph-level,
  only inside detected boxes) → bubbles `speech-bubble-segmentation` (**mask is an ID
  map**: pixel value = bubble number 1..N, 0 = none).
- OCR: `paddle-ocr-vl-1.6` via `crates/koharu-llm/src/paddleocr_vl.rs` (llama.cpp GGUF)
  — this is the latest release. The candle path in `crates/koharu-ml/src/paddleocr_vl/`
  is dev-bin only, also aligned to 1.6.
- Inpainter default: **lama-manga**. **`flux2-klein` is the QUALITY pick as of
  2026-07-14**: on real pages it now produces cleaner flat bubble fills than LaMa
  with no text regeneration (an earlier "regenerates text" verdict from 07-08 no
  longer reproduces — likely fixed by the mask-fallback work). Since `6b8497e1`
  it inpaints per-bubble crops at native resolution (~40s/page at the
  user-preferred `flux2_steps=2`, ~70-105s at 4; config knob PATCHes as
  camelCase `flux2Steps`, GET returns snake_case) on this 6 GB GPU; models
  (~2.7 GB) present in `%LOCALAPPDATA%\Koharu\models`. Its prompt
  is a precomputed embedding compiled into the exe (`koharu-ml/src/flux2_klein/
  precomputed.rs`) — model re-downloads cannot affect it.
- FLUX.1 Fill (12B) was trialed (2026-07-14) and REJECTED for bubble cleanup:
  generative fill invents content (objects/text) in flat masked bubbles
  regardless of prompt/mask config. Do not revisit it. This fork does only local
  inpainting (lama-manga / flux2-klein).
- **Rendered-colour write-back** (scene v6): the renderer persists the text colour it
  actually painted into `TextData.rendered_text_color` (beside `rendered_font_size_px`),
  and the UI swatch prefers it over the black guess for auto blocks. Blocks rendered
  before v6 lack the field until re-rendered once.
- **Fallback for undetected glyphs** (`crates/koharu-ml/src/inpainting/mask.rs`): a detected
  text block whose seg mask is empty (e.g. white-on-black lettering) gets its rect
  erased clipped to the bubble covering ≥25 % of it; blocks outside any bubble are
  left alone so artwork is never erased.
- A page that was never run through the detector yields an empty seg mask and every
  inpainter silently no-ops — if inpainting "does nothing", check the page has text
  nodes first.

### API quick reference (base `http://127.0.0.1:4000/api/v1`)

- `POST /pipelines {"steps":[engine ids],"pages":[id],"sourceLanguage":...}`
- `PATCH /config {"pipeline":{"inpainter":"lama-manga"}}`
- `PUT /projects/current {"id":"badend"}`
- `GET /operations` = status only; job **warnings only appear on the SSE stream**
  `GET /events` (`jobWarning`).
- Masks/images are scene nodes: `GET /scene.json` → blob hash → `GET /blobs/{hash}`.
  `/pages/{id}/masks/{role}` is PUT (upload) only.
- `GET /meta` version = git hash at build time (`-dirty` when uncommitted).

### Known backlog

- Repeat the Flux2 background-quality comparison with retained images and
  verified configuration; inspect remaining glyph fragments.
- Troubleshoot long processing times through the detector/inpainter pathway.
- Simplify the UI by reducing visual clutter and easing the overall workflow.
- Consider flux2-klein as default inpainter and/or batch detect+inpaint the
  unprocessed BadEnd pages (7, 9, 12-15, 17, 19, 20) — user undecided.
- Auto-reopen last project on startup (friction hit repeatedly; no mechanism exists).
- When polling `GET /operations`, match the operation **by id** — the list is not
  chronological; `ops[-1]` can be a stale completed op while yours still runs.

Fuller history and per-project (BadEnd) status live in Claude's memory dir:
`C:\Users\amiru\.claude\projects\D--projects-koharuFORK\memory\fork-dev-state.md`.

## Claude coordination and worker orchestration

### Parallel Claude coordination (user instruction, 2026-09-25)

Claude may work independently in parallel on other commits, branches, or
worktrees. Keep Claude's handoff current whenever Codex makes a change.

- Read the shared coordination log before editing:
  `D:\projects\koharuFORK\.maintenance\claude-coordination.md`.
  Use this canonical absolute path from other worktrees too, so branch-local
  copies do not become separate sources of coordination state.
- Record intended scope before overlapping work; preserve other agents'
  uncommitted edits. Use separate worktrees for concurrent implementation
  when Git is available; do not switch another worker's checkout or overwrite
  its changes.
- After each coherent change, append affected files, purpose, branch/commit
  or worktree identity when available, verification actually performed,
  outstanding work, and integration/conflict notes. Update existing task
  handoffs when findings or implementation invalidate them. Preserve prior
  entries; do not overwrite the shared log with an older branch's copy.
- If Claude has a known active communication channel, send it the update as
  well. A written handoff is not proof that Claude has read it; distinguish
  recorded updates from delivered/acknowledged messages. Do not launch a new
  Claude worker just to announce a change.
- Initial OCR evidence and review prompt are under
  `exports/ocr-badend-017-2026-09-25/`; these are investigation artifacts,
  not an implemented or inference-validated fix.

### Bounded Claude worker use

Claude Code is available as a bounded local workhorse; Codex remains the
orchestrator and is responsible for scope, diff review, verification, and the
final user-facing result. When the user asks to use Claude, or a separable work
package would materially benefit from it, use the repo skill
`$claude-workhorse` and `bun run claude:worker`.

- Use `--mode analyze` for read-only investigation and `--mode implement` only
  after the user's request authorizes edits.
- The wrapper is pinned to the exact `claude-opus-5` model ID. Do not substitute
  Sonnet, Fable, or the moving `opus` alias unless the user explicitly requests
  a different model.
- Give Claude one explicit task with scope, constraints, acceptance criteria,
  and relevant checks. Never hand it an unbounded "fix everything" request.
- Inspect the pre/post diff yourself and independently run the proportionate
  tests; Claude's report is not proof.
- Preserve all pre-existing working-tree changes. Do not run concurrent Claude
  implementation workers in this shared dirty tree.
- Never invoke Claude with `--dangerously-skip-permissions`. The project wrapper
  deliberately restricts filesystem scope, shell commands, web access, MCP,
  nested agents, and git mutations.
