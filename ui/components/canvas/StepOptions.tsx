'use client'

/**
 * The small arrows beside the toolbar's step buttons. Each opens that step's
 * settings where the work happens, instead of a trip through Settings.
 * These are shortcuts: they read and write the same config and stores as the
 * Settings dialog, which stays as it is.
 */

import { ChevronDownIcon } from 'lucide-react'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import { TranslationSettings } from '@/components/TranslationSettings'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import {
  getGetConfigQueryKey,
  patchConfig,
  useGetConfig,
  useGetEngineCatalog,
} from '@/lib/api/default/default'
import type { EngineCatalog, PipelineConfig, PipelineConfigPatch } from '@/lib/api/schemas'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

function StepOptionsPopover({
  step,
  label,
  className,
  children,
}: {
  step: string
  label: string
  className?: string
  children: ReactNode
}) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant='ghost'
          size='icon-xs'
          className='-ml-1 w-4 text-muted-foreground'
          data-testid={`toolbar-${step}-options`}
          aria-label={label}
          title={label}
        >
          <ChevronDownIcon className='size-3' />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align='start'
        className={className ?? 'w-72 space-y-3 p-3'}
        data-testid={`toolbar-${step}-options-panel`}
      >
        {children}
      </PopoverContent>
    </Popover>
  )
}

/** Saves part of the pipeline config and refreshes everything that reads it. */
async function savePipeline(patch: PipelineConfigPatch) {
  try {
    await patchConfig({ pipeline: patch })
  } catch (e) {
    useEditorUiStore.getState().showError(String(e))
  } finally {
    await queryClient.invalidateQueries({ queryKey: getGetConfigQueryKey() })
  }
}

type EngineKey =
  | 'detector'
  | 'segmenter'
  | 'bubble_segmenter'
  | 'font_detector'
  | 'ocr'
  | 'inpainter'

const ENGINE_FIELDS: Record<
  EngineKey,
  { label: string; patch: keyof PipelineConfigPatch; catalog: keyof EngineCatalog }
> = {
  detector: { label: 'settings.detector', patch: 'detector', catalog: 'detectors' },
  segmenter: { label: 'settings.segmenter', patch: 'segmenter', catalog: 'segmenters' },
  bubble_segmenter: {
    label: 'settings.bubbleSegmenter',
    patch: 'bubbleSegmenter',
    catalog: 'bubbleSegmenters',
  },
  font_detector: {
    label: 'settings.fontDetector',
    patch: 'fontDetector',
    catalog: 'fontDetectors',
  },
  ocr: { label: 'settings.ocr', patch: 'ocr', catalog: 'ocr' },
  inpainter: { label: 'settings.inpainter', patch: 'inpainter', catalog: 'inpainters' },
}

function EnginePicker({
  engine,
  pipeline,
  catalog,
}: {
  engine: EngineKey
  pipeline: PipelineConfig
  catalog: EngineCatalog
}) {
  const { t } = useTranslation()
  const field = ENGINE_FIELDS[engine]
  const engines = catalog[field.catalog] ?? []
  return (
    <div className='space-y-1.5'>
      <Label className='text-xs'>{t(field.label)}</Label>
      <Select
        value={pipeline[engine] ?? engines[0]?.id ?? ''}
        onValueChange={(id) => void savePipeline({ [field.patch]: id })}
      >
        <SelectTrigger className='w-full' data-testid={`step-engine-${engine}`}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {engines.map((e) => (
            <SelectItem key={e.id} value={e.id}>
              {e.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  )
}

function usePipelineSettings() {
  const { data: config } = useGetConfig()
  const { data: catalog } = useGetEngineCatalog()
  return { pipeline: config?.pipeline, catalog }
}

function EngineOptions({
  step,
  label,
  engines,
}: {
  step: string
  label: string
  engines: EngineKey[]
}) {
  const { pipeline, catalog } = usePipelineSettings()
  return (
    <StepOptionsPopover step={step} label={label}>
      {pipeline && catalog
        ? engines.map((engine) => (
            <EnginePicker key={engine} engine={engine} pipeline={pipeline} catalog={catalog} />
          ))
        : null}
    </StepOptionsPopover>
  )
}

export function DetectOptions() {
  const { t } = useTranslation()
  return (
    <EngineOptions
      step='detect'
      label={t('toolbar.detectOptions', 'Detection settings')}
      engines={['detector', 'segmenter', 'bubble_segmenter', 'font_detector']}
    />
  )
}

export function OcrOptions() {
  const { t } = useTranslation()
  return (
    <EngineOptions step='ocr' label={t('toolbar.ocrOptions', 'OCR settings')} engines={['ocr']} />
  )
}

/** Model, target language and the custom prompt: the Settings pane itself. */
export function TranslateOptions() {
  const { t } = useTranslation()
  return (
    <StepOptionsPopover
      step='translate'
      label={t('toolbar.translateOptions', 'Translation settings')}
      className='max-h-[70vh] w-96 overflow-y-auto p-4'
    >
      <TranslationSettings />
    </StepOptionsPopover>
  )
}

/** The clean-up engine and, for Flux.2, its quality and shortcuts. */
export function InpaintOptions() {
  const { t } = useTranslation()
  const { pipeline, catalog } = usePipelineSettings()
  return (
    <StepOptionsPopover step='inpaint' label={t('toolbar.inpaintOptions', 'Clean-up settings')}>
      {pipeline && catalog ? (
        <>
          <EnginePicker engine='inpainter' pipeline={pipeline} catalog={catalog} />
          {pipeline.inpainter === 'flux2-klein' && (
            <>
              <div className='space-y-1.5'>
                <Label className='text-xs'>{t('settings.flux2KleinQuality')}</Label>
                <div
                  role='group'
                  aria-label={t('settings.flux2KleinQuality')}
                  className='grid grid-cols-2 gap-2'
                >
                  {[
                    { steps: 2, label: t('settings.flux2Fast') },
                    { steps: 4, label: t('settings.flux2Quality') },
                  ].map(({ steps, label }) => (
                    <Button
                      key={steps}
                      type='button'
                      variant={pipeline.flux2_steps === steps ? 'secondary' : 'outline'}
                      size='sm'
                      aria-pressed={pipeline.flux2_steps === steps}
                      data-testid={`step-flux2-steps-${steps}`}
                      onClick={() => void savePipeline({ flux2Steps: steps })}
                    >
                      {label}
                    </Button>
                  ))}
                </div>
              </div>
              <div className='flex items-center justify-between gap-3'>
                <Label htmlFor='step-flux2-flat-fill' className='text-xs'>
                  {t('settings.flux2FlatFill')}
                </Label>
                <Switch
                  id='step-flux2-flat-fill'
                  checked={pipeline.flux2_flat_fill ?? true}
                  onCheckedChange={(v) => void savePipeline({ flux2FlatFill: v })}
                />
              </div>
              <div className='flex items-center justify-between gap-3'>
                <Label htmlFor='step-flux2-fast-large-crops' className='text-xs'>
                  {t('settings.flux2FastLargeCrops')}
                </Label>
                <Switch
                  id='step-flux2-fast-large-crops'
                  checked={pipeline.flux2_fast_large_crops ?? true}
                  onCheckedChange={(v) => void savePipeline({ flux2FastLargeCrops: v })}
                />
              </div>
            </>
          )}
        </>
      ) : null}
    </StepOptionsPopover>
  )
}
