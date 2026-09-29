'use client'

import {
  ImageIcon,
  MinusIcon,
  PlusIcon,
  RotateCcwIcon,
  ShrinkIcon,
  XIcon,
  ZoomInIcon,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { fitCanvasToViewport, zoomCanvasToBox } from '@/components/canvas/canvasViewport'
import { Button } from '@/components/ui/button'
import { RichTextDraftTextarea } from '@/components/ui/rich-text-draft-textarea'
import { SplittableDraftTextarea } from '@/components/ui/splittable-draft-textarea'
import type { TextNodeEntry } from '@/hooks/useCurrentPage'
import type { Page, TextDataPatch } from '@/lib/api/schemas'
import { applyOp, applyOpFromScene, queueAutoRender } from '@/lib/io/scene'
import { splitBlock } from '@/lib/io/splitNode'
import { ops } from '@/lib/ops'
import type { SplitField } from '@/lib/splitBlock'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { effectiveTextColor, mergeTextStyle } from '@/lib/textStyle'

const EDITOR_WIDTH = 240
const EDITOR_GAP = 10
/** Room to keep free beside a zoomed-to box so the editor doesn't cover it. */
export const QUICK_EDITOR_RESERVE = EDITOR_WIDTH + EDITOR_GAP * 2
const EDITOR_APPROX_HEIGHT = 230
const MIN_FONT_SIZE = 6
const MAX_FONT_SIZE = 300

/**
 * Small floating editor anchored beside the selected block: the OCR'd source
 * and its translation right next to the box, so misrecognised text is easy
 * to spot and fix in place without hunting through the side panel.
 */
export function BlockQuickEditor({
  page,
  node,
  index,
  scale,
  showOriginal,
  onToggleOriginal,
  onClose,
}: {
  page: Page
  node: TextNodeEntry
  index: number
  scale: number
  showOriginal: boolean
  onToggleOriginal: () => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  // Zoomed away from the whole-page fit (by this button or by hand).
  const zoomed = !useEditorUiStore((s) => s.autoFitEnabled)
  const box = node.transform
  // Prefer the right side of the box; flip to the left when that would run
  // off the page. Top tracks the box, clamped so the editor stays visible.
  const rightX = (box.x + box.width) * scale + EDITOR_GAP
  const fitsRight = rightX + EDITOR_WIDTH <= page.width * scale
  const left = fitsRight ? rightX : Math.max(0, box.x * scale - EDITOR_WIDTH - EDITOR_GAP)
  const top = Math.max(0, Math.min(box.y * scale, page.height * scale - EDITOR_APPROX_HEIGHT))

  const patch = (p: TextDataPatch) => {
    void (async () => {
      await applyOp(ops.updateNode(page.id, node.id, { data: { text: p } as never }))
      queueAutoRender(page.id)
    })()
  }

  // Same caret split as the side panel's textareas: right-click at the split
  // point → the block divides along its text flow. The editor closes itself
  // afterwards because the edited node is replaced by the two halves.
  const splitAt = (field: SplitField, offset: number) => {
    void splitBlock(page.id, node.id, { field, offset })
  }

  // Slant: rotation about the box centre, matching the canvas outline and
  // the baked-in sprite rotation. Draft state commits on blur / Enter so
  // typing "-1" doesn't fire a re-render at "-".
  const rotation = box.rotationDeg ?? 0
  const [slantDraft, setSlantDraft] = useState(String(Math.round(rotation * 10) / 10))
  useEffect(() => {
    setSlantDraft(String(Math.round(rotation * 10) / 10))
  }, [rotation, node.id])

  const commitSlant = (raw: string) => {
    const parsed = Number.parseFloat(raw)
    if (!Number.isFinite(parsed)) {
      setSlantDraft(String(Math.round(rotation * 10) / 10))
      return
    }
    const deg = Math.max(-180, Math.min(180, Math.round(parsed * 10) / 10))
    setSlantDraft(String(deg))
    if (deg === rotation) return
    void (async () => {
      await applyOp(
        ops.updateNode(page.id, node.id, {
          transform: { ...box, rotationDeg: deg },
          // Keep the user's box footprint: a slant tweak must not hand the
          // block back to bubble-fit expansion.
          data: { text: { lockLayoutBox: true } } as never,
        }),
      )
      queueAutoRender(page.id)
    })()
  }

  // Size: the block's own override, else what the renderer fitted ("auto").
  // Built from the latest saved scene so quick repeated clicks add up.
  const fontSize = node.data.style?.fontSize ?? undefined
  const renderedSize = node.data.renderedFontSizePx ?? undefined
  const [sizeDraft, setSizeDraft] = useState(
    fontSize !== undefined ? String(Math.round(fontSize)) : '',
  )
  useEffect(() => {
    setSizeDraft(fontSize !== undefined ? String(Math.round(fontSize)) : '')
  }, [fontSize, node.id])

  const setFontSize = (next: (current: number) => number | null) => {
    // The size change is only visible on the rendered text, not the original.
    if (showOriginal) onToggleOriginal()
    void (async () => {
      const applied = await applyOpFromScene((scene) => {
        const current = scene.pages[page.id]?.nodes[node.id]
        if (!current || !('text' in current.kind)) return null
        const text = current.kind.text
        const base = text.style?.fontSize ?? text.renderedFontSizePx ?? 16
        const size = next(base)
        const clamped =
          size === null ? null : Math.max(MIN_FONT_SIZE, Math.min(MAX_FONT_SIZE, Math.round(size)))
        if (clamped === (text.style?.fontSize ?? null)) return null
        return ops.updateNode(page.id, node.id, {
          data: { text: { style: mergeTextStyle(text.style, { fontSize: clamped }) } } as never,
        })
      })
      if (applied) queueAutoRender(page.id)
    })()
  }

  const commitSizeDraft = (raw: string) => {
    const value = raw.trim()
    if (value === '') {
      setFontSize(() => null)
      return
    }
    const parsed = Number.parseInt(value, 10)
    if (!Number.isFinite(parsed) || parsed < 1) {
      setSizeDraft(fontSize !== undefined ? String(Math.round(fontSize)) : '')
      return
    }
    setFontSize(() => parsed)
  }

  return (
    <div
      data-testid='block-quick-editor'
      className='absolute flex flex-col gap-1.5 rounded-lg border border-border bg-popover/95 p-2 shadow-lg backdrop-blur-sm'
      style={{ left, top, width: EDITOR_WIDTH, zIndex: 40, pointerEvents: 'auto' }}
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <div className='flex items-center justify-between'>
        <span className='text-[10px] font-semibold tracking-wide text-muted-foreground uppercase'>
          #{index + 1}
        </span>
        <div className='flex items-center gap-1'>
          <Button
            variant='ghost'
            size='icon-xs'
            className='size-4 text-muted-foreground hover:text-foreground'
            aria-label={t('textBlocks.zoomToBox', 'Zoom to this box')}
            title={t('textBlocks.zoomToBox', 'Zoom to this box')}
            data-testid='quick-editor-zoom'
            onClick={() => zoomCanvasToBox(box, QUICK_EDITOR_RESERVE)}
          >
            <ZoomInIcon className='size-3' />
          </Button>
          {zoomed && (
            <Button
              variant='ghost'
              size='icon-xs'
              className='size-4 text-muted-foreground hover:text-foreground'
              aria-label={t('textBlocks.fitPage', 'Back to the whole page')}
              title={t('textBlocks.fitPage', 'Back to the whole page')}
              data-testid='quick-editor-fit'
              onClick={fitCanvasToViewport}
            >
              <ShrinkIcon className='size-3' />
            </Button>
          )}
          <Button
            variant='ghost'
            size='icon-xs'
            className={
              showOriginal
                ? 'size-4 bg-primary/15 text-primary hover:text-primary'
                : 'size-4 text-muted-foreground hover:text-foreground'
            }
            aria-label={t('textBlocks.peekOriginal')}
            aria-pressed={showOriginal}
            title={t('textBlocks.peekOriginal')}
            data-testid='quick-editor-peek-toggle'
            onClick={onToggleOriginal}
          >
            <ImageIcon className='size-3' />
          </Button>
          <Button
            variant='ghost'
            size='icon-xs'
            className='size-4 text-muted-foreground hover:text-foreground'
            aria-label={t('common.close', { defaultValue: 'Close' })}
            data-testid='quick-editor-close'
            onClick={onClose}
          >
            <XIcon className='size-3' />
          </Button>
        </div>
      </div>
      <div className='flex flex-col gap-0.5'>
        <span className='text-[10px] text-muted-foreground uppercase'>
          {t('textBlocks.ocrLabel')}
        </span>
        <SplittableDraftTextarea
          data-testid='quick-editor-ocr'
          value={node.data.text ?? ''}
          placeholder={t('textBlocks.addOcrPlaceholder')}
          rows={2}
          onValueChange={(value) => patch({ text: value })}
          className='min-h-0 resize-none bg-background px-1.5 py-1 text-xs'
          splitLabel={t('textBlocks.splitAtCursor')}
          onSplit={(offset) => splitAt('text', offset)}
        />
      </div>
      <div className='flex flex-col gap-0.5'>
        <span className='text-[10px] text-muted-foreground uppercase'>
          {t('textBlocks.translationLabel')}
        </span>
        <RichTextDraftTextarea
          data-testid='quick-editor-translation'
          value={node.data.translation ?? ''}
          styleRanges={node.data.styleRanges ?? []}
          inheritedColor={effectiveTextColor(node.data.style, node.data.renderedTextColor)}
          inheritedEffect={node.data.style?.effect}
          placeholder={t('textBlocks.addTranslationPlaceholder')}
          rows={2}
          onPatch={patch}
          className='min-h-0 resize-none bg-background px-1.5 py-1 text-xs'
          splitLabel={t('textBlocks.splitAtCursor')}
          onSplit={(offset) => splitAt('translation', offset)}
        />
      </div>
      <div className='flex items-center gap-1.5'>
        <span className='flex-1 text-[10px] text-muted-foreground uppercase'>
          {t('render.fontSizeLabel')}
        </span>
        <div className='flex items-center rounded-md border border-input bg-background'>
          <Button
            type='button'
            variant='ghost'
            size='icon-xs'
            className='size-6 rounded-r-none'
            aria-label={t('textBlocks.smaller', 'Smaller')}
            data-testid='quick-editor-size-down'
            onClick={() => setFontSize((size) => size - 1)}
          >
            <MinusIcon className='size-3' />
          </Button>
          <input
            data-testid='quick-editor-size'
            type='number'
            step={1}
            min={MIN_FONT_SIZE}
            max={MAX_FONT_SIZE}
            inputMode='numeric'
            value={sizeDraft}
            placeholder={renderedSize !== undefined ? `auto (${Math.round(renderedSize)})` : 'auto'}
            onChange={(e) => setSizeDraft(e.target.value)}
            onBlur={(e) => commitSizeDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commitSizeDraft((e.target as HTMLInputElement).value)
            }}
            className='h-6 w-16 [appearance:textfield] border-x border-input bg-transparent px-1 text-center text-xs outline-none [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'
          />
          <Button
            type='button'
            variant='ghost'
            size='icon-xs'
            className='size-6 rounded-l-none'
            aria-label={t('textBlocks.larger', 'Larger')}
            data-testid='quick-editor-size-up'
            onClick={() => setFontSize((size) => size + 1)}
          >
            <PlusIcon className='size-3' />
          </Button>
        </div>
        <Button
          variant='ghost'
          size='icon-xs'
          className='size-5 text-muted-foreground hover:text-foreground'
          aria-label={t('render.resetToAuto')}
          title={t('render.resetToAuto')}
          data-testid='quick-editor-size-auto'
          disabled={fontSize === undefined}
          onClick={() => setFontSize(() => null)}
        >
          <RotateCcwIcon className='size-3' />
        </Button>
      </div>
      <div className='flex items-center gap-1.5'>
        <span className='flex-1 text-[10px] text-muted-foreground uppercase'>
          {t('textBlocks.slant')}
        </span>
        <input
          data-testid='quick-editor-slant'
          type='number'
          step={1}
          min={-180}
          max={180}
          value={slantDraft}
          onChange={(e) => setSlantDraft(e.target.value)}
          onBlur={(e) => commitSlant(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commitSlant((e.target as HTMLInputElement).value)
          }}
          className='h-6 w-16 rounded-md border border-input bg-background px-1.5 text-right text-xs outline-none focus-visible:border-ring'
        />
        <span className='text-[10px] text-muted-foreground'>°</span>
        <Button
          variant='ghost'
          size='icon-xs'
          className='size-5 text-muted-foreground hover:text-foreground'
          aria-label={t('textBlocks.straighten')}
          title={t('textBlocks.straighten')}
          data-testid='quick-editor-straighten'
          disabled={rotation === 0}
          onClick={() => commitSlant('0')}
        >
          <span className='text-[10px] font-semibold'>0°</span>
        </Button>
      </div>
    </div>
  )
}
