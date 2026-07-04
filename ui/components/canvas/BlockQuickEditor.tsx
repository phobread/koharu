'use client'

import { ImageIcon, XIcon } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { DraftTextarea } from '@/components/ui/draft-textarea'
import type { TextNodeEntry } from '@/hooks/useCurrentPage'
import type { Page, TextDataPatch } from '@/lib/api/schemas'
import { applyOp, queueAutoRender } from '@/lib/io/scene'
import { ops } from '@/lib/ops'

const EDITOR_WIDTH = 240
const EDITOR_GAP = 10
const EDITOR_APPROX_HEIGHT = 170

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
        <DraftTextarea
          data-testid='quick-editor-ocr'
          value={node.data.text ?? ''}
          placeholder={t('textBlocks.addOcrPlaceholder')}
          rows={2}
          onValueChange={(value) => patch({ text: value })}
          className='min-h-0 resize-none bg-background px-1.5 py-1 text-xs'
        />
      </div>
      <div className='flex flex-col gap-0.5'>
        <span className='text-[10px] text-muted-foreground uppercase'>
          {t('textBlocks.translationLabel')}
        </span>
        <DraftTextarea
          data-testid='quick-editor-translation'
          value={node.data.translation ?? ''}
          placeholder={t('textBlocks.addTranslationPlaceholder')}
          rows={2}
          onValueChange={(value) => patch({ translation: value })}
          className='min-h-0 resize-none bg-background px-1.5 py-1 text-xs'
        />
      </div>
    </div>
  )
}
