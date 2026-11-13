/**
 * "Even out sizes": give a set of text boxes one font size, the middle one of
 * the sizes they have now, and resize each box around its centre so its text
 * fits at that size.
 *
 * A box is resized by the same factor as its font. Text laid out at twice the
 * size in a box twice as wide breaks into the same lines, so nothing has to
 * be measured here. The resized box is locked: it is the layout boundary from
 * then on, also for text that used to follow its speech bubble's shape.
 */

import type { TextNodeEntry } from '@/hooks/useCurrentPage'
import type { Transform } from '@/lib/api/schemas'
import { displayBox } from '@/lib/displayBox'

/**
 * Pixels a box loses to things that don't scale with the font: the outline's
 * clearance on both sides (up to 9px each) and the fit check's own 2px.
 * The box padding from Settings comes on top.
 */
const FIXED_MARGIN = 20

/** Extra room for line widths that don't scale exactly (whole-pixel sizes). */
const ROOM = 1.03

export type EvenSizeChange = {
  id: string
  fontSize: number
  /** The resized box; absent when the box already has the size. */
  transform?: Transform
}

export type EvenSizes = {
  /** The size every box gets. */
  size: number
  changes: EvenSizeChange[]
}

/** The size a box's text is drawn at: the one set by hand, else the fitted one. */
export function currentFontSize(node: TextNodeEntry): number | undefined {
  const size = node.data.style?.fontSize ?? node.data.renderedFontSizePx
  return size != null && Number.isFinite(size) && size > 0 ? size : undefined
}

function middle(sizes: number[]): number {
  const sorted = [...sizes].sort((a, b) => a - b)
  const half = Math.floor(sorted.length / 2)
  return sorted.length % 2 === 1 ? sorted[half] : (sorted[half - 1] + sorted[half]) / 2
}

/**
 * What "Even out sizes" would change for `nodes`, or `null` when there is
 * nothing to even out: fewer than two boxes with drawn text, or all of them
 * at the middle size already.
 */
export function evenOutSizes(nodes: TextNodeEntry[], boxPadding = 0): EvenSizes | null {
  const drawn = nodes.flatMap((node) => {
    const size = node.data.translation?.trim() ? currentFontSize(node) : undefined
    return size === undefined ? [] : [{ node, size }]
  })
  if (drawn.length < 2) return null

  const target = Math.max(1, Math.round(middle(drawn.map((d) => d.size))))
  const margin = FIXED_MARGIN + 2 * Math.max(0, boxPadding)
  const changes: EvenSizeChange[] = []

  for (const { node, size } of drawn) {
    if (Math.abs(size - target) < 0.5) {
      // Already this size: pin it, so a later edit doesn't refit the box.
      if (node.data.style?.fontSize !== target) changes.push({ id: node.id, fontSize: target })
      continue
    }
    const factor = target / size
    const shown = displayBox(node.transform, node.data)
    // The stored box holds its text with the fixed margins already inside it;
    // a box drawn around bubble-shaped text has none.
    const hasMargins = shown === node.transform
    const extra = hasMargins ? Math.max(0, 1 - factor) * margin : margin
    const width = Math.ceil(shown.width * factor * ROOM + extra)
    const height = Math.ceil(shown.height * factor * ROOM + extra)
    const centreX = shown.x + shown.width / 2
    const centreY = shown.y + shown.height / 2
    changes.push({
      id: node.id,
      fontSize: target,
      transform: {
        x: centreX - width / 2,
        y: centreY - height / 2,
        width,
        height,
        rotationDeg: node.transform.rotationDeg ?? 0,
      },
    })
  }

  return changes.length === 0 ? null : { size: target, changes }
}
