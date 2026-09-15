# Verify saved-project compatibility

Audit date: 2026-09-05. Current working-tree formats: scene v8, history v3.
The audit added tests and documentation; production behavior and the installed
desktop executable were not changed.

## Evidence

The CUDA-feature release build passed 104 koharu-app unit tests and one golden
fixture integration test. This includes 16 session and 15 history tests.
New coverage checks headerless v6/v7 scenes with multiple text nodes, v8 rich-text
and writing-direction save/reopen, and durable-log write failure, rollback,
poisoning and recovery. Existing tests cover scene v1-v7 upgrades, interim
scene-v7/history-v2 version collisions, future-version rejection and torn tails.

The historical-fixture test decodes synthetic v1 and v6 snapshots serialized
using the original koharu-core code, compares every scene field against paired
historical JSON, compacts to v8 and compares again after reopening. Generation
provenance is in `crates/koharu-app/tests/fixtures/README.md`.

Git comparison against original scene-model commit `0cf9ac6e` confirmed the
reused ProjectMeta/ProjectStyle, Transform, ImageData, MaskData and font/blob
layouts already existed there. Subsequent committed scene changes added the
documented rendered-text fields, not unversioned fields to those shared types.
The current uncommitted additions are the documented v7/v8 text fields.

Opus 4.8 performed a read-only review and implemented three session tests.
Codex inspected its edits, checked git history, added independent fixtures and
failure tests, and ran all validation. No definite migration defect was found
within the tested formats and fixtures.

## Normal regression checks

From the repository root:

```powershell
bun cargo test --release -p koharu-app --features cuda --lib --test project_compatibility
```

The local-project test is ignored by default because it requires explicit paths.
The release profile reused the machine's CUDA build cache during this audit.
The initial debug build stopped when flash-attn tried to download CUTLASS through
the restricted network; it was not a test failure.

## Check disposable copies of local projects

Close the app before selecting source metadata so snapshots and logs are stable.
Use an OS-separated path list (semicolon on Windows):

```powershell
$env:KOHARU_COMPAT_PROJECTS = 'D:\path\first.khrproj;D:\path\second.khrproj'
bun cargo test --release -p koharu-app --features cuda --test project_compatibility -- --ignored --nocapture
```

The test reads only `scene.bin`, `project.toml` and optional `history.log` from
each supplied directory. It opens a temporary copy, adds synthetic multilingual
styled text with a vertical-writing override, performs undo/redo, reopens without
compacting to exercise replay, compacts to v8/history-v3, and reopens again.
It compares complete scenes and epochs and checks source metadata remains
byte-identical. Temporary copies are removed automatically. Original projects
are never opened through ProjectSession or migrated in place.

All twelve copies passed:

| Set | Projects | Original page counts |
| --- | --- | --- |
| Pre-v8 recovery snapshots | 5, 8-217, badend, m | 24, 7, 20, 19 |
| Current projects | 108, 5, 8-217, badend, caitlyn, domina, la, m | 13, 24, 7, 20, 7, 4, 5, 19 |

## Limits

These checks validate metadata persistence and blob references, not image-blob
integrity, rendered appearance or GPU inference. The supplied current projects
have history-v3 logs (BadEnd and la contain frames; the others are header-only).
Legacy-log coverage comes from synthetic unit fixtures, while the copy test
also exercises new current-format frames after opening/migration. This does not
establish compatibility with every headerless
history layout ever written by an experimental build. No desktop rebuild was
needed for these test-only changes; subsequent application changes still ship
through the full CUDA Tauri build required by AGENTS.md.
