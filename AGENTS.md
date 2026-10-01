# AGENTS.md — Handoff & build guide for this fork

This is a personal fork of [Koharu](https://github.com/mayocream/koharu) (a manga
translation desktop app: Rust + Tauri backend, Next.js UI in `ui/`, local HTTP/RPC/MCP
server shared by GUI and headless modes). It is edited for personal use on a
**Windows 11** machine with an **NVIDIA RTX 4050 Laptop GPU (6 GB, compute 8.9)**.
Project contribution rules are in [`CONTRIBUTING.md`](CONTRIBUTING.md) (note the AI
usage policy). Open work is in [`TODO.md`](TODO.md).

## Installed app

- The everyday app is `D:\apps\Koharu\KoharuFORK.exe`, launched by the
  **Koharu - Translate (FINAL)** desktop shortcut. It is the only retained build.
  `.maintenance/release.json` (local, gitignored) records the installed hash, source
  commit and verification; update it whenever the installed exe changes.
- It uses the owner's real data under `%LOCALAPPDATA%\koharu` (projects, config, fonts,
  models, runtime). **Use an isolated data root (`KOHARU_DATA_ROOT`) for every
  development test**, verify process executable paths before stopping apps, and never
  send test mutations to an arbitrary port-4000 instance.
- Automatic upstream update checks are disabled. Settings → Runtime → Clear cache
  removes only regenerable thumbnails; Free up space removes unreferenced project images.
- Branch **`KoharuFORK`** on `origin` (github.com/phobread/koharu). `main` tracks
  upstream, not this build. Upstream 0.83+ is a near-total rewrite, so port ideas by
  hand; do not merge. Push only when the owner asks; the owner runs git
  add/commit/push themselves.
- Public backup of the 2026-09-15 build:
  `https://github.com/phobread/koharu/releases/tag/definitive-2026-09-15`.

---

## Critical build environment (read before building)

These are non-obvious and were the source of every build failure during setup
(full install steps: [`docs/en-US/how-to/build-with-cuda-windows.md`](docs/en-US/how-to/build-with-cuda-windows.md)):

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
   installed. `scripts/dev.ts` auto-discovers `nvcc` and `cl.exe` on Windows. Its
   directory-walk fallback for MSVC is fork-only (upstream's vswhere path assumes a VS
   Installer dir that doesn't exist here) — keep it when porting upstream changes.
6. **`cuda` is NOT a default feature** of the `koharu` crate — you must pass
   `--features cuda` explicitly, or use `bun run build` / `bun run dev` (the default
   desktop feature path on Windows/Linux is `cuda`).

## Build & run

```bash
bun install                       # JS deps; REQUIRED before any tauri build (tauri CLI is a devDependency)

# Full desktop app — produces target/release/KoharuFORK.exe
bun run build                     # = tauri build --no-bundle, with cuda
bun run dev                       # dev loop: tauri dev + fixed-port server

# Direct Rust builds (bypass Tauri wrapper); --features cuda is required
bun cargo build --release -p koharu     --features cuda
bun cargo build --release -p koharu-ml  --features cuda   # vision/OCR ML crate only
```

Build gotchas:

- **Ship only via the full Tauri build** (`bun run build`, i.e.
  `bun run scripts/dev.ts tauri build --no-bundle --features cuda`).
  `bun cargo build -p koharu` writes only `koharu.exe`, not the launched `KoharuFORK.exe`.
- **Stop the running app first** if it runs from `target/` or the final binary rename
  fails with "Access is denied (os error 5)". The compile cache makes the rerun fast.
  `target/` was deleted on 2026-10-01, so the next build is a full rebuild.
- Agent shells may set `NoDefaultCurrentDirectoryInExePath=1`, which breaks vswhere
  discovery in `scripts/dev.ts`: clear it for the build
  (`env -u NoDefaultCurrentDirectoryInExePath bun run build`).
- While the owner is using the PC, build at below-normal priority with
  `CARGO_BUILD_JOBS=10`.
- Keep scratch target/store directories on short paths (MSBuild breaks past 260 chars).

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

- **Export needs the layer to exist.** `POST /api/v1/projects/current/export` returns
  **400 `no pages have the requested layer populated`** unless those pages were actually
  rendered/inpainted. The UI's "Export all pages" falls back per page from rendered to
  inpainted to source. `.khr` and source export work on any non-empty project. The
  desktop window loads the UI from `http://127.0.0.1:<port>` (a remote origin in the
  Tauri capability), and `isTauri()` is true there.
- **Debugging the webview:** launch with env
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`, then drive CDP at
  `http://127.0.0.1:9222/json` (Bun has WebSocket built in). `scripts/cdp/` has smoke-test
  scripts (README has usage). The release binary is `windows_subsystem=windows`; capture
  logs with `Start-Process -RedirectStandardError`.
- The app does **not** auto-reopen the last project after a restart:
  `PUT /api/v1/projects/current {"id":"<project>"}`.

## Hard rules (violating these corrupts saved projects)

- **Scene format is postcard (positional encoding).** `SCENE_FORMAT_VERSION = 9`
  (`crates/koharu-app/src/session.rs`), written only when a scene has official-release
  images; other scenes are still written as v8 so older builds can open them. Any
  change to a persisted koharu-core struct requires bumping the version AND freezing
  the old layout in `session.rs::mod compat`.
- **History log is `HISTORY_LOG_VERSION = 3`** (`history.rs`, "KHLG" header).
  Persisted Op-layout changes also require a frozen decoder and migration there.
- **Never use serde `double_option` on persisted types.**
- Clearing a translation via the API must send the empty string `""`, not JSON `null`.
- **No CORS layer** on the local API (removed deliberately; cross-origin and
  DNS-rebinding requests are rejected). Don't reintroduce one.

Persistence checks and fixtures:
[`docs/en-US/how-to/verify-project-compatibility.md`](docs/en-US/how-to/verify-project-compatibility.md).

## Workflow rules

- After UI edits: `bun run format` (oxfmt) **from the repo root** (it fails with
  "Script not found" inside `ui/`). After Rust edits: `bun cargo fmt`.
  UI tests: `bun run test` from `ui/` (vitest) — NOT `bun test`.
- Server binds 127.0.0.1:4000, hops to 4001+ if busy — API debugging must scan ports.
- **Any commit that changes the HTTP API shape** (routes, request/response/config
  structs) must also regenerate the client in the same commit:
  `bun run generate:api` from `ui/` (orval; regenerates `ui/openapi.json` +
  `ui/lib/api/**`), then `bun run format`. Orval drops the
  `CodexAuthAttemptStatus`/`GradientDirection`/`TextAlign` value imports in
  `ui/lib/api/default/default.{msw,faker}.ts` — re-add them by hand.
- Commit trailer: a `Co-Authored-By:` line naming the model that wrote the change
  (e.g. `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`).

## Engine lineup (current)

- Detector `comic-text-bubble-detector` → seg `comic-text-detector-seg` (glyph-level,
  only inside detected boxes) → bubbles `speech-bubble-segmentation` (**mask is an ID
  map**: pixel value = bubble number 1..N, 0 = none).
- OCR: `paddle-ocr-vl-1.6` via `crates/koharu-llm/src/paddleocr_vl.rs` (llama.cpp GGUF),
  with a PP-OCRv5 line checker/repair and black-on-white redraw of thick-outlined
  lettering before both readers. The candle path in `crates/koharu-ml/src/paddleocr_vl/`
  is dev-bin only.
- Inpainter default: **lama-manga**; **`flux2-klein`** is the quality pick. Flux2
  inpaints per-bubble crops (large crops at half size by default: Settings → Engines →
  "Faster cleanup of large areas"), plain bubbles and plain panels get a flat fill, and
  its prompt is a precomputed embedding compiled into the exe
  (`koharu-ml/src/flux2_klein/precomputed.rs`). The repair brush can use LaMa
  ("Repair with LaMa"). FLUX.1 Fill (12B) was rejected for bubble cleanup (it invents
  content); this fork does only local inpainting.
- Renderer: unlocked boxes inside a bubble use bubble-shaped lettering when it gives
  bigger text than the box. A page with an official English release keeps the
  release everywhere except the owner's boxes (`crates/koharu-app/src/official.rs`).
- **Fallback for undetected glyphs** (`crates/koharu-ml/src/inpainting/mask.rs`): a detected
  text block whose seg mask is empty (e.g. white-on-black lettering) gets its rect
  erased clipped to the bubble covering ≥25 % of it; blocks outside any bubble are
  left alone so artwork is never erased.
- A page that was never run through the detector yields an empty seg mask and every
  inpainter silently no-ops — if inpainting "does nothing", check the page has text
  nodes first.

Verification write-ups: `docs/en-US/how-to/verify-*.md` (Flux2 inpainting, rich text,
project compatibility, model pins, OpenRouter translation, frontend shell).

### API quick reference (base `http://127.0.0.1:4000/api/v1`)

- `POST /pipelines {"steps":[engine ids],"pages":[id],"sourceLanguage":...}`
- `PATCH /config {"pipeline":{"inpainter":"lama-manga"}}` (PATCH takes camelCase keys,
  GET returns snake_case)
- `PUT /projects/current {"id":"badend"}`
- `GET /operations` = status only; match the operation **by id** (the list is not
  chronological). Job **warnings only appear on the SSE stream** `GET /events`
  (`jobWarning`).
- Masks/images are scene nodes: `GET /scene.json` → blob hash → `GET /blobs/{hash}`.
  `/pages/{id}/masks/{role}` is PUT (upload) only.
- `GET /meta` version = git hash at build time (`-dirty` when uncommitted).

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
