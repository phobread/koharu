'use client'

import {
  addOfficialPagesFromPaths,
  applyCommand,
  createPages,
  createPagesFromPaths,
  createProject,
  deleteCurrentProject,
  getConfig,
  getSceneJson,
  getExportCurrentProjectUrl,
  getGetConfigQueryKey,
  getGetCurrentLlmQueryKey,
  getGetSceneJsonQueryKey,
  importProject,
  listOperations,
  listProjects,
  patchConfig,
  putCurrentProject,
  redo,
  reorderTextNodes,
  startPipeline,
  undo,
} from '@/lib/api/default/default'
import { ApiError } from '@/lib/api/fetch'
import type {
  AddOfficialPagesResponse,
  ConfigPatch,
  CreateProjectRequest,
  ExportProjectRequest,
  Op,
  OpenProjectRequest,
  ProjectSummary,
  ReadingOrder,
  SceneSnapshot,
  Scene,
} from '@/lib/api/schemas'
import { renderDefaultsForPipeline } from '@/lib/io/renderDefaults'
import { filenameFromContentDisposition } from '@/lib/io/saveBlob'
import { ops } from '@/lib/ops'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

/**
 * Imperative action helpers. Every mutation below is a thin wrapper that
 *   1. calls the orval-generated request function (never raw `fetch`), and
 *   2. invalidates the React Query cache entries affected by the change.
 *
 * The UI reads scene / config / llm state via the generated `useGet*` hooks;
 * after each mutation React Query refetches — no client-side scene reducer,
 * no optimistic mirroring, backend is the single source of truth.
 */

export const invalidateScene = () =>
  queryClient.invalidateQueries({ queryKey: getGetSceneJsonQueryKey() })

const invalidateConfig = () => queryClient.invalidateQueries({ queryKey: getGetConfigQueryKey() })

const invalidateLlm = () => queryClient.invalidateQueries({ queryKey: getGetCurrentLlmQueryKey() })

// Ops ------------------------------------------------------------------------

let historyMutationQueue: Promise<void> = Promise.resolve()

const enqueueHistoryMutation = (run: () => Promise<void>): Promise<void> => {
  const next = historyMutationQueue.then(run, run)
  historyMutationQueue = next.catch(() => undefined)
  return next
}

/** Pipelines must see queued box deletions and edits before taking a scene snapshot. */
export async function awaitPendingSceneEdits(): Promise<void> {
  let pending: Promise<void>
  do {
    pending = historyMutationQueue
    await pending
  } while (pending !== historyMutationQueue)
}

export async function applyOp(op: Op): Promise<void> {
  await enqueueHistoryMutation(async () => {
    await applyCommand(op)
    await invalidateScene()
  })
}

/** Build a dependent edit only after earlier typing/formatting saves finish. */
export async function applyOpFromScene(build: (scene: Scene) => Op | null): Promise<boolean> {
  let applied = false
  await enqueueHistoryMutation(async () => {
    const { scene } = await getSceneJson()
    const op = build(scene)
    if (!op) return
    await applyCommand(op)
    await invalidateScene()
    applied = true
  })
  return applied
}

export async function undoOp(): Promise<void> {
  await enqueueHistoryMutation(async () => {
    await undo()
    await invalidateScene()
  })
}

export async function redoOp(): Promise<void> {
  await enqueueHistoryMutation(async () => {
    await redo()
    await invalidateScene()
  })
}

export async function reorderPageTextNodes(pageId: string, order: ReadingOrder): Promise<void> {
  await reorderTextNodes(pageId, order)
  await invalidateScene()
}

// Auto-render ---------------------------------------------------------------
//
// `queueAutoRender(pageId)` schedules a debounced renderer-pipeline invocation
// so a text-block edit (move/resize/translation/color/etc.) produces an
// updated rendered image without the user running Render manually.
//
// Coalescing is essential: slider drags and typing emit many ops per second;
// the trailing-edge debounce fires one render after the edits settle.

const AUTO_RENDER_DEBOUNCE_MS = 500

// Debounce per page: editing page A then jumping to page B within the window
// must not cancel A's pending render — each page settles independently.
const autoRenderTimers = new Map<string, ReturnType<typeof setTimeout>>()

export function queueAutoRender(pageId: string): void {
  const existing = autoRenderTimers.get(pageId)
  if (existing) clearTimeout(existing)
  autoRenderTimers.set(
    pageId,
    setTimeout(() => {
      autoRenderTimers.delete(pageId)
      void runAutoRender(pageId)
    }, AUTO_RENDER_DEBOUNCE_MS),
  )
}

// Auto-renders being started, and the jobs of started ones, so an export can
// wait for the render of an edit made just before it.
const autoRenderStarts = new Set<Promise<void>>()
const autoRenderJobs = new Set<string>()

function runAutoRender(pageId: string): Promise<void> {
  const start = (async () => {
    try {
      const cfg = await getConfig()
      const renderer = cfg.pipeline?.renderer
      if (!renderer) return
      const { operationId } = await startPipeline({
        steps: [renderer],
        pages: [pageId],
        ...renderDefaultsForPipeline(),
      })
      autoRenderJobs.add(operationId)
    } catch (err) {
      // Auto-render failures shouldn't disturb the editing flow; users can
      // always run Render manually from the toolbar / menu.
      console.error('Auto-render failed:', err)
    }
  })()
  autoRenderStarts.add(start)
  void start.finally(() => autoRenderStarts.delete(start))
  return start
}

/**
 * Start the debounced auto-renders now and wait for every auto-render to
 * finish, so what comes next (an export) sees the latest edits rendered.
 * Asks the server rather than the event stream (a job is registered before
 * its start request returns). Gives up waiting after `timeoutMs`.
 */
export async function settleAutoRenders(timeoutMs = 60_000, pollMs = 250): Promise<void> {
  for (const [pageId, timer] of [...autoRenderTimers]) {
    clearTimeout(timer)
    autoRenderTimers.delete(pageId)
    void runAutoRender(pageId)
  }
  await Promise.all([...autoRenderStarts])
  const deadline = Date.now() + timeoutMs
  while (autoRenderJobs.size > 0) {
    const operations = await listOperations().then(
      (r) => r.operations,
      () => null,
    )
    if (!operations) return
    const running = new Set(operations.filter((j) => j.status === 'running').map((j) => j.id))
    for (const id of autoRenderJobs) if (!running.has(id)) autoRenderJobs.delete(id)
    if (autoRenderJobs.size === 0 || Date.now() >= deadline) return
    await new Promise((resolve) => setTimeout(resolve, pollMs))
  }
}

/**
 * Delete text boxes as one undo step. Built from the latest saved scene, so
 * boxes already gone are skipped and queued edits land first. Returns how
 * many boxes were removed; they also leave the selection.
 */
export async function deleteTextNodes(pageId: string, ids: Iterable<string>): Promise<number> {
  const wanted = new Set(ids)
  if (wanted.size === 0) return 0
  let removed: string[] = []
  await applyOpFromScene((scene) => {
    const page = scene.pages[pageId]
    if (!page) return null
    // Removal indices track the shrinking node list so undo re-inserts each
    // box where it was.
    const keys = Object.keys(page.nodes)
    const batch: Op[] = []
    removed = []
    for (const id of [...keys]) {
      const node = page.nodes[id]
      if (!wanted.has(id) || !node || !('text' in node.kind)) continue
      const idx = keys.indexOf(id)
      batch.push(ops.removeNode(pageId, id, node, idx))
      keys.splice(idx, 1)
      removed.push(id)
    }
    if (batch.length === 0) return null
    return batch.length === 1 ? batch[0] : ops.batch('Delete text boxes', batch)
  })
  if (removed.length === 0) return 0
  const selection = useSelectionStore.getState()
  if (selection.pageId === pageId) {
    selection.selectMany([...selection.nodeIds].filter((id) => !removed.includes(id)))
  }
  queueAutoRender(pageId)
  return removed.length
}

/** Select every text node on the active page. No-op if no project/page open. */
export function selectAllTextNodesOnCurrentPage(): void {
  const pageId = useSelectionStore.getState().pageId
  if (!pageId) return
  const snap = queryClient.getQueryData<SceneSnapshot>(getGetSceneJsonQueryKey())
  const page = snap?.scene?.pages?.[pageId]
  if (!page) return
  const ids: string[] = []
  for (const [id, node] of Object.entries(page.nodes)) {
    if (node && 'text' in node.kind) ids.push(id)
  }
  useSelectionStore.getState().selectMany(ids)
}

// Project lifecycle ----------------------------------------------------------

/** Page/node ids are project-scoped: any project transition must drop the
 * selection, or the UI keeps acting on ids from the previous scene. */
const resetSelection = () => useSelectionStore.getState().setPage(null)

/** Remember the opened project for the mouse forward button. */
const rememberProject = (id: string) => useEditorUiStore.getState().setLastProjectId(id)

export async function createAndOpenProject(req: CreateProjectRequest): Promise<ProjectSummary> {
  const summary = await createProject(req)
  rememberProject(summary.id)
  resetSelection()
  await invalidateScene()
  return summary
}

export async function switchProject(req: OpenProjectRequest): Promise<void> {
  await putCurrentProject(req)
  rememberProject(req.id)
  resetSelection()
  await invalidateScene()
}

let reopening = false

/**
 * Open the project opened last (the mouse forward button). Does nothing when
 * there is none, it was deleted, or a reopen is already under way. Returns
 * whether a project was opened.
 */
export async function reopenLastProject(): Promise<boolean> {
  const id = useEditorUiStore.getState().lastProjectId
  if (!id || reopening) return false
  reopening = true
  try {
    const { projects } = await listProjects()
    if (!projects.some((p) => p.id === id)) return false
    await switchProject({ id })
    return true
  } finally {
    reopening = false
  }
}

export async function closeProject(): Promise<void> {
  // A click on the back arrow can land right after typing: save that first.
  await awaitPendingSceneEdits()
  await deleteCurrentProject()
  resetSelection()
  await invalidateScene()
}

// Pages import ---------------------------------------------------------------

export async function uploadPages(files: File[], replace: boolean): Promise<string[]> {
  const form = new FormData()
  for (const file of files) form.append('file', file, file.name)
  form.append('replace', replace ? 'true' : 'false')
  const res = await createPages({ body: form })
  if (replace) resetSelection()
  await invalidateScene()
  return res.pages
}

/**
 * Tauri fast-path: hand the backend a list of absolute file paths. Skips
 * the per-file `readFile` IPC round-trip, skips JS-side buffering, skips
 * multipart upload — the Rust side reads + decodes + hashes in parallel.
 */
export async function uploadPagesByPaths(paths: string[], replace: boolean): Promise<string[]> {
  const res = await createPagesFromPaths({ paths, replace })
  if (replace) resetSelection()
  await invalidateScene()
  return res.pages
}

/**
 * Give the pages their official release (desktop: files by path). The
 * backend pairs each file with the page showing the same picture; cleaned
 * pages take the release's onomatopoeia at once, and lose their rendered
 * image, which is rendered again here.
 */
export async function addOfficialPagesByPaths(paths: string[]): Promise<AddOfficialPagesResponse> {
  await awaitPendingSceneEdits()
  const res = await addOfficialPagesFromPaths({ paths })
  await invalidateScene()
  for (const pageId of res.rerender) queueAutoRender(pageId)
  return res
}

export async function uploadKhrArchive(file: File): Promise<ProjectSummary> {
  // The generated `importProject` takes the archive as a `Blob` and sets the
  // `application/zip` content type itself; a `File` is already a `Blob`.
  const summary = await importProject(file)
  rememberProject(summary.id)
  resetSelection()
  await invalidateScene()
  return summary
}

// Export ---------------------------------------------------------------------

/**
 * Export wrapper that keeps the server-supplied filename.
 *
 * The backend returns the raw file for single-page exports (e.g. a PNG or
 * PSD with `Content-Type: image/png`), and a zip when the format produces
 * multiple files. The raw-file shortcut means we can't hardcode `.zip` in
 * the UI — we'd end up feeding a PNG to `unzipSync` and crashing. Read
 * the `Content-Disposition` filename so the caller gets the correct
 * extension + `blob.type` to drive the save path.
 */
export async function exportProject(
  req: ExportProjectRequest,
): Promise<{ blob: Blob; filename?: string }> {
  const res = await fetch(getExportCurrentProjectUrl(), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(req),
  })
  if (!res.ok) {
    const body = await res.json().catch(() => null)
    const message =
      (body && typeof body === 'object' && 'message' in body && typeof body.message === 'string'
        ? body.message
        : null) ??
      res.statusText ??
      `HTTP ${res.status}`
    throw new ApiError(res.status, message, body)
  }
  const blob = await res.blob()
  const filename = filenameFromContentDisposition(res.headers.get('content-disposition'))
  return { blob, filename }
}

// Config ---------------------------------------------------------------------

export async function updateConfig(patch: ConfigPatch): Promise<void> {
  await patchConfig(patch)
  await invalidateConfig()
}

// LLM ------------------------------------------------------------------------

export function invalidateCurrentLlm(): Promise<void> {
  return invalidateLlm()
}
