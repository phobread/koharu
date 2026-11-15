'use client'

import { EyeIcon } from 'lucide-react'
import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { findImageBlob, findMaskBlob, useCurrentPage, useTextNodes } from '@/hooks/useCurrentPage'
import { useScene } from '@/hooks/useScene'
import { formatShortcutForDisplay, getPlatform } from '@/lib/shortcutUtils'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import { cn } from '@/lib/utils'

/**
 * What the canvas shows, next to the step buttons. The page images (original
 * / cleaned / translated) stack opaquely, so only one is ever really visible:
 * a single switch models that honestly. The overlays that combine with any
 * view (text boxes, detected text, brush strokes) sit behind the eye button.
 *
 * Release (the official release in place of the raw page) is offered only in
 * projects that have one, and works only on pages that have one. Elsewhere
 * the page shows as Original, and Release comes back on the next page that
 * has a release.
 */

type ViewId = 'original' | 'cleaned' | 'translated' | 'release'

export function ViewControls() {
  const { t } = useTranslation()
  const page = useCurrentPage()
  const { scene, epoch: sceneEpoch } = useScene()
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
  const showOfficialImage = useEditorUiStore((s) => s.showOfficialImage)
  const setShowOfficialImage = useEditorUiStore((s) => s.setShowOfficialImage)
  const toggleTextBoxesKey = usePreferencesStore((s) => s.shortcuts.toggleTextBoxes)

  const hasRendered = !!(page && findImageBlob(page, 'rendered'))
  const hasInpainted = !!(page && findImageBlob(page, 'inpainted'))
  const hasOfficial = !!(page && findImageBlob(page, 'official'))
  const hasSegment = !!(page && findMaskBlob(page, 'segment'))
  const hasBrush = !!(page && findMaskBlob(page, 'brushInpaint'))
  const projectHasOfficial = useMemo(
    () => Object.values(scene?.pages ?? {}).some((p) => findImageBlob(p, 'official') !== null),
    [scene],
  )
  // Silence warning about unused epoch dep — it's the invalidation trigger.
  void sceneEpoch

  const view: ViewId =
    showOfficialImage && hasOfficial
      ? 'release'
      : showRenderedImage
        ? 'translated'
        : showInpaintedImage
          ? 'cleaned'
          : 'original'
  const setView = (next: ViewId) => {
    if (next === 'release') {
      setShowOfficialImage(true)
      return
    }
    setShowOfficialImage(false)
    setShowRenderedImage(next === 'translated')
    setShowInpaintedImage(next !== 'original')
  }

  const views: { id: ViewId; label: string; enabled: boolean; title?: string }[] = [
    { id: 'original', label: t('layers.viewOriginal'), enabled: true },
    { id: 'cleaned', label: t('layers.viewCleaned'), enabled: hasInpainted },
    { id: 'translated', label: t('layers.viewTranslated'), enabled: hasRendered },
  ]
  if (projectHasOfficial) {
    views.push({
      id: 'release',
      label: t('layers.viewRelease', 'Release'),
      enabled: hasOfficial,
      title: hasOfficial
        ? t('layers.viewReleaseTitle', 'The official release of this page')
        : t('layers.viewReleaseNone', 'This page has no official release'),
    })
  }

  const overlays: {
    id: string
    label: string
    checked: boolean
    setChecked: (show: boolean) => void
    enabled: boolean
    shortcut?: string
  }[] = [
    {
      id: 'textBlocks',
      label: t('layers.textBlocks'),
      checked: showTextBlocksOverlay,
      setChecked: setShowTextBlocksOverlay,
      enabled: textNodes.length > 0,
      shortcut: formatShortcutForDisplay(toggleTextBoxesKey ?? '', getPlatform() === 'mac'),
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
  const overlaysOn = overlays.filter((o) => o.enabled && o.checked).length

  return (
    <div className='flex items-center gap-1'>
      <div
        className={cn(
          'grid gap-0.5 rounded-md border border-border bg-muted/60 p-0.5',
          views.length > 3 ? 'grid-cols-4' : 'grid-cols-3',
        )}
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
            title={v.title}
            onClick={() => setView(v.id)}
            className={cn(
              'rounded-[5px] px-2 py-0.5 text-xs transition-colors',
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
      <Popover>
        <PopoverTrigger asChild>
          <Button
            variant='ghost'
            size='xs'
            data-testid='view-overlays'
            data-active={overlaysOn > 0}
            className='text-muted-foreground data-[active=true]:bg-primary/10 data-[active=true]:text-primary'
            title={t('layers.overlays', 'Show on the page')}
            aria-label={t('layers.overlays', 'Show on the page')}
          >
            <EyeIcon className='size-4' />
          </Button>
        </PopoverTrigger>
        <PopoverContent align='end' className='w-52 p-1.5' data-testid='view-overlays-popover'>
          {overlays.map((o) => (
            <label
              key={o.id}
              data-testid={`overlay-${o.id}`}
              className={cn(
                'flex cursor-pointer items-center justify-between gap-2 rounded px-1.5 py-1',
                !o.enabled && 'cursor-default opacity-40',
              )}
            >
              <span
                className={cn('text-xs', o.checked ? 'text-foreground' : 'text-muted-foreground')}
              >
                {o.label}
                {o.shortcut && (
                  <kbd className='ml-1.5 rounded border border-border px-1 font-sans text-[10px] text-muted-foreground'>
                    {o.shortcut}
                  </kbd>
                )}
              </span>
              <Switch
                checked={o.checked}
                disabled={!o.enabled}
                onCheckedChange={o.setChecked}
                className='scale-90'
              />
            </label>
          ))}
        </PopoverContent>
      </Popover>
    </div>
  )
}
