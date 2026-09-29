'use client'

import { getConfig, startPipeline } from '@/lib/api/default/default'
import type { PipelineConfig, StartPipelineResponse } from '@/lib/api/schemas'
import { renderDefaultsForPipeline } from '@/lib/io/renderDefaults'
import { awaitPendingSceneEdits } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { type ProcessSteps, usePreferencesStore } from '@/lib/stores/preferencesStore'

const ALL_STEPS: ProcessSteps = {
  detect: true,
  ocr: true,
  translate: true,
  inpaint: true,
  render: true,
}

/** Engine ids for the chosen steps (all by default), in pipeline order. */
export function processSteps(p: PipelineConfig, chosen: ProcessSteps = ALL_STEPS): string[] {
  return [
    ...(chosen.detect ? [p.detector, p.segmenter, p.bubble_segmenter, p.font_detector] : []),
    chosen.ocr ? p.ocr : null,
    chosen.translate ? p.translator : null,
    chosen.inpaint ? p.inpainter : null,
    chosen.render ? p.renderer : null,
  ].filter((s): s is string => !!s)
}

/**
 * Start a Process run of the ticked steps over `pages` (all pages when
 * omitted). With `onlyMissing` each step runs only where its output is
 * missing, so finished pages and hand-corrected boxes are kept; without it
 * the ticked steps are redone. Returns `undefined` when no pipeline is
 * configured or no step is ticked.
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
  const steps = processSteps(cfg.pipeline, prefs.processSteps)
  if (steps.length === 0) return undefined
  return startPipeline({
    steps,
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
