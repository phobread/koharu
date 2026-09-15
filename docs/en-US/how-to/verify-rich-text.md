# Rich-text and vertical-writing verification

The 2026-09-05 follow-up keeps scene format v8 and history log v3 unchanged.

## Changes covered

- Splits carry exact source translation spans, so formatting stays on the
  correct occurrence in repeated text (`go go`, `猫🙂猫🙂`). Interior whitespace
  is retained, and automatic splits do not cut UTF-16 surrogate pairs.
- The second split block is inserted immediately after the first in page order.
  Merging clips formatting to each trimmed fragment before joining it.
- Split/merge commands wait for earlier saves and build from a fresh scene,
  preventing an older component snapshot from overwriting recent typing.
  Canvas vertical splits put the first fragment on the right, and both halves
  retain the available writing-direction metadata.
- The rich-text editor retains newer local text and formatting while earlier
  queued saves are acknowledged, including after focus moves to the toolbar.
  Unrelated external changes such as undo replace that local formatting state.
  Focused undo also updates the displayed draft; replacing a selection records
  the collapsed caret before the formatting toolbar can be used again.
- Vertical emphasis pairs (`！！`, `!!`, `！？`) can change UTF-8 length when
  normalized into one glyph. Layout now maps clusters and line ranges back to
  original text offsets before rendering character styles. A combined pair uses
  its first source character's style, like a font ligature.
- Synthetic bold/italic gets measured canvas clearance. The app refits the text
  with that clearance so the sprite stays within the box. A regression comparing
  tight and generously padded italic rendering reproduced roughly 20% ink loss
  before the fix; it covers horizontal/vertical and character-level effects.

## Automated checks

From the repository root:

```powershell
bun run format
bun cargo fmt
bun cargo test --release -p koharu-app -p koharu-renderer -p koharu-core -p koharu-psd --features cuda --lib
```

From `ui/`:

```powershell
bun run test
bun x tsc --noEmit
```

Focused UI coverage lives in `tests/lib/io/splitNode.test.ts`,
`tests/lib/splitBlock.test.ts`, `tests/lib/richText.test.ts`, and
`tests/components/RichTextDraftTextarea.test.tsx`.

Ship through the full desktop build, with the app stopped:

```powershell
bun run scripts/dev.ts tauri build --no-bundle --features cuda
```

The UI build downloads its configured Google Fonts and requires network access.

## Recorded result (2026-09-05)

260 UI tests, TypeScript checking, and 203 Rust tests across app/core/renderer/PSD
passed. The expanded italic pixel regression also passed independently. The
full Tauri CUDA build completed and replaced `target/release/KoharuFORK.exe`.

A disposable project passed split/merge batch undo/redo, vertical rendering,
sprite bounds, PNG/PSD/KHR export, and close/reopen through the live API. Its
600 × 600 PNG and PSD composites were pixel-identical. Visual inspection and
pixel counts confirmed red `です` after `！！` and red `H` after `!!`, with the
preceding glyphs remaining black. In the actual webview editor, selecting `です`,
toggling bold off, and appending `猫🙂` preserved the red/italic UTF-8 range
`12..18` and the complete translation in the backend. Startup loaded the RTX
4050 CUDA backend; these text-rendering checks themselves use CPU rasterization.

## Live acceptance checks

Use a disposable project with a blank page; do not modify existing projects.

1. Enter `go go`, format only the second word, split between words, then merge.
   Check page order, formatting, and undo/redo of each operation.
2. Repeat with mixed CJK, Korean, and emoji. Type several changes quickly and
   use the formatting toolbar while saves are still pending.
3. Set vertical writing and render `はい！！です` with only `です` styled, and
   `!!H` with only `H` styled. Both trailing runs must retain their colors and
   effects. Confirm padded sprites stay within their unrotated text boxes.
4. Export rendered PNG, PSD, and KHR. Check the PNG and PSD composite visually;
   close and reopen the disposable project to verify text, ranges and direction.
5. Delete only the disposable project when finished.

## Export limits

Rendered PNG and PSD raster sprites/composites carry character formatting.
The current PSD editable-text metadata still describes block-level styling;
it does not encode the app's character-level style ranges. Editing a text layer
in Photoshop may therefore change its rich formatting. Photoshop editing and
editable-text rotation are not validated by the raster/export checks above.

Tiny boxes that cannot fit even a 1 px font retain the existing least-overflow
fallback. This follow-up does not change that policy.
