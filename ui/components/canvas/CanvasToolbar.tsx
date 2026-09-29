'use client'

import {
  LanguagesIcon,
  LoaderCircleIcon,
  ScanIcon,
  ScanTextIcon,
  TypeIcon,
  Wand2Icon,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { ViewControls } from '@/components/canvas/ViewControls'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { getConfig, startPipeline, useGetCurrentLlm } from '@/lib/api/default/default'
import { renderDefaultsForPipeline } from '@/lib/io/renderDefaults'
import { awaitPendingSceneEdits } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useJobsStore } from '@/lib/stores/jobsStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function CanvasToolbar() {
  return (
    <div className='flex shrink-0 flex-wrap items-center gap-2 border-b border-border/60 bg-card px-3 py-2 text-xs text-foreground'>
      <WorkflowButtons />
      <div className='flex-1' />
      <ViewControls />
    </div>
  )
}

/** Currently-busy step (derived from jobsStore). */
function useCurrentStep(): string | null {
  const jobs = useJobsStore((s) => s.jobs)
  for (const j of Object.values(jobs)) {
    if (j.status === 'running' && j.progress?.step) return String(j.progress.step)
  }
  return null
}

function useIsProcessing(): boolean {
  const jobs = useJobsStore((s) => s.jobs)
  return Object.values(jobs).some((j) => j.status === 'running')
}

function WorkflowButtons() {
  const { t } = useTranslation()
  const { data: llmState } = useGetCurrentLlm()
  const llmReady = llmState?.status === 'ready'
  const llmLoading = llmState?.status === 'loading'
  const pageId = useSelectionStore((s) => s.pageId)
  const hasPage = pageId !== null
  const isProcessing = useIsProcessing()
  const currentStep = useCurrentStep()

  /**
   * Run a pipeline step (or a small chain). `GET /config` is the single
   * source of truth for engine ids — every field has a serde default in
   * the Rust `PipelineConfig`, so we trust what the server returns and
   * never hard-code fallbacks here.
   *
   * Detect is the only multi-engine button; it bundles detector +
   * segmenter + font-detector so the subsequent single-engine steps
   * (OCR / Inpaint / Render) find their inputs already on the page. These
   * buttons deliberately redo their step on the current page (Detect
   * replaces the page's boxes); Process in the menu fills in only what's
   * missing.
   */
  const runStep = async (
    pick: (p: NonNullable<Awaited<ReturnType<typeof getConfig>>['pipeline']>) => string[],
  ) => {
    if (!pageId) return
    await awaitPendingSceneEdits()
    const cfg = await getConfig()
    if (!cfg.pipeline) return
    const steps = pick(cfg.pipeline).filter((s): s is string => !!s)
    if (steps.length === 0) return
    const editor = useEditorUiStore.getState()
    const prefs = usePreferencesStore.getState()
    await startPipeline({
      steps,
      pages: [pageId],
      targetLanguage: editor.selectedLanguage,
      sourceLanguage: prefs.ocrLanguage,
      systemPrompt: prefs.customSystemPrompt,
      // Shared render defaults (font, size, padding, shader): manual Render
      // must match what auto-render produces, or the page changes on click.
      ...renderDefaultsForPipeline(),
      readingOrder: editor.readingOrder === 'custom' ? undefined : editor.readingOrder,
    })
  }

  type PipelinePick = (
    p: NonNullable<Awaited<ReturnType<typeof getConfig>>['pipeline']>,
  ) => string[]
  const detectChain: PipelinePick = (p) => [
    p.detector!,
    p.segmenter!,
    p.bubble_segmenter!,
    p.font_detector!,
  ]
  const ocrChain: PipelinePick = (p) => [p.ocr!]
  const translateChain: PipelinePick = (p) => [p.translator!]
  const inpaintChain: PipelinePick = (p) => [p.inpainter!]
  const renderChain: PipelinePick = (p) => [p.renderer!]

  const isDetecting = currentStep === 'detect'
  const isOcr = currentStep === 'ocr'
  const isInpainting = currentStep === 'inpaint'
  const isTranslating = currentStep === 'llmGenerate'
  const isRendering = currentStep === 'render'

  return (
    <div className='flex flex-wrap items-center gap-0.5'>
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(detectChain)}
        data-testid='toolbar-detect'
        title={t('processing.redoDetect', 'Detect text on this page again (replaces its boxes)')}
        disabled={!hasPage || isProcessing}
      >
        {isDetecting ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <ScanIcon className='size-4' />
        )}
        {t('processing.detect')}
      </Button>
      <Separator orientation='vertical' className='mx-0.5 h-4' />
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(ocrChain)}
        data-testid='toolbar-ocr'
        title={t('processing.redoOcr', 'Read every box on this page again (replaces its OCR text)')}
        disabled={!hasPage || isProcessing}
      >
        {isOcr ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <ScanTextIcon className='size-4' />
        )}
        {t('processing.ocr')}
      </Button>
      <Separator orientation='vertical' className='mx-0.5 h-4' />
      <Button
        variant='ghost'
        size='xs'
        onClick={() =>
          // No model loaded: take the user to where it's chosen and loaded.
          llmReady
            ? void runStep(translateChain)
            : useEditorUiStore.getState().openSettings('translation')
        }
        disabled={!hasPage || isProcessing}
        data-testid='toolbar-translate'
        data-llm-ready={llmReady ? 'true' : 'false'}
        title={
          llmReady
            ? t('llm.redoTranslate', 'Translate every box on this page again')
            : llmLoading
              ? t('llm.translateLoading', 'The translation model is still loading')
              : t('llm.translateNoModel', 'No translation model loaded: click to choose one')
        }
        className='relative'
      >
        {isTranslating || llmLoading ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <LanguagesIcon className='size-4' />
        )}
        {t('llm.translate', 'Translate')}
        {!llmReady && !llmLoading && (
          <span
            data-testid='toolbar-translate-no-model'
            className='absolute top-0.5 right-0.5 size-1.5 rounded-full bg-amber-400'
          />
        )}
      </Button>
      <Separator orientation='vertical' className='mx-0.5 h-4' />
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(inpaintChain)}
        data-testid='toolbar-inpaint'
        title={t('processing.redoInpaint', 'Clean this page again')}
        disabled={!hasPage || isProcessing}
      >
        {isInpainting ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <Wand2Icon className='size-4' />
        )}
        {t('mask.inpaint')}
      </Button>
      <Separator orientation='vertical' className='mx-0.5 h-4' />
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(renderChain)}
        data-testid='toolbar-render'
        title={t('processing.redoRender', 'Render this page again')}
        disabled={!hasPage || isProcessing}
      >
        {isRendering ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <TypeIcon className='size-4' />
        )}
        {t('llm.render')}
      </Button>
    </div>
  )
}
