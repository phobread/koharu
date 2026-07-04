'use client'

import { getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import type { SceneSnapshot } from '@/lib/api/schemas'
import { openImageFiles, openImageFolder, openKhrFile } from '@/lib/io/openFiles'
import { saveBlob, saveBlobToDirectory } from '@/lib/io/saveBlob'
import { exportProject, uploadKhrArchive, uploadPages, uploadPagesByPaths } from '@/lib/io/scene'
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

const exportExtension: Record<'khr' | 'psd' | 'rendered' | 'inpainted', string> = {
  khr: 'khr',
  psd: 'zip',
  rendered: 'zip',
  inpainted: 'zip',
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

function exportFailureMessage(
  format: 'khr' | 'psd' | 'rendered' | 'inpainted',
  raw: string,
): string {
  if (/no pages in selection/i.test(raw)) {
    return 'No pages selected to export — add or select a page first.'
  }
  if (/requested layer populated|layer populated/i.test(raw)) {
    const layer = format === 'inpainted' ? 'inpainted images' : 'rendered images'
    return `No ${layer} to export yet — run Process → Process All first to generate them, then try exporting again.`
  }
  return `Export failed: ${raw}`
}

export async function exportCurrentProjectAs(
  format: 'khr' | 'psd' | 'rendered' | 'inpainted',
  pages?: string[],
  opts?: { outputDirectory?: string },
): Promise<void> {
  try {
    const defaultFont = usePreferencesStore.getState().defaultFont
    const { blob, filename } = await exportProject({ format, pages, defaultFont })
    const base = sanitiseBaseName(currentProjectName())
    // Prefer the server's Content-Disposition filename (matches the actual
    // bytes — a raw PNG/PSD for single-file responses, a zip for multi).
    // Fall back to our guess only if the header is missing/unparseable.
    const defaultName = filename ?? `${base}.${exportExtension[format]}`
    if (opts?.outputDirectory) {
      await saveBlobToDirectory(blob, defaultName, opts.outputDirectory)
      return
    }
    await saveBlob(blob, defaultName)
  } catch (err) {
    // Surface the failure to the user instead of swallowing it. Previously this
    // only `console.error`'d and rethrew into a `void` caller, so a failed
    // export (e.g. exporting rendered/inpainted before those layers exist, which
    // the server rejects with "no pages have the requested layer populated")
    // showed no dialog and no message — looking like a broken feature.
    console.error('Export failed:', err)
    const raw = err instanceof Error ? err.message : String(err)
    useEditorUiStore.getState().showError(exportFailureMessage(format, raw))
  }
}
