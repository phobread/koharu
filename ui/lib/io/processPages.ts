'use client'

import { getConfig, startPipeline } from '@/lib/api/default/default'
import type { PipelineConfig, StartPipelineResponse } from '@/lib/api/schemas'
import { renderDefaultsForPipeline } from '@/lib/io/renderDefaults'
import { awaitPendingSceneEdits } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'

/** Engine ids of a full Process run, in order. */
export function processSteps(p: PipelineConfig): string[] {
  return [
    p.detector,
    p.segmenter,
    p.bubble_segmenter,
    p.font_detector,
    p.ocr,
    p.translator,
    p.inpainter,
    p.renderer,
  ].filter((s): s is string => !!s)
}

/**
 * Start a Process run over `pages` (all pages when omitted). With
 * `onlyMissing` each step runs only where its output is missing, so finished
 * pages and hand-corrected boxes are kept; without it every step is redone.
 * Returns `undefined` when no pipeline is configured.
 */
export async function processPages(opts: {
  pages?: string[]
  onlyMissing: boolean
}): Promise<StartPipelineResponse | undefined> {
  await awaitPendingSceneEdits()
  const cfg = await getConfig()
  if (!cfg.pipeline) return undefined
  const editor = useEditorUiStore.getState()
  const prefs = usePreferencesStore.getState()
  return startPipeline({
    steps: processSteps(cfg.pipeline),
    pages: opts.pages,
    onlyMissing: opts.onlyMissing,
    targetLanguage: editor.selectedLanguage,
    sourceLanguage: prefs.ocrLanguage,
    systemPrompt: prefs.customSystemPrompt,
    readingOrder: editor.readingOrder === 'custom' ? undefined : editor.readingOrder,
    ...renderDefaultsForPipeline(),
  })
}

/** Ids of `selected` pages in page order. */
export function orderedPageIds(
  pages: Record<string, unknown> | undefined,
  selected: Set<string>,
): string[] {
  return Object.keys(pages ?? {}).filter((id) => selected.has(id))
}

/**
 * [`processPages`] for buttons and menus: shows `nothingToDo` when an
 * only-missing run finds every page done, and errors in the error card.
 */
export async function processPagesWithFeedback(
  opts: { pages?: string[]; onlyMissing: boolean },
  nothingToDo: string,
): Promise<StartPipelineResponse | undefined> {
  const ui = useEditorUiStore.getState()
  try {
    const started = await processPages(opts)
    if (started && opts.onlyMissing && started.pageCount === 0) ui.showNotice(nothingToDo)
    return started
  } catch (err) {
    ui.showError(String(err))
    return undefined
  }
}
