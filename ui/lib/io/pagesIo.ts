'use client'

import { getGetSceneJsonQueryKey, getSceneJson } from '@/lib/api/default/default'
import type { SceneSnapshot } from '@/lib/api/schemas'
import { openImageFiles, openImageFolder, openKhrFile } from '@/lib/io/openFiles'
import { prepareExportDirectory, saveBlob, saveBlobToDirectory } from '@/lib/io/saveBlob'
import {
  awaitPendingSceneEdits,
  exportProject,
  settleAutoRenders,
  uploadKhrArchive,
  uploadPages,
  uploadPagesByPaths,
} from '@/lib/io/scene'
import { type ExportSummary, exportSummary } from '@/lib/pageStatus'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'

/**
 * Platform-neutral image import. `openImageFiles` / `openImageFolder` return
 * `File[]` on both Tauri and the web; the upload + scene invalidation lives
 * in `lib/io/scene.ts` on top of the orval-generated `createPages` mutation.
 */
export async function importPages(
  mode: 'replace' | 'append',
  source: 'files' | 'folder',
): Promise<void> {
  const picked = source === 'folder' ? await openImageFolder() : await openImageFiles()
  const replace = mode === 'replace'
  if (picked.kind === 'paths') {
    if (picked.paths.length === 0) return
    await uploadPagesByPaths(picked.paths, replace)
    return
  }
  if (picked.files.length === 0) return
  await uploadPages(picked.files, replace)
}

/**
 * Import a `.khr` archive. Works on both desktop and web: the archive file
 * is picked via the cross-platform `openKhrFile`, and the destination is
 * allocated server-side so the client never needs to touch the filesystem.
 */
export async function importKhrFile(): Promise<void> {
  const file = await openKhrFile()
  if (!file) return
  await uploadKhrArchive(file)
}

// ---------------------------------------------------------------------------
// Export (server returns bytes; saveBlob dispatches Tauri-dialog / web-FS)
// ---------------------------------------------------------------------------

type ExportFormat = 'khr' | 'psd' | 'rendered' | 'inpainted' | 'best'

/** The folder, inside the project's export folder, each format's dialog opens in. */
const exportSubfolder: Record<ExportFormat, string | undefined> = {
  khr: undefined,
  psd: 'PSD',
  rendered: 'Rendered',
  inpainted: 'Cleaned',
  best: 'Rendered',
}

const exportExtension: Record<ExportFormat, string> = {
  khr: 'khr',
  psd: 'zip',
  rendered: 'zip',
  inpainted: 'zip',
  best: 'zip',
}

/** Sanitise an arbitrary project name for use as a filename stem. */
function sanitiseBaseName(name: string | undefined | null): string {
  const cleaned = (name ?? '')
    .trim()
    .replace(/[\\/:*?"<>|]+/g, '_')
    .replace(/\s+/g, ' ')
  return cleaned.length > 0 ? cleaned : 'koharu-export'
}

/** Read the current project name from React Query's cached scene snapshot. */
function currentProjectName(): string | undefined {
  const snap = queryClient.getQueryData<SceneSnapshot>(getGetSceneJsonQueryKey())
  return snap?.scene.project?.name ?? undefined
}

function exportFailureMessage(format: ExportFormat, raw: string): string {
  if (/no pages in selection/i.test(raw)) {
    return 'No pages selected to export — add or select a page first.'
  }
  if (/requested layer populated|layer populated/i.test(raw)) {
    const layer = format === 'inpainted' ? 'inpainted images' : 'rendered images'
    return `No ${layer} to export yet — run Process first to make them, then export again.`
  }
  return `Export failed: ${raw}`
}

/**
 * Export the current project in `format` and save it; `pages` limits image
 * formats to those pages. Waits for queued edits and their auto-renders
 * first. Returns whether the file was saved (false when cancelled or failed;
 * failures are shown in the error card).
 */
export async function exportCurrentProjectAs(
  format: ExportFormat,
  pages?: string[],
  opts?: { outputDirectory?: string },
): Promise<boolean> {
  try {
    await awaitPendingSceneEdits()
    await settleAutoRenders()
    const { defaultFont, exportFolder } = usePreferencesStore.getState()
    const { blob, filename } = await exportProject({ format, pages, defaultFont })
    const base = sanitiseBaseName(currentProjectName())
    // Prefer the server's Content-Disposition filename (matches the actual
    // bytes — a raw PNG/PSD for single-file responses, a zip for multi).
    // Fall back to our guess only if the header is missing/unparseable.
    const defaultName = filename ?? `${base}.${exportExtension[format]}`
    if (opts?.outputDirectory) {
      return await saveBlobToDirectory(blob, defaultName, opts.outputDirectory)
    }
    const defaultDirectory = await prepareExportDirectory(
      exportFolder,
      currentProjectName(),
      exportSubfolder[format],
    )
    return await saveBlob(blob, defaultName, { defaultDirectory })
  } catch (err) {
    // Surface the failure to the user instead of swallowing it. Previously this
    // only `console.error`'d and rethrew into a `void` caller, so a failed
    // export (e.g. exporting rendered/inpainted before those layers exist, which
    // the server rejects with "no pages have the requested layer populated")
    // showed no dialog and no message — looking like a broken feature.
    console.error('Export failed:', err)
    const raw = err instanceof Error ? err.message : String(err)
    useEditorUiStore.getState().showError(exportFailureMessage(format, raw))
    return false
  }
}

/**
 * Export `pages` (every page when omitted) as images, each as far as it got:
 * rendered, else cleaned, else the original. Returns what went out, for the
 * notice, or `undefined` when nothing was saved.
 */
export async function exportPageImages(pages?: string[]): Promise<ExportSummary | undefined> {
  if (!(await exportCurrentProjectAs('best', pages))) return undefined
  // Classify from the server's scene: the auto-renders just waited for may
  // not have reached the UI's copy yet.
  const scene = await getSceneJson()
    .then((snap) => snap.scene)
    .catch(() => undefined)
  return scene ? exportSummary(scene.pages, pages) : undefined
}
