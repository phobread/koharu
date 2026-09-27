# Cowork review brief

## For me (before starting Cowork)

1. Open Claude Cowork on `github.com/phobread/koharu`.
2. Make sure it uses the **`KoharuFORK`** branch. `main` is the repo's default
   branch but it tracks upstream Koharu, not my build; the last Cowork review
   worked on `main` and its fixes had to be ported by hand.
3. First message: *"Read COWORK-REVIEW.md on the KoharuFORK branch and follow
   it."*

## For Cowork

### What this is

A personal fork of [Koharu](https://github.com/mayocream/koharu), a manga
translation desktop app: Rust crates under `crates/` (Tauri app, axum HTTP/MCP
server, candle ML, llama.cpp bindings) and a Next.js UI under `ui/`. It runs on
one Windows 11 laptop with an RTX 4050 (6 GB VRAM). Everything is local except
translation, which goes through OpenRouter.

The `KoharuFORK` branch (commit `d69a0821` or later) is exactly the source of
the build I use every day. Recent work, newest first:

- `d69a0821` perf: Flux2 never shares the GPU with the detection/OCR models,
  BF16 tensor-core Flux2 linears, flat fill for plain single-colour bubbles
- `d5be5966` QoL port: startup Retry screen, Settings > Privacy tab, MCP
  pipelines through the REST launch path, typed job cancellation
- `f8123bda`, `132f8b11` Korean OCR line splitting and repair
- `4e137799`, `a6b77674` security: pinned SHA-256 for runtime downloads, local
  API rejects cross-origin and DNS-rebinding requests

### What I want

Review the codebase and tell me what else I should change: bugs, risks, dead
or duplicated code, simplifications, missing tests. Also include ideas for the
**UI refresh** in `TODO.md`: the interface has too many buttons, dials and
dropdown menus, and I want the everyday translate-and-touch-up workflow to be
simpler.

Deliver:

1. A report, `REVIEW-<date>.md`, with findings ranked by impact and effort,
   each with file and line references and a one-line suggested fix.
2. Any code changes on a **new branch off `KoharuFORK`** (for example
   `cowork/review-<date>`), one commit per change. Never commit to `main` or
   `KoharuFORK`, and don't open pull requests against upstream.

### Things you can't check from the cloud

You can't build with CUDA, run the models, or open my projects, so you can't
measure speed or output quality. Mark any claim about performance or
inpainting/OCR quality as unverified; I test those locally.

### Rules that protect my saved projects and builds

- Scene files are postcard-encoded (`SCENE_FORMAT_VERSION = 8`) and the history
  log is versioned (`HISTORY_LOG_VERSION = 3`). Any change to a persisted struct
  needs a version bump plus a frozen decoder for the old layout
  (`crates/koharu-app/src/session.rs` `mod compat`, `history.rs`). Never use
  serde `double_option` on persisted types.
- No CORS layer on the local API. That is deliberate; see
  `crates/koharu-rpc/src/server.rs` and `guard.rs`.
- Any change to the HTTP API shape must regenerate the UI client in the same
  commit (`bun run generate:api` in `ui/`, then `bun run format`). Orval 8.19
  drops the `CodexAuthAttemptStatus`, `GradientDirection` and `TextAlign` value
  imports from `ui/lib/api/default/default.msw.ts` and `default.faker.ts`;
  re-add them by hand.
- Format with `cargo fmt` and `bun run format`. UI tests run with `bun run test`
  from `ui/` (vitest), not `bun test`.
- Inpainting stays local (LaMa or Flux.2 Klein). FLUX.1 Fill and remote
  inpainting were tried and rejected.

### Already decided, don't re-propose

- Hayai OCR was evaluated and dropped; PaddleOCR-VL 1.6 plus the PP-OCRv5
  Korean verifier stays.
- BF16 Flux2 linears stay on: a blind review on 23 pages found no quality
  difference on balance, and they're 18% faster.
- Flat fill stays on by default and has a switch in Settings > Engines.
- Leftover text the detector never boxes (text over artwork, hand-lettered
  SFX) is fixed by hand. The fix for half-removed text in plain bubbles is
  already in `TODO.md`.

### Docs that are out of date

`AGENTS.md` is partly stale: it still says "never push", and lists old build
hashes and CUDA details. `CLAUDE.md` describes a Codex-orchestrated local setup
that doesn't apply to you. For the current state, trust `TODO.md`, this file
and `git log`.
