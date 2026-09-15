# Image-model revision pins — 2026-09-14

The handoff's revision-pinning item is implemented for bundled Hugging Face
image-processing assets: inpainting, detection, segmentation, font detection,
and OCR. The user clarified the image-processing focus during implementation.
Optional local translation LLMs and OpenRouter remain unchanged. The optional
Qwen encoder/tokenizer used by the Flux2 prompt-generation development tool is
included because it produces the inpainting prompt embedding; the application's
compiled prompt embedding was not regenerated.

## Baseline and scope

`crates/koharu-runtime/src/model_pins/catalog.rs` records 17 repositories and
47 artifact entries, including associated configurations and tokenizers.
Each repository has an immutable 40-character commit ID. Each artifact records
its filename, expected size, and Hugging Face content ID (LFS SHA256 or regular
Git blob SHA1). The pins are compiled into the executable, not user settings.

Ten repositories already existed in the user's cache. Their current cached
revisions were retained, and all 20 associated cached artifacts (5,537,689,055
bytes) were hashed and checked against upstream metadata at those revisions.
No working model was replaced. Historical unused blobs were left in place.
The other seven repositories were pinned from public metadata available at the
audit; their weights were not downloaded or quality-tested.

The broader initial audit also recorded optional local translation repositories.
Those records are retained only as research evidence and are excluded from the
implemented catalog after the user's clarification.

| Existing image-model repository | Pinned revision |
| --- | --- |
| black-forest-labs/FLUX.2-small-decoder | `a3efc24f613ef42d9428af62fdbd6f5fd8856c4a` |
| unsloth/FLUX.2-klein-4B-GGUF | `0084d1df98e2e2137fe776d55170bc4792ec1d66` |
| PaddlePaddle/PaddleOCR-VL-1.6-GGUF | `511b09642bb324401f15f97cc23bc67e8f0a291d` |
| ogkalu/comic-text-and-bubble-detector | `16e8a622f91fabc6b5b65c96d32d1183f8843546` |
| mayocream/comic-text-detector | `15ade029f4dabd502bc97af6051c8b9f2bec24d5` |
| mayocream/speech-bubble-segmentation | `387bc1e93f3d24702bc8609798b6a13b37420edc` |
| mayocream/lama-manga | `f91c85b26913b3e83f9877867b4c336da3675238` |
| mayocream/aot-inpainting | `bde6131f9d3ef841b435507def8534715ac8e87c` |
| fffonion/yuzumarker-font-detection | `7484242bb840f39e27f10df8ade1f8a9a8fa8f53` |
| PaddlePaddle/PP-DocLayoutV3_safetensors | `3ec586e86ed9245a567bb13395a3db64d5c077cc` |

The optional Korean OCR verification helper uses a versioned vendor archive,
not a Hugging Face repository; that separate archive/runtime path is unchanged.
No scene/history layout, HTTP API, generation settings, inference implementation,
or custom file-path support changed.

## Resolution behavior

Bundled image callers and package presence checks use the same pinned lookup.
It reads `snapshots/<exact commit>/<filename>` directly, then the known
content-addressed blob. It never consults or updates `refs/main`, and never
substitutes another revision if the pinned artifact is unavailable.

The existing Windows cache has model blobs but empty snapshot directories;
the previous code silently failed to create symlinks and needed online metadata
to find those blobs again. The new lookup recognizes their audited content IDs
without needing links or a network connection. Cached sizes are checked; it
does not rehash gigabytes of cached weights on every load. The baseline and
post-test audit perform the full hash verification separately.

For a missing cache entry, the URL contains the exact commit. Remote metadata
must match its commit, content ID and size before downloading. Errors identify
the repository, filename and pinned revision. Unknown catalog entries and
wrong-size cached files fail clearly rather than selecting newer weights.
The generic `huggingface_model` downloader retains the existing branch/cache
behavior for local translation and user-selected repositories.

This follows Hugging Face's
[revision-based download model](https://huggingface.co/docs/huggingface_hub/guides/download)
and [content-addressed cache layout](https://huggingface.co/docs/huggingface_hub/guides/manage-cache).
The implementation also accounts for the actual locked Rust `hf-hub` 0.5.0:
its `CacheRepo::get` requires a ref file even for commit-hash revisions, so
pinned lookups use the commit snapshot path directly.

## Verification

```powershell
bun cargo fmt
bun cargo test --release -p koharu-runtime --lib
bun cargo test --release -p koharu-app --features cuda --test model_pins
```

The runtime suite passed 30 tests. Tests cover cache reuse without links/refs,
stable lookup after `main` changes, no fallback to `main`, preserved custom
repository loading, immutable/unique catalog entries, missing pin errors,
wrong-size cache files, and remote metadata mismatches.

The package coverage test passed. All 20 existing Hugging Face image packages
resolved offline in 0.02 seconds without preparing any missing assets. The
post-test hash audit confirmed all 5,537,689,055 cached bytes unchanged.
The full CUDA Tauri build and its UI TypeScript check passed. The resulting
development executable SHA256 is
`81D9C405671ECDEFF2A368F8DC89D8EC138A8A9FFCF00198F0D1530CB5A8393B`.
STABLE remains
`3DF7B456D4DAA76CF177FDF75E4CCFEE20469E16315040D264800B06A616E483`.
No Koharu process was left running.

An opt-in network test downloaded just the detector's 470-byte
`preprocessor_config.json` into a temporary cache, then resolved the same bytes
again. No model weights were downloaded:

```powershell
bun cargo test --release -p koharu-runtime --lib downloads::tests::downloads_small_pinned_asset_and_reuses_it -- --ignored --nocapture
```

The app integration test checks that every registered Hugging Face image
package has a pinned artifact. The opt-in cache check only calls `ensure` for
already-present packages, so it does not prepare missing models or run inference:

```powershell
$env:KOHARU_PIN_CACHE_ROOT = "$env:LOCALAPPDATA/Koharu"
bun cargo test --release -p koharu-app --features cuda --test model_pins existing_image_packages_resolve_offline -- --ignored --nocapture
```

Retained evidence is under `.recovery/model-pins-2026-09-14/`: `audit.json`,
`cache-after.json`, the audit scripts, source backups, and the previous development
executable. No real project was opened or changed. Protected STABLE was not
promoted or used for testing.

## Deliberate upgrades

Choose and review a new immutable repository commit, then update its revision
and all relevant artifact content IDs/sizes together in the catalog. Keep
weights, projectors, tokenizers and configs within each repository on that same
revision. Cross-repository pairs (Flux2/decoder and the optional prompt encoder/
tokenizer) require an explicit compatibility check before upgrading either.
Verify file availability through metadata first, retain old cached blobs, run
the checks above, and perform an isolated inference comparison only for models
whose bytes actually change. Ship through the full CUDA Tauri build:

```powershell
bun run scripts/dev.ts tauri build --no-bundle --features cuda
```

Do not update pins automatically from a moving `main` branch or overwrite
STABLE without the user's explicit request.
