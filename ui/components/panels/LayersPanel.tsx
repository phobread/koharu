'use client'

import { useTranslation } from 'react-i18next'

import { Switch } from '@/components/ui/switch'
import { findImageBlob, findMaskBlob, useCurrentPage, useTextNodes } from '@/hooks/useCurrentPage'
import { useScene } from '@/hooks/useScene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { cn } from '@/lib/utils'

/**
 * Replaces the old Photoshop-style layer list: the page images (original /
 * inpainted / rendered) stack opaquely, so only one is ever really visible —
 * a single "View" choice models that honestly. The overlays that genuinely
 * combine with any view are simple on/off switches below it.
 */

type ViewId = 'original' | 'cleaned' | 'translated'

export function LayersPanel() {
  const { t } = useTranslation()
  const page = useCurrentPage()
  const { epoch: sceneEpoch } = useScene()
  const textNodes = useTextNodes()
  const showInpaintedImage = useEditorUiStore((s) => s.showInpaintedImage)
  const setShowInpaintedImage = useEditorUiStore((s) => s.setShowInpaintedImage)
  const showSegmentationMask = useEditorUiStore((s) => s.showSegmentationMask)
  const setShowSegmentationMask = useEditorUiStore((s) => s.setShowSegmentationMask)
  const showBrushLayer = useEditorUiStore((s) => s.showBrushLayer)
  const setShowBrushLayer = useEditorUiStore((s) => s.setShowBrushLayer)
  const showTextBlocksOverlay = useEditorUiStore((s) => s.showTextBlocksOverlay)
  const setShowTextBlocksOverlay = useEditorUiStore((s) => s.setShowTextBlocksOverlay)
  const showRenderedImage = useEditorUiStore((s) => s.showRenderedImage)
  const setShowRenderedImage = useEditorUiStore((s) => s.setShowRenderedImage)

  const hasRendered = !!(page && findImageBlob(page, 'rendered'))
  const hasInpainted = !!(page && findImageBlob(page, 'inpainted'))
  const hasSegment = !!(page && findMaskBlob(page, 'segment'))
  const hasBrush = !!(page && findMaskBlob(page, 'brushInpaint'))
  // Silence warning about unused epoch dep — it's the invalidation trigger.
  void sceneEpoch

  const view: ViewId = showRenderedImage
    ? 'translated'
    : showInpaintedImage
      ? 'cleaned'
      : 'original'
  const setView = (next: ViewId) => {
    setShowRenderedImage(next === 'translated')
    setShowInpaintedImage(next !== 'original')
  }

  const views: { id: ViewId; label: string; enabled: boolean }[] = [
    { id: 'original', label: t('layers.viewOriginal'), enabled: true },
    { id: 'cleaned', label: t('layers.viewCleaned'), enabled: hasInpainted },
    { id: 'translated', label: t('layers.viewTranslated'), enabled: hasRendered },
  ]

  const overlays = [
    {
      id: 'textBlocks',
      label: t('layers.textBlocks'),
      checked: showTextBlocksOverlay,
      setChecked: setShowTextBlocksOverlay,
      enabled: textNodes.length > 0,
    },
    {
      id: 'mask',
      label: t('layers.mask'),
      checked: showSegmentationMask,
      setChecked: setShowSegmentationMask,
      enabled: hasSegment,
    },
    {
      id: 'brush',
      label: t('layers.brush'),
      checked: showBrushLayer,
      setChecked: setShowBrushLayer,
      enabled: hasBrush,
    },
  ]

  return (
    <div className='flex flex-col gap-3 px-2 pt-2'>
      <div
        className='grid grid-cols-3 gap-0.5 rounded-md border border-border bg-muted/60 p-0.5'
        role='radiogroup'
        aria-label={t('layers.title')}
      >
        {views.map((v) => (
          <button
            key={v.id}
            type='button'
            role='radio'
            aria-checked={view === v.id}
            data-testid={`view-${v.id}`}
            disabled={!v.enabled}
            onClick={() => setView(v.id)}
            className={cn(
              'rounded-[5px] px-1 py-1 text-xs transition-colors',
              view === v.id
                ? 'bg-background font-medium text-foreground shadow-sm'
                : 'text-muted-foreground',
              v.enabled ? 'cursor-pointer hover:text-foreground' : 'cursor-default opacity-40',
            )}
          >
            {v.label}
          </button>
        ))}
      </div>

      <div className='flex flex-col gap-1'>
        {overlays.map((o) => (
          <label
            key={o.id}
            data-testid={`overlay-${o.id}`}
            className={cn(
              'flex cursor-pointer items-center justify-between gap-2 rounded px-1 py-1',
              !o.enabled && 'cursor-default opacity-40',
            )}
          >
            <span
              className={cn('text-xs', o.checked ? 'text-foreground' : 'text-muted-foreground')}
            >
              {o.label}
            </span>
            <Switch
              checked={o.checked}
              disabled={!o.enabled}
              onCheckedChange={o.setChecked}
              className='scale-90'
            />
          </label>
        ))}
      </div>
    </div>
  )
}
