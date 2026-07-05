'use client'

import type { StateStorage } from 'zustand/middleware'

import { getConfig, patchConfig } from '@/lib/api/default/default'

/**
 * A zustand `persist` storage backed by the server config (`config.toml`)
 * instead of the browser's `localStorage`.
 *
 * The desktop webview loads the UI from the ephemeral `http://127.0.0.1:<port>`
 * origin; `localStorage` is keyed by that origin and is silently wiped whenever
 * the port drifts (e.g. 4000 was busy at launch). Persisting preferences
 * through the backend's `editor.client` blob makes them survive restarts
 * regardless of the port — the root cause of settings (LLM choice, font, …)
 * resetting on restart.
 *
 * Multiple stores share the single `editor.client` field: it holds a JSON
 * object keyed by each store's persist `name`. We keep an in-memory cache,
 * merge per-store writes into it, and debounce a single `PATCH /config` so a
 * burst of edits (or two stores flushing at once) collapses into one request.
 */

type Blob = Record<string, string>

const LEGACY_LOCALSTORAGE_KEY = 'koharu-config'
const FLUSH_DELAY_MS = 400
const LOAD_RETRIES = 5
const LOAD_RETRY_BASE_MS = 300

let cache: Blob | null = null
let loadPromise: Promise<Blob> | null = null
let flushTimer: ReturnType<typeof setTimeout> | null = null
let inflight: Promise<void> | null = null
let dirty = false
let dirtyVersion = 0
let lifecycleFlushInstalled = false

function readLegacyBlob(): Blob | null {
  if (typeof localStorage === 'undefined') return null
  const legacy = localStorage.getItem(LEGACY_LOCALSTORAGE_KEY)
  return legacy ? { [LEGACY_LOCALSTORAGE_KEY]: legacy } : null
}

/**
 * `GET /config` with retries. A single transient failure at startup must not
 * poison hydration: settings would silently fall back to defaults for the
 * whole session (and the first write would overwrite the real saved settings
 * server-side — how "the LLM keeps resetting" bugs are born).
 */
async function getConfigWithRetry(): Promise<Awaited<ReturnType<typeof getConfig>>> {
  let lastError: unknown
  for (let attempt = 0; attempt < LOAD_RETRIES; attempt += 1) {
    try {
      return await getConfig()
    } catch (err) {
      lastError = err
      await new Promise((r) => setTimeout(r, LOAD_RETRY_BASE_MS * 2 ** attempt))
    }
  }
  throw lastError
}

async function load(): Promise<Blob> {
  if (cache) return cache
  if (loadPromise) return loadPromise
  loadPromise = (async () => {
    let blob: Blob = {}
    try {
      const config = await getConfigWithRetry()
      const raw = config.editor?.client
      if (raw) {
        try {
          const parsed: unknown = JSON.parse(raw)
          if (parsed && typeof parsed === 'object') blob = parsed as Blob
        } catch {
          blob = {}
        }
      }
    } catch (err) {
      // Do not cache anything after a failed config read — not even the
      // legacy localStorage blob (it predates some stores, so treating it as
      // the full truth would hydrate those stores as "empty" and let a later
      // flush overwrite their real server-side settings). Failing keeps
      // hydration pending, which is the safe state.
      loadPromise = null
      throw err
    }
    // One-time migration: if nothing is stored server-side yet but the old
    // localStorage prefs exist, adopt them so users don't lose their settings
    // when the source of truth moves to the backend.
    if (Object.keys(blob).length === 0) {
      const legacy = readLegacyBlob()
      if (legacy) {
        blob = legacy
        void patchConfig({ editor: { client: JSON.stringify(blob) } }).catch(() => {})
      }
    }
    cache = blob
    return blob
  })()
  return loadPromise
}

function scheduleFlush(): void {
  if (flushTimer) clearTimeout(flushTimer)
  flushTimer = setTimeout(() => {
    flushTimer = null
    void flushServerConfigStorage()
  }, FLUSH_DELAY_MS)
}

async function writeSnapshot(snapshot: string, keepalive: boolean): Promise<void> {
  if (keepalive && typeof fetch !== 'undefined') {
    const response = await fetch('/api/v1/config', {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ editor: { client: snapshot } }),
      keepalive: true,
    })
    if (!response.ok) throw new Error(`config persist failed: ${response.status}`)
    return
  }
  await patchConfig({ editor: { client: snapshot } })
}

export async function flushServerConfigStorage(options?: { keepalive?: boolean }): Promise<void> {
  if (!cache && loadPromise) {
    await loadPromise.catch(() => {})
  }
  if (!cache || !dirty) return
  if (flushTimer) {
    clearTimeout(flushTimer)
    flushTimer = null
  }
  const keepalive = options?.keepalive ?? false

  // Chain behind any in-flight PATCH so writes never interleave/clobber.
  if (!keepalive && inflight) await inflight.catch(() => {})
  const snapshot = JSON.stringify(cache ?? {})
  const version = dirtyVersion
  if (keepalive) {
    if (dirtyVersion === version) dirty = false
    void writeSnapshot(snapshot, true).catch((err) => {
      console.error('Failed to persist settings to config before unload:', err)
    })
    return
  }

  inflight = writeSnapshot(snapshot, false)
    .then(() => undefined)
    .then(() => {
      if (dirtyVersion === version) dirty = false
    })
    .catch((err) => {
      console.error('Failed to persist settings to config:', err)
    })
    .finally(() => {
      inflight = null
    })
  await inflight
}

function flushForLifecycleExit(): void {
  void flushServerConfigStorage({ keepalive: true })
}

function installLifecycleFlush(): void {
  if (lifecycleFlushInstalled || typeof window === 'undefined') return
  lifecycleFlushInstalled = true
  window.addEventListener('pagehide', flushForLifecycleExit)
  window.addEventListener('beforeunload', flushForLifecycleExit)
  if (typeof document !== 'undefined') {
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'hidden') flushForLifecycleExit()
    })
  }
}

installLifecycleFlush()

export const serverConfigStorage: StateStorage = {
  getItem: async (name) => {
    const blob = await load()
    return blob[name] ?? null
  },
  setItem: async (name, value) => {
    const blob = await load()
    blob[name] = value
    dirty = true
    dirtyVersion += 1
    scheduleFlush()
  },
  removeItem: async (name) => {
    const blob = await load()
    delete blob[name]
    dirty = true
    dirtyVersion += 1
    scheduleFlush()
  },
}
