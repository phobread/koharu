import { describe, expect, it } from 'vitest'

import type { Transform } from '@/lib/api/schemas'
import { displayBox } from '@/lib/displayBox'

const box: Transform = { x: 100, y: 100, width: 200, height: 100, rotationDeg: 0 }
const text = (spriteTransform: Transform | null, extra = {}) => ({
  spriteTransform,
  translation: 'Hello there',
  lockLayoutBox: false,
  ...extra,
})

describe('displayBox', () => {
  it('keeps the stored box for text fitted into it', () => {
    const centred = { x: 131, y: 120, width: 138, height: 60, rotationDeg: 0 }
    expect(displayBox(box, text(centred))).toBe(box)
  })

  it('shows the text when bubble layout reaches past the box', () => {
    const sprite = { x: 80, y: 60, width: 260, height: 170, rotationDeg: 0 }
    expect(displayBox(box, text(sprite))).toEqual(sprite)
  })

  it('shows the text when bubble layout sits off-centre inside the box', () => {
    const sprite = { x: 110, y: 104, width: 120, height: 50, rotationDeg: 0 }
    expect(displayBox(box, text(sprite))).toEqual(sprite)
  })

  it('keeps the stored box when locked, slanted, untranslated or unrendered', () => {
    const sprite = { x: 80, y: 60, width: 260, height: 170, rotationDeg: 0 }
    expect(displayBox(box, text(sprite, { lockLayoutBox: true }))).toBe(box)
    const slanted = { ...box, rotationDeg: 8 }
    expect(displayBox(slanted, text(sprite))).toBe(slanted)
    expect(displayBox(box, text(sprite, { translation: '  ' }))).toBe(box)
    expect(displayBox(box, text(null))).toBe(box)
  })
})
