# Endgame review triage — 2026-07-14

Three GPT-5.6 Sol read-only review sessions (high reasoning) swept the codebase:
A = koharu-ml + koharu-app, B = core/rpc/renderer/llm/scripts, C = ui/.
51 findings total. Full transcripts in the session scratchpad
(`sol-findings-{A,B,C}.txt`); this file is the durable triage verdict.
All three sessions confirmed CLEAN on fork invariants: no postcard/double_option
violations, no remote-inpaint residue, no stale pre-crates/ paths, and the
UndetectedBlockFallback::Skip wiring is correct.

Legend: [SOL] = fix delegated to Sol per-bug; [ME] = orchestrator fixes directly
(frontend/docs); deferred items live in NITS.md.

## ACCEPTED — Rust wave (sequential Sol runs, one branch+spec each)

R1 [SOL] crates/koharu-app/src/session.rs — compact() lock-order inversion
    (scene→history vs history→scene everywhere else; ABBA deadlock) AND
    lost-edit race (locks dropped between snapshot clone and truncate_log; a
    frame appended in that window is truncated but not in the snapshot).
    VERIFIED in code. Fix: acquire history first, clone scene under it, HOLD the
    history lock through truncate_log.
R2 [SOL] crates/koharu-ml/src/inpainting/strategy.rs:155 — run_crop with
    caller-supplied windows returns early; expanded-mask ink outside every
    window (e.g. repair-brush strokes outside text blocks) silently skipped
    under Crop strategy. VERIFIED. Fix: after window pass, process remaining
    working_mask contours via boxes_from_mask.
R3 [SOL] crates/koharu-ml/src/flux2_klein/mod.rs:318 — empty mask falls through
    to full-frame generation (known "garbled glyphs page" footgun). Fix: return
    input clone when binarized mask has no nonzero pixels.
R4 [SOL] crates/koharu-rpc/src/routes/pages.rs:105 — replace-pages clears the
    project before decoding uploads; one corrupt file = empty project; undo
    split across two entries. Fix: decode all first, one atomic batch.
R5 [SOL] crates/koharu-rpc/src/bootstrap.rs:62 — `while let Ok` on broadcast recv
    dies permanently on Lagged; downloads UI/SSE silently stops. Fix: continue
    on Lagged, exit on Closed only.
R6 [SOL] crates/koharu-app/src/pipeline/engines/renderer.rs:54 — emptied
    translations keep stale sprite/spriteTransform/rendered fields on the node
    (ghost sprites in PSD export; interacts with un-inpaint clearing
    translations). Fix: emit clearing patches for skipped nodes.

## ACCEPTED — UI wave [ME], one branch, vitest after

U1 ui/lib/io/saveBlob.ts:41 — zip-slip: entries with `..`/absolute paths escape
    the chosen export dir. VERIFIED. Reject non-contained paths.
U2 ui/lib/events.ts:91 — clean SSE close never reconnects (onclose returning
    normally resolves fetch-event-source; comment claims otherwise). VERIFIED.
    Throw retryable from onclose.
U3 ui/hooks/useRenderBrushDrawing.ts:46 — full-canvas brush PUTs not serialized;
    later stroke can be overwritten by an earlier slow PUT. Serialize like the
    repair-brush queue.
U4 ui/hooks/useMaskDrawing.ts:61 — detached bitmap decode has no page/generation
    guard; page switch mid-decode paints old mask onto new page (can then be
    uploaded). Generation token + discard stale.
U5 ui/lib/io/scene.ts:105 — auto-render debounce keeps ONE pending pageId;
    editing then switching pages within 500ms drops page A's render. Per-page
    debounce.
U6 ui/components/canvas/CanvasToolbar.tsx:106 — manual Render omits
    renderDefaultsForPipeline() (fontSize/boxPadding/shader), diverging from
    auto-render. Spread the shared defaults.
U7 ui/lib/io/scene.ts:152 — project switch/replace-import leaves
    selectionStore.pageId pointing into the old scene. Clear/reconcile on
    project transitions.
U8 ui/components/ui/font-select.tsx:43 — google-font load state never resets on
    family/source change (falsely ready / stuck loading). Reset + associate
    completion with requesting family.
U9 ui/components/ui/color-picker.tsx:113 — pointerup outside the picker loses
    the commit and leaves dragging=true (blocks external updates). Pointer
    capture / window-level up+cancel.

## ACCEPTED — docs [ME]

D1 crates/koharu-core/src/protocol.rs:4 — doc comment path pre-dates crates/
    move.

## DEFERRED

Everything else → NITS.md with per-item reasons. Notables deliberately NOT
fixed now: history.log frame versioning + op-application failure-atomicity
(both need real design, not a patch); permissive CORS on the local API
(hardening worth doing carefully — must not break tauri/dev origins; also an
upstream conversation); rotated-block split geometry (real workflow bug, needs
proper local-frame math + tests — first candidate for a future wave);
SettingsDialog/serverConfigStorage/config.rs write-serialization family (one
coherent design across UI+backend, not three point patches).

## STATUS (update each line as work lands; hashes only after they exist)

- [x] Triage written (88e1ab8c)
- [x] UI wave U1-U9 (bf8df631 U1-U8 orchestrator-applied pre-pivot; 98d64d12 U9
      via Sol; 216 ui tests pass)
- [x] D1 (58e5ea4e, Sol)
- [x] R1 (1e48923a, Sol — compact lock order + truncation race; 76 app tests)
- [x] R2 (0b17f9fa, Sol — residual pass after crop windows; +orchestrator
      regression test; 59 ml tests)
- [x] R3 (37c14f24, Sol — flux empty-mask no-op)
- [x] R4 (b175aa0f, Sol — decode-before-mutate, single replace batch)
- [x] R5 (78bad730, Sol — download forwarder survives Lagged)
- [x] R6 (94a6ba3a, Sol — renderer clears write-backs on emptied translations)
- [ ] Full tauri build + ship
