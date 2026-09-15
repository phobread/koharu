import type { TextRangeStyle, TextStyleRange } from '@/lib/api/schemas'

const utf8Length = (value: string): number => new TextEncoder().encode(value).length

/** Convert the textarea's UTF-16 selection offset to the UTF-8 byte offsets
 * stored in the scene and emitted by HarfRust glyph clusters. */
export const utf16OffsetToUtf8 = (text: string, offset: number): number =>
  utf8Length(text.slice(0, Math.max(0, Math.min(offset, text.length))))

const styleIsEmpty = (style: TextRangeStyle): boolean =>
  style.color == null && style.bold == null && style.italic == null

const sameColor = (a?: number[] | null, b?: number[] | null): boolean =>
  a == null || b == null
    ? a == null && b == null
    : a.length === b.length && a.every((v, i) => v === b[i])

const sameStyle = (a: TextRangeStyle, b: TextRangeStyle): boolean =>
  sameColor(a.color, b.color) && a.bold === b.bold && a.italic === b.italic

export const styleAtOffset = (ranges: TextStyleRange[], offset: number): TextRangeStyle => {
  const style: TextRangeStyle = {}
  for (const range of ranges) {
    if (range.start > offset || range.end <= offset) continue
    if (range.style.color != null) style.color = range.style.color
    if (range.style.bold != null) style.bold = range.style.bold
    if (range.style.italic != null) style.italic = range.style.italic
  }
  return style
}

const canonicalize = (ranges: TextStyleRange[]): TextStyleRange[] => {
  const points = [...new Set(ranges.flatMap((range) => [range.start, range.end]))]
    .filter((point) => Number.isInteger(point) && point >= 0)
    .sort((a, b) => a - b)
  const result: TextStyleRange[] = []
  for (let i = 0; i + 1 < points.length; i += 1) {
    const start = points[i]
    const end = points[i + 1]
    if (start >= end) continue
    const style = styleAtOffset(ranges, start)
    if (styleIsEmpty(style)) continue
    const previous = result.at(-1)
    if (previous && previous.end === start && sameStyle(previous.style, style)) {
      previous.end = end
    } else {
      result.push({ start, end, style })
    }
  }
  return result
}

export type TextRangeStyleUpdate = {
  color?: number[] | null
  bold?: boolean | null
  italic?: boolean | null
}

/** Clip styles to an exact UTF-16 text slice, then rebase to its UTF-8 origin.
 * Unlike edit inference, this preserves which occurrence of repeated text was cut. */
export const sliceTextStyleRanges = (
  text: string,
  ranges: TextStyleRange[],
  start: number,
  end: number,
): TextStyleRange[] => {
  const byteStart = utf16OffsetToUtf8(text, start)
  const byteEnd = utf16OffsetToUtf8(text, end)
  return canonicalize(
    ranges.flatMap((range) => {
      const clippedStart = Math.max(range.start, byteStart)
      const clippedEnd = Math.min(range.end, byteEnd)
      return clippedStart < clippedEnd
        ? [{ ...range, start: clippedStart - byteStart, end: clippedEnd - byteStart }]
        : []
    }),
  )
}

/** Apply partial style overrides to a selection while preserving/splitting
 * formatting on either side. The result is sorted and non-overlapping. */
export const applyTextRangeStyle = (
  ranges: TextStyleRange[],
  start: number,
  end: number,
  update: TextRangeStyleUpdate,
): TextStyleRange[] => {
  if (start >= end) return canonicalize(ranges)
  const points = [...new Set([...ranges.flatMap((range) => [range.start, range.end]), start, end])]
    .filter((point) => Number.isInteger(point) && point >= 0)
    .sort((a, b) => a - b)
  const result: TextStyleRange[] = []
  for (let i = 0; i + 1 < points.length; i += 1) {
    const intervalStart = points[i]
    const intervalEnd = points[i + 1]
    if (intervalStart >= intervalEnd) continue
    const style = styleAtOffset(ranges, intervalStart)
    if (intervalStart >= start && intervalEnd <= end) {
      if ('color' in update) style.color = update.color ?? undefined
      if ('bold' in update) style.bold = update.bold ?? undefined
      if ('italic' in update) style.italic = update.italic ?? undefined
    }
    if (!styleIsEmpty(style)) result.push({ start: intervalStart, end: intervalEnd, style })
  }
  return canonicalize(result)
}

/** Preserve range formatting through a normal textarea edit. Insertions made
 * inside a styled run inherit that run; text outside an edit shifts by the
 * UTF-8 byte delta. */
export const rebaseTextStyleRanges = (
  oldText: string,
  newText: string,
  ranges: TextStyleRange[],
): TextStyleRange[] => {
  if (oldText === newText || ranges.length === 0) return canonicalize(ranges)
  const oldChars = [...oldText]
  const newChars = [...newText]
  let prefixChars = 0
  while (
    prefixChars < oldChars.length &&
    prefixChars < newChars.length &&
    oldChars[prefixChars] === newChars[prefixChars]
  ) {
    prefixChars += 1
  }
  let suffixChars = 0
  while (
    suffixChars < oldChars.length - prefixChars &&
    suffixChars < newChars.length - prefixChars &&
    oldChars[oldChars.length - 1 - suffixChars] === newChars[newChars.length - 1 - suffixChars]
  ) {
    suffixChars += 1
  }

  const oldStart = utf8Length(oldChars.slice(0, prefixChars).join(''))
  const oldEnd = utf8Length(oldChars.slice(0, oldChars.length - suffixChars).join(''))
  const newEnd = utf8Length(newChars.slice(0, newChars.length - suffixChars).join(''))
  const delta = newEnd - oldEnd

  return canonicalize(
    ranges.flatMap((range): TextStyleRange[] => {
      if (range.end <= oldStart) return [range]
      if (range.start >= oldEnd) {
        return [{ ...range, start: range.start + delta, end: range.end + delta }]
      }

      const nextStart = Math.min(range.start, oldStart)
      const nextEnd = range.end >= oldEnd ? range.end + delta : newEnd
      return nextStart < nextEnd ? [{ ...range, start: nextStart, end: nextEnd }] : []
    }),
  )
}

export const selectionHasStyle = (
  ranges: TextStyleRange[],
  start: number,
  end: number,
  key: 'bold' | 'italic',
  inherited: boolean,
): boolean => {
  if (start >= end) return false
  const points = [...new Set([...ranges.flatMap((range) => [range.start, range.end]), start, end])]
    .filter((point) => point >= start && point <= end)
    .sort((a, b) => a - b)
  for (let i = 0; i + 1 < points.length; i += 1) {
    if ((styleAtOffset(ranges, points[i])[key] ?? inherited) !== true) return false
  }
  return true
}
