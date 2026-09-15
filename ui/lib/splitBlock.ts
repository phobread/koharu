import type { TextDirection, Transform } from '@/lib/api/schemas'
import { rotateVec } from '@/lib/rotatedBox'

type SplitInput = { text?: string | null; translation?: string | null }

export type BlockSplitPart = {
  transform: Transform
  text: string | null
  translation: string | null
  /** UTF-16 slice of the original translation retained by this half. */
  translationSpan: { start: number; end: number }
}

export type BlockSplit = {
  /** `leftRight` cuts a wide box into left/right halves; `topBottom` cuts a
   *  tall box into upper/lower halves. Part `a` always holds the text that
   *  reads first: top for `topBottom`, left for a `leftRight` midpoint split,
   *  and the *right* side for a caret split of vertical (RTL-column) text. */
  axis: 'leftRight' | 'topBottom'
  a: BlockSplitPart
  b: BlockSplitPart
}

/** Which text field a caret split was issued in. */
export type SplitField = 'text' | 'translation'

const SENTENCE_RE = /[^.!?。！？]+[.!?。！？]*/g

type TextSlice = { value: string; start: number; end: number }

function trimmedSlice(value: string, start: number, end: number): TextSlice {
  const raw = value.slice(start, end)
  const trimmed = raw.trim()
  const trimmedStart = start + raw.length - raw.trimStart().length
  return { value: trimmed, start: trimmedStart, end: trimmedStart + trimmed.length }
}

function slicesAt(value: string, offset: number): [TextSlice, TextSlice] {
  return [trimmedSlice(value, 0, offset), trimmedSlice(value, offset, value.length)]
}

function naturalSlices(value: string, ratio?: number): [TextSlice, TextSlice] {
  const groups = [/[^\r\n]+/g, ...(ratio === undefined ? [SENTENCE_RE] : []), /\S+/g]
  // Keep the original characters/spacing inside each half. Rebuilding from
  // words or lines would detach character formatting from its byte offsets.
  for (const pattern of groups) {
    const matches = [...value.matchAll(pattern)].filter((match) => match[0].trim())
    if (matches.length < 2) continue
    const cut =
      ratio === undefined
        ? Math.ceil(matches.length / 2)
        : Math.min(Math.max(Math.round(matches.length * ratio), 1), matches.length - 1)
    return slicesAt(value, matches[cut].index)
  }
  if (ratio !== undefined) {
    const trimmed = trimmedSlice(value, 0, value.length)
    const chars = [...trimmed.value]
    if (chars.length > 1) {
      const cut = Math.min(Math.max(Math.round(chars.length * ratio), 1), chars.length - 1)
      return slicesAt(value, trimmed.start + chars.slice(0, cut).join('').length)
    }
  }
  return slicesAt(value, value.length)
}

function validCaret(value: string, offset: number): boolean {
  if (!Number.isInteger(offset) || offset < 0 || offset > value.length) return false
  // A UTF-16 offset between a surrogate pair is not a character boundary.
  const previous = value.charCodeAt(offset - 1)
  const next = value.charCodeAt(offset)
  return !(previous >= 0xd800 && previous <= 0xdbff && next >= 0xdc00 && next <= 0xdfff)
}

/**
 * Divide text into two halves: by line if it's multi-line, else by sentence,
 * else by word at the midpoint, else everything lands in the first half. The
 * first half (`a`) maps to the top (tall box) or left (wide box).
 */
export function splitTextValue(value: string | null | undefined): [string, string] {
  const [a, b] = naturalSlices(value ?? '')
  return [a.value, b.value]
}

/**
 * Split text at an exact character offset, trimming whitespace on both sides
 * of the cut. Returns `null` when either side would end up empty (nothing to
 * split).
 */
export function splitTextValueAt(
  value: string | null | undefined,
  offset: number,
): [string, string] | null {
  const v = value ?? ''
  if (!validCaret(v, offset)) return null
  const [a, b] = slicesAt(v, offset)
  if (!a.value || !b.value) return null
  return [a.value, b.value]
}

/**
 * Split text near a fractional position, preferring natural boundaries: the
 * line break closest to `ratio` if multi-line, else the word gap closest to
 * `ratio`, else the character offset itself (CJK text has no word gaps).
 */
export function splitTextValueNear(
  value: string | null | undefined,
  ratio: number,
): [string, string] {
  const [a, b] = naturalSlices(value ?? '', Math.min(Math.max(ratio, 0), 1))
  return [a.value, b.value]
}

/** Keep both halves usable: neither side smaller than 20% of the box. */
const MIN_SPLIT_RATIO = 0.2

function placeInRotatedFrame(original: Transform, half: Transform): Transform {
  const deg = original.rotationDeg ?? 0
  if (deg === 0) return half

  const cx = original.x + original.width / 2
  const cy = original.y + original.height / 2
  const halfCx = half.x + half.width / 2
  const halfCy = half.y + half.height / 2
  const [dx, dy] = rotateVec(halfCx - cx, halfCy - cy, deg)

  // Halves rotate about their own centers, so the unrotated center offset must be rotated into the original's frame for the pieces to tile the original rotated rect.
  return { ...half, x: cx + dx - half.width / 2, y: cy + dy - half.height / 2 }
}

/**
 * Split a text block at a caret position inside one of its text fields. The
 * cut follows the text flow: horizontal text stacks the halves top/bottom,
 * vertical (manga RTL-column) text places them side by side with the first
 * half on the right. The other text field is divided near the same fractional
 * position, and the box is split proportionally to the caret (clamped so
 * neither half collapses). Returns `null` when the caret leaves either side
 * empty. The halves tile the original box exactly at any rotation.
 */
export function splitTextBlockAt(
  transform: Transform,
  data: SplitInput,
  cut: { field: SplitField; offset: number },
  direction: TextDirection,
): BlockSplit | null {
  const primary = data[cut.field] ?? ''
  if (!validCaret(primary, cut.offset)) return null
  const parts = slicesAt(primary, cut.offset)
  if (!parts[0].value || !parts[1].value) return null

  const ratio = Math.min(
    Math.max(cut.offset / Math.max(primary.length, 1), MIN_SPLIT_RATIO),
    1 - MIN_SPLIT_RATIO,
  )
  const otherField: SplitField = cut.field === 'text' ? 'translation' : 'text'
  const otherParts = naturalSlices(data[otherField] ?? '', cut.offset / Math.max(primary.length, 1))

  let aT: Transform
  let bT: Transform
  if (direction === 'vertical') {
    // Vertical columns read right→left: the first half takes the right side.
    const aW = transform.width * ratio
    aT = { ...transform, x: transform.x + (transform.width - aW), width: aW }
    bT = { ...transform, width: transform.width - aW }
  } else {
    const aH = transform.height * ratio
    aT = { ...transform, height: aH }
    bT = { ...transform, y: transform.y + aH, height: transform.height - aH }
  }
  aT = placeInRotatedFrame(transform, aT)
  bT = placeInRotatedFrame(transform, bT)

  const [aPrimary, bPrimary] = parts
  const [aOther, bOther] = otherParts
  const texts = (primary: TextSlice, other: TextSlice) =>
    cut.field === 'text'
      ? {
          text: primary.value || null,
          translation: other.value || null,
          translationSpan: { start: other.start, end: other.end },
        }
      : {
          text: other.value || null,
          translation: primary.value || null,
          translationSpan: { start: primary.start, end: primary.end },
        }

  return {
    axis: direction === 'vertical' ? 'leftRight' : 'topBottom',
    a: { transform: aT, ...texts(aPrimary, aOther) },
    b: { transform: bT, ...texts(bPrimary, bOther) },
  }
}

const CJK_RE = /[　-ヿ㐀-鿿豈-﫿ｦ-ﾟ]/

/** Join text fragments reading-order first→last: CJK runs join without a
 *  separator, everything else with a space. */
export function joinMergedText(parts: (string | null | undefined)[]): string | null {
  const vals = parts.map((v) => (v ?? '').trim()).filter(Boolean)
  if (vals.length === 0) return null
  return vals.join(vals.every((v) => CJK_RE.test(v)) ? '' : ' ')
}

export type MergedBlock = {
  transform: Transform
  text: string | null
  translation: string | null
}

/**
 * Merge two or more text blocks (given in reading order) into one: the union
 * of their boxes plus their texts joined in order. Because split halves tile
 * the original box exactly at any rotation, merging them reconstructs the
 * pre-split box at any rotation — this is the inverse of both split flavours.
 * Rotation follows the first block. Returns `null` for fewer than two blocks.
 */
export function mergeTextBlocks(
  blocks: { transform: Transform; text?: string | null; translation?: string | null }[],
): MergedBlock | null {
  if (blocks.length < 2) return null
  const deg = blocks[0].transform.rotationDeg ?? 0
  if (deg === 0) {
    const x = Math.min(...blocks.map((b) => b.transform.x))
    const y = Math.min(...blocks.map((b) => b.transform.y))
    const right = Math.max(...blocks.map((b) => b.transform.x + b.transform.width))
    const bottom = Math.max(...blocks.map((b) => b.transform.y + b.transform.height))
    return {
      transform: {
        ...blocks[0].transform,
        x,
        y,
        width: right - x,
        height: bottom - y,
      },
      text: joinMergedText(blocks.map((b) => b.text)),
      translation: joinMergedText(blocks.map((b) => b.translation)),
    }
  }

  const origin = {
    x: blocks[0].transform.x + blocks[0].transform.width / 2,
    y: blocks[0].transform.y + blocks[0].transform.height / 2,
  }
  const bounds = blocks.map((block) => {
    const transform = block.transform
    const px = transform.x + transform.width / 2
    const py = transform.y + transform.height / 2
    const [dx, dy] = rotateVec(px - origin.x, py - origin.y, -deg)
    const qx = origin.x + dx
    const qy = origin.y + dy
    return {
      left: qx - transform.width / 2,
      top: qy - transform.height / 2,
      right: qx + transform.width / 2,
      bottom: qy + transform.height / 2,
    }
  })
  // Union is computed in the first block's de-rotated frame so merging split halves reconstructs the pre-split box at any slant.
  const left = Math.min(...bounds.map((bound) => bound.left))
  const top = Math.min(...bounds.map((bound) => bound.top))
  const right = Math.max(...bounds.map((bound) => bound.right))
  const bottom = Math.max(...bounds.map((bound) => bound.bottom))
  const width = right - left
  const height = bottom - top
  const deRotatedCenter = { x: (left + right) / 2, y: (top + bottom) / 2 }
  const [centerDx, centerDy] = rotateVec(
    deRotatedCenter.x - origin.x,
    deRotatedCenter.y - origin.y,
    deg,
  )
  const center = { x: origin.x + centerDx, y: origin.y + centerDy }
  return {
    transform: {
      ...blocks[0].transform,
      x: center.x - width / 2,
      y: center.y - height / 2,
      width,
      height,
    },
    text: joinMergedText(blocks.map((b) => b.text)),
    translation: joinMergedText(blocks.map((b) => b.translation)),
  }
}

/**
 * Split a text block into two halves along its longer side (wide → left/right,
 * tall → top/bottom), dividing the source + translation text between them. The
 * two halves tile the original box exactly (no gap/overlap) at any rotation.
 */
export function splitTextBlock(
  transform: Transform,
  data: SplitInput,
  direction?: TextDirection,
): BlockSplit {
  const leftRight = direction === 'vertical' || transform.width >= transform.height
  const [aText, bText] = splitTextValue(data.text)
  const [aTranslation, bTranslation] = naturalSlices(data.translation ?? '')

  let aT: Transform
  let bT: Transform
  if (leftRight) {
    const halfW = transform.width / 2
    aT = { ...transform, x: transform.x + (direction === 'vertical' ? halfW : 0), width: halfW }
    bT = {
      ...transform,
      x: transform.x + (direction === 'vertical' ? 0 : halfW),
      width: transform.width - halfW,
    }
  } else {
    const halfH = transform.height / 2
    aT = { ...transform, height: halfH }
    bT = { ...transform, y: transform.y + halfH, height: transform.height - halfH }
  }
  aT = placeInRotatedFrame(transform, aT)
  bT = placeInRotatedFrame(transform, bT)

  return {
    axis: leftRight ? 'leftRight' : 'topBottom',
    a: {
      transform: aT,
      text: aText || null,
      translation: aTranslation.value || null,
      translationSpan: { start: aTranslation.start, end: aTranslation.end },
    },
    b: {
      transform: bT,
      text: bText || null,
      translation: bTranslation.value || null,
      translationSpan: { start: bTranslation.start, end: bTranslation.end },
    },
  }
}
