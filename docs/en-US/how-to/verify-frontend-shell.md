# Frontend shell integration (2026-09-14)

The approved 0.81.7 presentation prototype is integrated into the fork UI:
compact page rows, upstream surface colours, a rounded canvas workspace and
one full-height inspector with Text, Properties and Layers tabs. The existing
AI tab remains available when signed in. Radix tabs provide keyboard navigation;
inactive editing panes stay mounted to retain local drafts. The toolbar wraps
at narrow canvas widths, and the new layout uses a separate saved-layout ID.

This adapts the presentation around the existing scene API and editor. It does
not port upstream's WASM canvas, command bridge, agent pane or backend changes.
Existing rich-text, split/merge, vertical-writing and inpainting logic remains
in place. Normal application providers, fonts and desktop packaging remain in
use; prototype-only telemetry/model-loading overrides and its data-root gate
are not part of the main application.

## Isolation and rollback

The personal STABLE executable and green shortcut must remain protected as
described in AGENTS.md. This integration targets the development fork only.

Before-edit copies of the affected UI files, the complete pre-existing working
tree diff, and the previous development executable are retained in
`.recovery/frontend-integration-2026-09-14/`. Preserve the user's other dirty
changes when reviewing or reverting this integration.

The test launcher `start-ui-preview.ps1` in that directory uses only its local
`Koharu-UI-Preview.exe`, port 4878 and disposable `data/projects/m.khrproj`.
The test data includes a runtime junction; do not recursively delete junctions.
The earlier frontend-only LAB remains on ports 4876/4877 for comparison.

## Checks

```powershell
bun run format
bun run tsc --noEmit --incremental false --project ui/tsconfig.json
# From ui/:
bun run test
# From the repository root, with the development executable stopped:
bun run scripts/dev.ts tauri build --no-bundle --features cuda
```

The UI suite passed: 42 files / 265 tests, including new draft-preservation,
keyboard-navigation and AI sign-out coverage in `Panels.test.tsx`.
TypeScript and whitespace checks passed. Google Fonts require network access
during the production UI build. Claude review was attempted but unavailable
because its OAuth session had expired; no Claude review is claimed.

Use a disposable project for live acceptance: select M003, edit and format a
translation, switch panes, split and merge, render, export, close/reopen and
undo/redo within a session. Closing a project clears the existing undo stack;
document changes themselves persist. Automatic rendering has a separate
history transaction from the initiating text edit.

Build hashes and live verification evidence are retained with the isolated
preview in `.recovery/frontend-integration-2026-09-14/`.

The full CUDA Tauri build passed. Its packaged frontend was opened on the
isolated M copy; translation editing, bold formatting, inspector switching
and automatic sprite rendering passed. The user requested wrapping up to
conserve usage, so a fresh live split/merge/export pass was not completed;
existing automated coverage passed. STABLE was not replaced.
