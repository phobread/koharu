import type { TextDirection, Transform } from '@/lib/api/schemas'

type SplitInput = { text?: string | null; translation?: string | null }

export type BlockSplitPart = {
  transform: Transform
  text: string | null
  translation: string | null
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

/**
 * Divide text into two halves: by line if it's multi-line, else by sentence,
 * else by word at the midpoint, else everything lands in the first half. The
 * first half (`a`) maps to the top (tall box) or left (wide box).
 */
export function splitTextValue(value: string | null | undefined): [string, string] {
  const v = (value ?? '').trim()
  if (!v) return ['', '']

  const lines = v
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean)
  if (lines.length >= 2) {
    const mid = Math.ceil(lines.length / 2)
    return [lines.slice(0, mid).join('\n'), lines.slice(mid).join('\n')]
  }

  const sentences = (v.match(SENTENCE_RE) ?? []).map((s) => s.trim()).filter(Boolean)
  if (sentences.length >= 2) {
    const mid = Math.ceil(sentences.length / 2)
    return [sentences.slice(0, mid).join(' '), sentences.slice(mid).join(' ')]
  }

  const words = v.split(/\s+/).filter(Boolean)
  if (words.length >= 2) {
    const mid = Math.ceil(words.length / 2)
    return [words.slice(0, mid).join(' '), words.slice(mid).join(' ')]
  }

  return [v, '']
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
  const a = v.slice(0, offset).trim()
  const b = v.slice(offset).trim()
  if (!a || !b) return null
  return [a, b]
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
  const v = (value ?? '').trim()
  if (!v) return ['', '']
  const r = Math.min(Math.max(ratio, 0), 1)

  const lines = v
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean)
  if (lines.length >= 2) {
    const cut = Math.min(Math.max(Math.round(lines.length * r), 1), lines.length - 1)
    return [lines.slice(0, cut).join('\n'), lines.slice(cut).join('\n')]
  }

  const words = v.split(/\s+/).filter(Boolean)
  if (words.length >= 2) {
    const cut = Math.min(Math.max(Math.round(words.length * r), 1), words.length - 1)
    return [words.slice(0, cut).join(' '), words.slice(cut).join(' ')]
  }

  const cut = Math.min(Math.max(Math.round(v.length * r), 1), v.length - 1)
  const at = splitTextValueAt(v, cut)
  return at ?? [v, '']
}

/** Keep both halves usable: neither side smaller than 20% of the box. */
const MIN_SPLIT_RATIO = 0.2

/**
 * Split a text block at a caret position inside one of its text fields. The
 * cut follows the text flow: horizontal text stacks the halves top/bottom,
 * vertical (manga RTL-column) text places them side by side with the first
 * half on the right. The other text field is divided near the same fractional
 * position, and the box is split proportionally to the caret (clamped so
 * neither half collapses). Returns `null` when the caret leaves either side
 * empty. The halves tile the original box exactly, preserving rotation.
 */
export function splitTextBlockAt(
  transform: Transform,
  data: SplitInput,
  cut: { field: SplitField; offset: number },
  direction: TextDirection,
): BlockSplit | null {
  const primary = data[cut.field] ?? ''
  const parts = splitTextValueAt(primary, cut.offset)
  if (!parts) return null

  const ratio = Math.min(
    Math.max(cut.offset / Math.max(primary.length, 1), MIN_SPLIT_RATIO),
    1 - MIN_SPLIT_RATIO,
  )
  const otherField: SplitField = cut.field === 'text' ? 'translation' : 'text'
  const otherParts = splitTextValueNear(data[otherField], cut.offset / Math.max(primary.length, 1))

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

  const [aPrimary, bPrimary] = parts
  const [aOther, bOther] = otherParts
  const texts = (primary: string, other: string) =>
    cut.field === 'text'
      ? { text: primary || null, translation: other || null }
      : { text: other || null, translation: primary || null }

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
 * the original box exactly, merging them reconstructs the pre-split box —
 * this is the inverse of both split flavours. Rotation follows the first
 * block. Returns `null` for fewer than two blocks.
 */
export function mergeTextBlocks(
  blocks: { transform: Transform; text?: string | null; translation?: string | null }[],
): MergedBlock | null {
  if (blocks.length < 2) return null
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

/**
 * Split a text block into two halves along its longer side (wide → left/right,
 * tall → top/bottom), dividing the source + translation text between them. The
 * two halves tile the original box exactly (no gap/overlap), preserving
 * rotation.
 */
export function splitTextBlock(transform: Transform, data: SplitInput): BlockSplit {
  const leftRight = transform.width >= transform.height
  const [aText, bText] = splitTextValue(data.text)
  const [aTranslation, bTranslation] = splitTextValue(data.translation)

  let aT: Transform
  let bT: Transform
  if (leftRight) {
    const halfW = transform.width / 2
    aT = { ...transform, width: halfW }
    bT = { ...transform, x: transform.x + halfW, width: transform.width - halfW }
  } else {
    const halfH = transform.height / 2
    aT = { ...transform, height: halfH }
    bT = { ...transform, y: transform.y + halfH, height: transform.height - halfH }
  }

  return {
    axis: leftRight ? 'leftRight' : 'topBottom',
    a: { transform: aT, text: aText || null, translation: aTranslation || null },
    b: { transform: bT, text: bText || null, translation: bTranslation || null },
  }
}
