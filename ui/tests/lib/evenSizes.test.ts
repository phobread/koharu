import { describe, expect, it } from 'vitest'

import type { TextNodeEntry } from '@/hooks/useCurrentPage'
import { evenOutSizes } from '@/lib/evenSizes'

function box(
  id: string,
  rendered: number | null,
  transform = { x: 100, y: 100, width: 200, height: 100, rotationDeg: 0 },
  text: Partial<TextNodeEntry['data']> = {},
): TextNodeEntry {
  return {
    id,
    transform,
    data: { translation: 'Hello there', renderedFontSizePx: rendered, ...text } as never,
  }
}

const centre = (t: { x: number; y: number; width: number; height: number }) => [
  t.x + t.width / 2,
  t.y + t.height / 2,
]

describe('evenOutSizes', () => {
  it('gives every box the middle size', () => {
    const result = evenOutSizes([box('a', 10), box('b', 20), box('c', 40)])
    expect(result?.size).toBe(20)
    expect(result?.changes.map((c) => [c.id, c.fontSize])).toEqual([
      ['a', 20],
      ['b', 20],
      ['c', 20],
    ])
  })

  it('takes the size between the two middle ones for an even count', () => {
    expect(evenOutSizes([box('a', 10), box('b', 20), box('c', 31), box('d', 40)])?.size).toBe(26)
  })

  it('counts a size set by hand before the fitted one', () => {
    const byHand = box('a', 12, undefined, { style: { fontSize: 30 } as never })
    expect(evenOutSizes([byHand, box('b', 30), box('c', 10)])?.size).toBe(30)
  })

  it('grows a box around its centre, by at least the factor its font grows by', () => {
    const small = box('a', 10, { x: 100, y: 100, width: 200, height: 100, rotationDeg: 0 })
    const change = evenOutSizes([small, box('b', 20), box('c', 20)])!.changes[0]
    expect(change.transform!.width).toBeGreaterThanOrEqual(400)
    expect(change.transform!.width).toBeLessThan(420)
    expect(change.transform!.height).toBeGreaterThanOrEqual(200)
    expect(centre(change.transform!)).toEqual([200, 150])
  })

  it('shrinks a box but keeps room for the margins that do not shrink', () => {
    const big = box('a', 40, { x: 0, y: 0, width: 400, height: 200, rotationDeg: 0 })
    const change = evenOutSizes([big, box('b', 20), box('c', 20)], 4)!.changes[0]
    // Half the size: half the box, plus half of the fixed 20px + 2 x 4px padding.
    expect(change.transform!.width).toBeGreaterThanOrEqual(200 + 14)
    expect(change.transform!.width).toBeLessThan(400)
    expect(centre(change.transform!)).toEqual([200, 100])
  })

  it('starts from the text as drawn when it follows its bubble, not from the stored box', () => {
    // The stored box sits at the top of the bubble; the text is drawn lower and wider.
    const inBubble = box(
      'a',
      10,
      { x: 100, y: 100, width: 50, height: 50, rotationDeg: 0 },
      { spriteTransform: { x: 80, y: 200, width: 120, height: 60 } as never },
    )
    const change = evenOutSizes([inBubble, box('b', 20), box('c', 20)])!.changes[0]
    expect(centre(change.transform!)).toEqual([140, 230])
    expect(change.transform!.width).toBeGreaterThanOrEqual(240 + 20)
  })

  it('keeps a box tilted', () => {
    const tilted = box('a', 10, { x: 0, y: 0, width: 100, height: 50, rotationDeg: 15 })
    const change = evenOutSizes([tilted, box('b', 20), box('c', 20)])!.changes[0]
    expect(change.transform!.rotationDeg).toBe(15)
    expect(centre(change.transform!)).toEqual([50, 25])
  })

  it('leaves the box alone when it has the middle size already, and pins the size', () => {
    const result = evenOutSizes([box('a', 20), box('b', 20), box('c', 40)])!
    expect(result.changes.find((c) => c.id === 'a')).toEqual({ id: 'a', fontSize: 20 })
  })

  it('skips boxes with no text drawn', () => {
    const empty = box('a', 90, undefined, { translation: '  ' })
    const neverDrawn = box('b', null)
    const result = evenOutSizes([empty, neverDrawn, box('c', 10), box('d', 30), box('e', 30)])!
    expect(result.size).toBe(30)
    expect(result.changes.map((c) => c.id)).toEqual(['c', 'd', 'e'])
  })

  it('has nothing to do for one box, or when every size is already set and equal', () => {
    expect(evenOutSizes([box('a', 20)])).toBeNull()
    const pinned = (id: string) => box(id, 20, undefined, { style: { fontSize: 20 } as never })
    expect(evenOutSizes([pinned('a'), pinned('b')])).toBeNull()
  })
})
