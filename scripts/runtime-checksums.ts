// @ts-nocheck
/**
 * Regenerates crates/koharu-runtime/checksums.txt: the pinned SHA-256 of every
 * native runtime archive Koharu downloads. koharu-runtime refuses any download
 * (or cached copy) that isn't listed here with a matching digest.
 *
 * Versions are read from the Rust sources, so after bumping one of them just
 * re-run this script:
 *   - llama.cpp release assets for LLAMA_CPP_TAG   (.cargo/config.toml)
 *   - ZLUDA release assets for RELEASE_TAG         (crates/koharu-runtime/src/zluda.rs)
 *   - CUDA wheels listed in WHEELS                 (crates/koharu-runtime/src/cuda.rs)
 *   - Korean OCR verifier downloads                (crates/koharu-runtime/src/korean_ocr.rs)
 *
 * Digests come from GitHub's release API (`asset.digest`, falling back to
 * hashing the download) and PyPI's JSON API. The Korean model archive lives at
 * an unversioned Baidu BOS URL with no published digest, so its approved hash
 * is carried forward from the existing file and the remote bytes are checked
 * against it: if they ever change, this script fails instead of silently
 * accepting them. Review the new archive, then re-pin it deliberately with
 * --repin-unversioned.
 *
 * Usage:
 *   bun scripts/runtime-checksums.ts                       # rewrite checksums.txt
 *   bun scripts/runtime-checksums.ts --check               # exit 1 if it is out of date
 *   bun scripts/runtime-checksums.ts --repin-unversioned   # accept current Baidu bytes (after review)
 * Set GITHUB_TOKEN to avoid GitHub API rate limits.
 */
import { readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'

const root = path.resolve(__dirname, '..')
const runtimeDir = path.join('crates', 'koharu-runtime')
const outputPath = path.join(root, runtimeDir, 'checksums.txt')

const LLAMA_REPO = 'ggml-org/llama.cpp'
const ZLUDA_REPO = 'vosen/ZLUDA'

async function read(relative: string): Promise<string> {
  return readFile(path.join(root, relative), 'utf8')
}

function capture(source: string, pattern: RegExp, what: string): string {
  const match = source.match(pattern)
  if (!match) throw new Error(`could not find ${what}`)
  return match[1]
}

async function fetchJson(url: string, headers: Record<string, string> = {}) {
  const res = await fetch(url, { headers })
  if (!res.ok) throw new Error(`GET ${url} failed: ${res.status} ${await res.text()}`)
  return res.json()
}

async function sha256OfUrl(url: string): Promise<string> {
  const res = await fetch(url)
  if (!res.ok || !res.body) throw new Error(`GET ${url} failed: ${res.status}`)
  const hasher = new Bun.CryptoHasher('sha256')
  for await (const chunk of res.body) hasher.update(chunk)
  return hasher.digest('hex')
}

async function githubRelease(repo: string, tag: string): Promise<string[]> {
  const headers: Record<string, string> = { Accept: 'application/vnd.github+json' }
  if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`
  const release = await fetchJson(
    `https://api.github.com/repos/${repo}/releases/tags/${encodeURIComponent(tag)}`,
    headers,
  )
  const lines: string[] = []
  for (const asset of release.assets.sort((a, b) => a.name.localeCompare(b.name))) {
    // Built the same way as the Rust download URLs (RELEASE_BASE_URL/tag/name).
    const url = `https://github.com/${repo}/releases/download/${tag}/${asset.name}`
    const digest = asset.digest?.startsWith('sha256:')
      ? asset.digest.slice('sha256:'.length)
      : await sha256OfUrl(url)
    lines.push(`${digest}  ${url}`)
  }
  return lines
}

async function pypiWheels(pkg: string, version: string): Promise<string[]> {
  const release = await fetchJson(`https://pypi.org/pypi/${pkg}/${version}/json`)
  return release.urls
    .filter((file) => file.filename.endsWith('.whl'))
    .sort((a, b) => a.filename.localeCompare(b.filename))
    .map((file) => `${file.digests.sha256}  ${file.url}`)
}

/** `url -> sha256` for every pinned line in an existing checksums file. */
function parsePins(contents: string): Map<string, string> {
  const pins = new Map<string, string>()
  for (const raw of contents.split('\n')) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    const [hash, url] = line.split(/\s+/, 2)
    if (hash && url) pins.set(url, hash)
  }
  return pins
}

/** The one ONNX Runtime wheel the Korean OCR verifier downloads, via PyPI. */
async function onnxRuntimeWheel(version: string, name: string, url: string): Promise<string> {
  const release = await fetchJson(`https://pypi.org/pypi/onnxruntime/${version}/json`)
  const file = release.urls.find((candidate) => candidate.filename === name)
  if (!file) throw new Error(`PyPI onnxruntime ${version} has no ${name}`)
  if (file.url !== url) {
    throw new Error(
      `ORT_WHEEL_URL in korean_ocr.rs does not match PyPI's URL for ${name}:\n  code: ${url}\n  pypi: ${file.url}`,
    )
  }
  return `${file.digests.sha256}  ${url}`
}

/**
 * An archive with no upstream digest: keep the approved pin and verify the
 * remote still matches it. Never accept new bytes unless explicitly re-pinned.
 */
async function unversioned(url: string, pins: Map<string, string>, repin: boolean): Promise<string> {
  const pinned = pins.get(url)
  const remote = await sha256OfUrl(url)
  if (repin) {
    if (pinned !== remote) console.warn(`re-pinned ${url}: ${pinned ?? '(none)'} -> ${remote}`)
    return `${remote}  ${url}`
  }
  if (!pinned) {
    throw new Error(
      `${url} has no pinned SHA-256 and no upstream digest to trust. Review the archive, ` +
        'then re-run with --repin-unversioned to pin its current bytes.',
    )
  }
  if (remote !== pinned) {
    throw new Error(
      `${url} changed upstream since it was pinned (pinned ${pinned}, now ${remote}). ` +
        'Review the new archive, then re-run with --repin-unversioned to accept it deliberately.',
    )
  }
  return `${pinned}  ${url}`
}

async function generate(current: string, repin: boolean): Promise<string> {
  const llamaTag = capture(
    await read('.cargo/config.toml'),
    /LLAMA_CPP_TAG\s*=\s*"([^"]+)"/,
    'LLAMA_CPP_TAG in .cargo/config.toml',
  )
  const zludaTag = capture(
    await read(path.join(runtimeDir, 'src', 'zluda.rs')),
    /const RELEASE_TAG: &str = "([^"]+)"/,
    'RELEASE_TAG in zluda.rs',
  )
  const wheels = [
    ...(await read(path.join(runtimeDir, 'src', 'cuda.rs'))).matchAll(
      /package: "([^"/]+)\/([^"]+)"/g,
    ),
  ].map(([, pkg, version]) => ({ pkg, version }))
  if (wheels.length === 0) throw new Error('no WHEELS found in cuda.rs')

  const korean = await read(path.join(runtimeDir, 'src', 'korean_ocr.rs'))
  const ortVersion = capture(korean, /const ORT_VERSION: &str = "([^"]+)"/, 'ORT_VERSION')
  const ortWheelName = capture(korean, /const ORT_WHEEL_NAME: &str = "([^"]+)"/, 'ORT_WHEEL_NAME')
  const ortWheelUrl = capture(korean, /const ORT_WHEEL_URL: &str = "([^"]+)"/, 'ORT_WHEEL_URL')
  const modelUrl = capture(korean, /const MODEL_ARCHIVE_URL: &str = "([^"]+)"/, 'MODEL_ARCHIVE_URL')

  const sections: string[] = [
    '# Generated by scripts/runtime-checksums.ts. Do not edit by hand; re-run the script.',
    '# SHA-256 of every native runtime archive Koharu downloads, keyed by download URL.',
    '# koharu-runtime verifies each download (and each cached copy) against this list.',
    '',
    `# llama.cpp ${llamaTag}`,
    ...(await githubRelease(LLAMA_REPO, llamaTag)),
    '',
    `# ZLUDA ${zludaTag}`,
    ...(await githubRelease(ZLUDA_REPO, zludaTag)),
  ]
  for (const { pkg, version } of wheels) {
    sections.push('', `# ${pkg} ${version} (PyPI)`, ...(await pypiWheels(pkg, version)))
  }
  sections.push(
    '',
    '# Fork-only: Korean OCR verifier downloads (crates/koharu-runtime/src/korean_ocr.rs).',
    `# onnxruntime ${ortVersion} (PyPI)`,
    await onnxRuntimeWheel(ortVersion, ortWheelName, ortWheelUrl),
    '# korean_PP-OCRv5 model (Baidu BOS, unversioned URL): approved baseline carried',
    '# forward; the script refuses changed bytes unless run with --repin-unversioned.',
    await unversioned(modelUrl, parsePins(current), repin),
  )
  return sections.join('\n') + '\n'
}

async function main() {
  const check = process.argv.includes('--check')
  const repin = process.argv.includes('--repin-unversioned')
  const current = await readFile(outputPath, 'utf8').catch(() => '')
  const expected = await generate(current, repin)

  if (current === expected) {
    console.log('crates/koharu-runtime/checksums.txt is up to date')
    return
  }
  if (check) {
    console.error('crates/koharu-runtime/checksums.txt is out of date. Expected contents:\n')
    console.error(expected)
    console.error('Run `bun scripts/runtime-checksums.ts` and commit the result.')
    process.exit(1)
  }
  await writeFile(outputPath, expected)
  console.log('wrote crates/koharu-runtime/checksums.txt')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
