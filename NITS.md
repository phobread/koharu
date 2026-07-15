# Deferred findings from the 2026-07-14 endgame review

Source: three Sol read-only review sessions (see REVIEW-TRIAGE-2026-07-14.md for
the accepted/fixed list). Each entry: location — issue — why deferred.

## Needs design, not a patch

- ~~crates/koharu-app/src/history.rs:29 — history.log frames unversioned~~ FIXED
  6c52152d (2026-07-16): "KHLG" + u16 version header mirroring scene.bin;
  headerless logs = legacy v0; replay dispatches by version and rejects
  newer-than-known; future Op changes add a frozen compat decode at the seam.
  5 tests. (The op non-failure-atomicity item just below is separate + still open.)
- crates/koharu-app/src/history.rs:81 + crates/koharu-core/src/op.rs:413 —
  op application is not failure-atomic (scene mutated before log write can
  fail; Batch stops mid-way without rollback; AddNode inserts before invariant
  check). Correct fix is clone-apply-swap or full rollback — invasive.
- ~~crates/koharu-rpc/src/server.rs — permissive CORS~~ FIXED e7b4b626
  (2026-07-15): CORS layer removed entirely — every legitimate client is
  same-origin (Tauri serves the UI; next dev proxies /api/v1) or non-browser.
  Residual: "simple request"-shaped calls can still fire blind cross-origin;
  closing that needs a session token (still deferred, upstream-worthy).
- Config write serialization family — mostly CLOSED; only slice 4 remains.
  Fixed:
  - ~~config.rs:42 unserialized read-modify-write~~ FIXED 3e0d1689 (slice 1):
    BootstrapManager async mutex serializes all three config write handlers
    (the ROOT — both write paths raced through here).
  - ~~API keys round-tripped the whole provider list~~ FIXED b2871585 (slice 2):
    onSaveKey/onClearKey use the dedicated setProviderSecret/clearProviderSecret
    endpoints + best-effort refresh; can't revert a concurrent base_url/other
    provider change anymore.
  - ~~serverConfigStorage keepalive edge~~ FIXED 978524be (slice 3):
    visibilitychange(hidden) uses the chained flush (keeps dirty on failure);
    keepalive reserved for real exit; no optimistic dirty clear.
  - ~~persistConfig failures resolved as success → erased a typed key~~ FIXED
    c172de28 (inline per-provider error, clear only on success).
  STILL OPEN — slice 4 (the actual frontend "redo", deferred by choice):
  persistConfig (engine-select, base_url blur, storage apply) still builds full
  payloads from the appConfig snapshot with no request ordering, and
  setAppConfig(saved) re-runs the SettingsDialog effect that resets Storage-pane
  drafts (an unrelated save can wipe unsaved Storage input). Design: committed-
  config ref separate from drafts + explicit-intent serial queue; reconcile only
  saved fields. See the slice-4 notes; Sol design-reviewed the approach.
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
- ~~crates/koharu-app/src/config.rs — "[REDACTED]" placeholder written to disk~~
  FIXED b82fe9f8 (2026-07-15): save() serializes a clone with provider api_keys
  cleared (config_for_disk); +regression test. Secrets live only in the keyring.
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
