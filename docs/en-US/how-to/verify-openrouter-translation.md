# OpenRouter structured translation — 2026-09-14

The user selected OpenRouter-only implementation after reviewing
`BACKEND-HANDOFF.md`. Local models and other provider endpoints retain the
tagged-text path. No persisted scene/history types or HTTP API shapes changed.

## Behavior

The existing OpenAI-compatible provider opts into structured translation only
for the HTTPS `openrouter.ai/api/v1` endpoint. Source blocks are sent in an
ordered JSON array, with one-based string IDs. The response schema requires
exactly those IDs as string-valued fields in a `translations` object. This
avoids depending on provider support for array-length or numeric constraints.

Requests use `response_format.type=json_schema`, `strict=true`, and OpenRouter
`provider.require_parameters=true`. This follows the
[OpenRouter structured-output documentation](https://openrouter.ai/docs/guides/features/structured-outputs).
The public endpoint listing for `anthropic/claude-opus-4.6` advertised
structured-output support on several providers when checked on September 14.
Support is endpoint-dependent and may change. Unsupported requests surface the
existing provider error; the app does not silently retry with loose output.

Custom translation guidance, target language and generation settings are kept.
Only input/output formatting instructions change. The parser checks exact ID
coverage, duplicate IDs, field types and unknown fields. Truncation, refusal
and abnormal completion statuses fail before any translation ops are emitted
for that request. JSON-escaped quotation marks and newlines are preserved as
content. Empty OCR blocks remain excluded by the existing pipeline filter.

## Automated checks

```powershell
bun cargo fmt
bun cargo test --release -p koharu-llm -p koharu-app --features koharu-app/cuda --lib
bun run scripts/dev.ts tauri build --no-bundle --features cuda
```

App unit tests: 117 passed, one opt-in live comparison ignored.
LLM unit tests: 38 passed, ten native-runtime-dependent tests ignored.
Full CUDA Tauri build and its UI TypeScript check passed. The development
executable SHA256 is
`278F0FBAF043A51456EC8B4D63070DF240442A7368C02D5F4BC414BCA6250AAA`.
No app process was left running. STABLE's SHA256 still matches
`3DF7B456D4DAA76CF177FDF75E4CCFEE20469E16315040D264800B06A616E483`.
Coverage includes numeric mapping beyond nine blocks, input order, duplicate /
missing / extra IDs, malformed JSON, empty blocks, custom prompts, literal
quotes/newlines, truncation/refusal handling, host scoping, legacy payloads,
and the actual HTTP schema/routing/generation-settings payload via a mock server.

An initial debug-profile build could not download its llama.cpp header source
inside the network sandbox. The verified release-profile checks used the
existing build cache and passed.

## Disposable M comparison

Evidence: `.recovery/structured-translation-2026-09-14/comparison/`.
Nine nonempty text blocks from retained M page `001.jpg` were translated with
`anthropic/claude-opus-4.6`, English, the user's saved custom guidance, and
unchanged provider defaults (temperature and max_tokens omitted).

Both the legacy and structured paths returned all nine translations. The
structured path saved and reopened correctly in a new metadata-only project
copy. The source copy's scene, history and project metadata remained
byte-identical. The user's original project was never opened for this test.

Single-run times were 7.99 seconds legacy and 10.95 seconds structured. The
translation guidance and sources matched, but the tagged/JSON protocol prompts
differed. These are compatibility observations, not an isolated constraint-only
experiment or evidence of better speed/translation quality. Wording varied;
there was no obvious block-mapping regression. The old path also succeeded, so
this sample does not establish a reduction in the real-world failure rate.
The demonstrated benefit is enforcing coverage and rejecting unsafe mappings.

The opt-in test makes two billable requests. To repeat, set
`KOHARU_TRANSLATION_PROJECT` to a retained M project directory (read only),
`KOHARU_TRANSLATION_OUTPUT` to a **new** evidence directory, and
`KOHARU_TRANSLATION_PROMPT` to a UTF-8 file containing the intended custom prompt:

```powershell
bun cargo test --release -p koharu-app --features cuda --lib llm::tests::openrouter_disposable_translation_comparison -- --ignored --nocapture
```

It reads the existing OpenAI-compatible credential from Windows credential
storage and sends it only to OpenRouter. Keys are not written into artifacts.
The recovery directory retains the before-edit source files and previous
development executable. Protected STABLE has not been promoted or tested.
