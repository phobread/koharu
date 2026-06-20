import type { Transform } from '@/lib/api/schemas'

type SplitInput = { text?: string | null; translation?: string | null }

export type BlockSplitPart = {
  transform: Transform
  text: string | null
  translation: string | null
}

export type BlockSplit = {
  /** `leftRight` cuts a wide box into left/right halves; `topBottom` cuts a
   *  tall box into upper/lower halves. */
  axis: 'leftRight' | 'topBottom'
  a: BlockSplitPart
  b: BlockSplitPart
}

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

const roundTransform = (t: Transform): Transform => ({
  ...t,
  x: Math.round(t.x),
  y: Math.round(t.y),
  width: Math.round(t.width),
  height: Math.round(t.height),
})

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
    aT = roundTransform({ ...transform, width: halfW })
    bT = roundTransform({ ...transform, x: transform.x + halfW, width: transform.width - halfW })
  } else {
    const halfH = transform.height / 2
    aT = roundTransform({ ...transform, height: halfH })
    bT = roundTransform({ ...transform, y: transform.y + halfH, height: transform.height - halfH })
  }

  return {
    axis: leftRight ? 'leftRight' : 'topBottom',
    a: { transform: aT, text: aText || null, translation: aTranslation || null },
    b: { transform: bT, text: bText || null, translation: bTranslation || null },
  }
}
