'use client'

import { BoldIcon, ItalicIcon, RemoveFormattingIcon } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { ColorPicker } from '@/components/ui/color-picker'
import { SplittableDraftTextarea } from '@/components/ui/splittable-draft-textarea'
import type { TextDataPatch, TextShaderEffect, TextStyleRange } from '@/lib/api/schemas'
import {
  applyTextRangeStyle,
  rebaseTextStyleRanges,
  selectionHasStyle,
  styleAtOffset,
  utf16OffsetToUtf8,
} from '@/lib/richText'
import { cn } from '@/lib/utils'

type RichTextDraftTextareaProps = {
  value: string
  styleRanges: TextStyleRange[]
  inheritedColor: number[]
  inheritedEffect?: TextShaderEffect | null
  onPatch: (patch: TextDataPatch) => void
  splitLabel: string
  onSplit: (offset: number) => void
  rows?: number
  placeholder?: string
  className?: string
  'data-testid'?: string
}

const colorToHex = (color: number[]) =>
  `#${color
    .slice(0, 3)
    .map((value) =>
      Math.max(0, Math.min(255, Math.round(value)))
        .toString(16)
        .padStart(2, '0'),
    )
    .join('')}`

const hexToColor = (value: string, alpha = 255): number[] => {
  const hex = value.replace('#', '')
  if (hex.length !== 6) return [0, 0, 0, alpha]
  return [
    Number.parseInt(hex.slice(0, 2), 16),
    Number.parseInt(hex.slice(2, 4), 16),
    Number.parseInt(hex.slice(4, 6), 16),
    alpha,
  ]
}

const snapshotKey = (value: string, ranges: TextStyleRange[]) =>
  JSON.stringify([
    value,
    ranges.map(({ start, end, style }) => [
      start,
      end,
      style.color ?? null,
      style.bold ?? null,
      style.italic ?? null,
    ]),
  ])

/** Plain-text editing plus selection-aware character formatting. The stored
 * translation remains plain text; styling lives in UTF-8 ranges beside it. */
export function RichTextDraftTextarea({
  value,
  styleRanges,
  inheritedColor,
  inheritedEffect,
  onPatch,
  splitLabel,
  onSplit,
  ...textareaProps
}: RichTextDraftTextareaProps) {
  const { t } = useTranslation()
  const valueRef = useRef(value)
  const rangesRef = useRef(styleRanges)
  const externalKeyRef = useRef(snapshotKey(value, styleRanges))
  const pendingKeysRef = useRef<string[]>([])
  const [selection, setSelection] = useState({ start: 0, end: 0 })
  const [revision, setRevision] = useState(0)

  useEffect(() => {
    const key = snapshotKey(value, styleRanges)
    if (key === externalKeyRef.current) return
    externalKeyRef.current = key
    const pendingIndex = pendingKeysRef.current.indexOf(key)
    if (pendingIndex >= 0) {
      pendingKeysRef.current.splice(0, pendingIndex + 1)
      // An earlier queued edit was saved; retain the newer local draft and ranges.
      if (pendingKeysRef.current.length) return
    } else {
      // A different external change (including undo/redo) replaces our draft state.
      pendingKeysRef.current = []
      setSelection({ start: 0, end: 0 })
    }
    valueRef.current = value
    rangesRef.current = styleRanges
    setRevision((current) => current + 1)
  }, [value, styleRanges])

  const hasSelection = selection.start < selection.end
  const selectionStyle = styleAtOffset(rangesRef.current, selection.start)
  const selectedColor = selectionStyle.color ?? inheritedColor
  const selectionBold = selectionHasStyle(
    rangesRef.current,
    selection.start,
    selection.end,
    'bold',
    inheritedEffect?.bold ?? false,
  )
  const selectionItalic = selectionHasStyle(
    rangesRef.current,
    selection.start,
    selection.end,
    'italic',
    inheritedEffect?.italic ?? false,
  )
  void revision

  const commitRanges = (next: TextStyleRange[]) => {
    rangesRef.current = next
    pendingKeysRef.current.push(snapshotKey(valueRef.current, next))
    setRevision((current) => current + 1)
    onPatch({ styleRanges: next })
  }

  const applyStyle = (update: Parameters<typeof applyTextRangeStyle>[3]) => {
    if (!hasSelection) return
    commitRanges(applyTextRangeStyle(rangesRef.current, selection.start, selection.end, update))
  }

  const recordSelection = (element: HTMLTextAreaElement) => {
    const text = valueRef.current
    setSelection({
      start: utf16OffsetToUtf8(text, element.selectionStart),
      end: utf16OffsetToUtf8(text, element.selectionEnd),
    })
  }

  return (
    <div className='flex flex-col gap-1'>
      <div className='flex min-h-5 items-center gap-0.5' data-testid='rich-text-toolbar'>
        <span className='mr-auto truncate text-[9px] text-muted-foreground'>
          {hasSelection
            ? t('textBlocks.formatSelection', { defaultValue: 'Format selection' })
            : t('textBlocks.selectToFormat', { defaultValue: 'Select text to format' })}
        </span>
        <Button
          type='button'
          variant='ghost'
          size='icon-xs'
          className={cn('size-5', selectionBold && hasSelection && 'bg-primary/15 text-primary')}
          disabled={!hasSelection}
          aria-label={t('render.effectBold')}
          aria-pressed={selectionBold}
          onClick={() => applyStyle({ bold: !selectionBold })}
        >
          <BoldIcon className='size-3' />
        </Button>
        <Button
          type='button'
          variant='ghost'
          size='icon-xs'
          className={cn('size-5', selectionItalic && hasSelection && 'bg-primary/15 text-primary')}
          disabled={!hasSelection}
          aria-label={t('render.effectItalic')}
          aria-pressed={selectionItalic}
          onClick={() => applyStyle({ italic: !selectionItalic })}
        >
          <ItalicIcon className='size-3' />
        </Button>
        <ColorPicker
          value={colorToHex(selectedColor)}
          disabled={!hasSelection}
          className='size-5'
          aria-label={t('textBlocks.selectionColor', { defaultValue: 'Selection color' })}
          onChange={(hex) => applyStyle({ color: hexToColor(hex, selectedColor[3] ?? 255) })}
        />
        <Button
          type='button'
          variant='ghost'
          size='icon-xs'
          className='size-5'
          disabled={!hasSelection}
          aria-label={t('textBlocks.clearSelectionFormatting', {
            defaultValue: 'Clear selection formatting',
          })}
          onClick={() => applyStyle({ color: null, bold: null, italic: null })}
        >
          <RemoveFormattingIcon className='size-3' />
        </Button>
      </div>
      <SplittableDraftTextarea
        {...textareaProps}
        value={valueRef.current}
        syncWhileFocused
        splitLabel={splitLabel}
        onSplit={onSplit}
        onSelect={(event) => recordSelection(event.currentTarget)}
        onValueChange={(nextValue, element) => {
          const nextRanges = rebaseTextStyleRanges(valueRef.current, nextValue, rangesRef.current)
          valueRef.current = nextValue
          rangesRef.current = nextRanges
          recordSelection(element)
          pendingKeysRef.current.push(snapshotKey(nextValue, nextRanges))
          setRevision((current) => current + 1)
          onPatch({ translation: nextValue, styleRanges: nextRanges })
        }}
      />
    </div>
  )
}
