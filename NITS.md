# Deferred findings from the 2026-07-14 endgame review

Source: three Sol read-only review sessions (see REVIEW-TRIAGE-2026-07-14.md for
the accepted/fixed list). Each entry: location — issue — why deferred.

## Needs design, not a patch

- crates/koharu-app/src/history.rs:29 — history.log frames unversioned; a scene
  schema bump makes older frames undecodable (replay warns + skips). Needs a
  frame version header + frozen Op layout chain, symmetric to scene compat.
- crates/koharu-app/src/history.rs:81 + crates/koharu-core/src/op.rs:413 —
  op application is not failure-atomic (scene mutated before log write can
  fail; Batch stops mid-way without rollback; AddNode inserts before invariant
  check). Correct fix is clone-apply-swap or full rollback — invasive.
- crates/koharu-rpc/src/server.rs:26 — permissive CORS on the unauthenticated
  local API; any webpage can call mutation endpoints while the app runs.
  Harden by restricting to the tauri/dev origins (+ optional token). Must not
  break the desktop webview or `next dev`; also worth proposing upstream.
- Config write serialization family (one design, three symptoms):
  crates/koharu-rpc/src/routes/config.rs:42 (unserialized read-modify-write),
  ui/components/SettingsDialog.tsx:250/258 (persistConfig races; failures
  resolved as success), ui/lib/stores/serverConfigStorage.ts:139 (lifecycle
  flush can be overwritten by an older in-flight PATCH).
- ~~ui/lib/splitBlock.ts — rotated-block splits~~ FIXED e41c6297 (2026-07-14):
  half centers rotated into the original's frame; merge unions in the first
  block's de-rotated frame; split→merge round-trips at any slant. 4 regression
  tests added.

## Real but low practical risk for a single-user local fork

- crates/koharu-rpc/src/routes/pages.rs:190 — import writes blobs to the
  captured session but commits pages via app.apply (current session); switching
  projects mid-import cross-wires them.
- crates/koharu-app/src/archive.rs:112 — .khr import trusts declared entry
  sizes (OOM on hostile archive); we import our own files.
- crates/koharu-app/src/archive.rs:49 — export silently skips WalkDir errors.
- crates/koharu-app/src/app.rs:148 — close_project doesn't cancel jobs holding
  the session Arc (fs4 lock can block instant reopen).
- crates/koharu-app/src/llm.rs:155 — concurrent local-LLM loads race the shared
  state (we use a remote provider day-to-day).
- crates/koharu-app/src/pipeline/engine.rs:139 — Registry::get can double-load
  an engine under concurrent jobs (GPU OOM risk on 6GB).
- crates/koharu-app/src/config.rs:145 — "[REDACTED]" placeholder is written to
  config.toml and would be read back as a real API key if the keyring lookup
  ever fails.
- crates/koharu-app/src/blobs.rs:78 — image cache deep-clones pixel buffers and
  bounds entries, not bytes (memory pressure on big batches).
- crates/koharu-app/src/renderer.rs:263 — per-block render failures only
  tracing-warn; job reports success with text missing (route to jobWarning).
- crates/koharu-app/src/pipeline/engines/comic_text_bubble.rs:92 — loader
  reports success before its worker thread actually loaded the model.
- crates/koharu-app/src/pipeline/engines/flux2_klein.rs:113 — force-CPU flag
  ignored (flux on CPU is impractical anyway).
- crates/koharu-app/src/ai.rs:141 — completed Codex login attempts never pruned.
- crates/koharu-core/src/op.rs:279 — AddPage skips validate_page_invariants
  (reachable via /history/apply only).
- crates/koharu-core/src/op.rs:577 — reorder validation accepts duplicate IDs.
- crates/koharu-rpc/src/binary.rs:44 — scene clone and epoch read not under one
  lock (stale scene labeled with newer epoch).
- crates/koharu-rpc/src/binary.rs:151 — thumbnail generation runs synchronous
  image work on async workers (move to spawn_blocking).
- crates/koharu-rpc/src/psd_export.rs:247 — PSD export ignores
  layer_transform.rotation_deg (misaligned rotated sprites in PSD).
- crates/koharu-rpc/src/mcp/mod.rs:205 — MCP pipeline jobs never registered in
  the operations registry; errors swallowed (we don't use the MCP surface).
- crates/koharu-llm/src/providers/deepl.rs:67 — unsupported DeepL targets are
  silently mapped to EN-US/ZH (we don't use DeepL).
- crates/koharu-llm/src/lib.rs:335 — Gemma4_12bIt missing from the Gemma
  generation-defaults arm (we don't run local Gemma).
- crates/koharu/src/app.rs:136 — GUI bootstrap failure panics instead of
  surfacing to the readiness path.
- scripts/dev.ts:43 — CUDA auto-discovery would pick an unsupported 13.3 if one
  is ever installed next to 13.2 (only 13.2 present today).
- ui/components/panels/RenderControlsPanel.tsx:546 — slow font download can
  commit an obsolete selection; failures applied silently.
- ~~ui/hooks/useBlobData.ts — sprite object URLs never revoked~~ FIXED bfe05974
  (2026-07-14): query-cache subscription revokes blobImage URLs on eviction.
