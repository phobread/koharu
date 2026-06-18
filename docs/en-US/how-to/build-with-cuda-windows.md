---
title: Build With CUDA on Windows
---

# Build With CUDA on Windows

This is a reproducible, battle-tested walkthrough for building Koharu from source with
CUDA GPU acceleration on Windows, including the non-obvious failures you will hit and how
to fix them. It complements [Build From Source](build-from-source.md); read that first
for the general flow.

Verified on: Windows 11, NVIDIA RTX 4050 Laptop (6 GB, compute 8.9), 2026-06-18.

## TL;DR of the gotchas

| Symptom | Cause | Fix |
| --- | --- | --- |
| `Unsupported cuda toolkit version: 13.3` | `cudarc` 0.19.7 supports CUDA ≤ 13.2 only, exact match | Install CUDA Toolkit **13.2** (not 13.3) |
| `preprocessor.h … C1189: MSVC traditional preprocessor` | CUDA 13 CCCL rejects MSVC legacy preprocessor | `NVCC_PREPEND_FLAGS=-Xcompiler=/Zc:preprocessor` |
| `cpp_dialect.h … libcu++ requires at least C++ 17` | CUDA 13 dropped C++14; candle passes no `-std` | `NVCC_APPEND_FLAGS=-std=c++17` |
| `Unable to find libclang` (building `koharu-llm`) | llama.cpp bindings need libclang | Install LLVM, set `LIBCLANG_PATH` |

## Prerequisites

- **NVIDIA driver** new enough for CUDA 13 (`nvidia-smi` should report CUDA ≥ 13).
- **Visual Studio 2022** with the C++ workload (provides `cl.exe`).
- **Rust ≥ 1.95** (2024 edition) and **Bun ≥ 1.0**.

## 1. CUDA Toolkit 13.2 (NOT 13.3)

`cudarc` (pulled in by candle) parses `nvcc --version` and requires an *exact*
`major.minor` from a hardcoded list whose newest entry is **13.2**. CUDA 13.3 makes the
build panic. winget offers 13.2 directly:

```powershell
# if you already installed 13.3, remove it first (elevated):
winget uninstall --id Nvidia.CUDA
# winget may leave the v13.3 folder behind; delete it to avoid duplicates.

winget install --id Nvidia.CUDA --version 13.2
```

Confirm: `nvcc --version` reports `release 13.2`. `CUDA_PATH` should point at
`C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.2`.

## 2. cuDNN 9.x (cuda13 build)

cuDNN is **not on winget**. Download the cuda13 archive from NVIDIA's redistributable
server and copy it into the toolkit (needs admin because of `Program Files`):

```
https://developer.download.nvidia.com/compute/cudnn/redist/cudnn/windows-x86_64/
  cudnn-windows-x86_64-9.23.2.1_cuda13-archive.zip   (or newer *_cuda13-archive.zip)
```

Extract and copy `bin\*`, `include\*`, `lib\*` into the matching folders under
`…\CUDA\v13.2`. (`cudarc` uses dynamic loading, so the DLLs are picked up at runtime from
the CUDA `bin` on `PATH`.)

## 3. LLVM / libclang (for the local-LLM crate)

`koharu-llm` binds llama.cpp with `bindgen`, which needs libclang:

```powershell
winget install --id LLVM.LLVM
setx LIBCLANG_PATH "C:\Program Files\LLVM\bin"
```

> Only the **full app** needs libclang. Building just `koharu-ml` does not.

## 4. NVCC flags for CUDA 13 + MSVC

Set these (user scope is fine; a fresh terminal then picks them up automatically):

```powershell
setx NVCC_PREPEND_FLAGS "-Xcompiler=/Zc:preprocessor"
setx NVCC_APPEND_FLAGS  "-std=c++17"
```

Without them, `nvcc` fails compiling candle's CUDA kernels (CCCL preprocessor /
C++17 dialect errors above).

## 5. Build

```powershell
bun install
bun run build            # full desktop app -> target\release\koharu.exe
# or, lower level (note: --features cuda is required; it is not a default):
bun cargo build --release -p koharu --features cuda
```

`bun run build` / `bun run dev` go through `scripts/dev.ts`, which auto-discovers `nvcc`
and `cl.exe`. The standalone helper binaries in `koharu-ml/bin` build with
`bun cargo build --release -p koharu-ml --features cuda`.

## 6. Verify

Run headless and drive one detection through the HTTP API (see
[Run GUI, Headless, and MCP Modes](run-gui-headless-and-mcp.md)):

```powershell
koharu.exe --headless --port 4000 --debug
# POST /api/v1/projects {"name":"t"}
# POST /api/v1/pages/from-paths {"paths":["<image.png>"],"replace":false}
# POST /api/v1/pipelines {"steps":["comic-text-detector"]}
# GET  /api/v1/operations  (poll until "completed")
```

Log lines `ggml_cuda_init: found 1 CUDA devices` and `GPU compute capability: 8.9`
confirm the GPU is in use.

## Known issue: standalone `koharu-ml` binaries abort at exit

The standalone dev binaries (e.g. `comic-text-detector.exe`) write their results and then
**abort on exit** with `CudnnError(CUDNN_STATUS_INTERNAL_ERROR)` inside `cudarc`'s
`Cudnn::drop` — the cuDNN handle is destroyed during thread-local teardown after the CUDA
context is already gone (`thread local panicked on drop, aborting`, exit `0xC0000409`).
It is cosmetic (output is already written) and originates in upstream `cudarc` / the
`mayocream/candle` fork.

The **full `koharu.exe` app is unaffected**: its long-lived multithreaded runtime cleans
up the cuDNN handle while the CUDA context is still valid, so it runs GPU inference and
shuts down cleanly (verified).
