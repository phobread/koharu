import { describe, expect, it } from 'vitest'

import {
  BRUSH_SIZE_MAX,
  BRUSH_SIZE_MIN,
  clampBrushSize,
  stampLine,
  stepBrushSize,
} from '@/lib/brushSize'

/** Pixels a stroke fills, as "x,y" keys; fails on any part-pixel rectangle. */
function stamped(from: { x: number; y: number }, to: { x: number; y: number }, size: number) {
  const pixels = new Set<string>()
  stampLine(
    {
      fillRect: (x: number, y: number, w: number, h: number) => {
        for (const n of [x, y, w, h]) expect(Number.isInteger(n)).toBe(true)
        for (let py = y; py < y + h; py++) {
          for (let px = x; px < x + w; px++) pixels.add(`${px},${py}`)
        }
      },
    },
    from,
    to,
    size,
  )
  return pixels
}

describe('stampLine', () => {
  it('fills one whole pixel for a 1px tap', () => {
    expect([...stamped({ x: 10.3, y: 4.8 }, { x: 10.3, y: 4.8 }, 1)]).toEqual(['10,4'])
  })

  it('draws a 1px line with no gaps, even on a pixel boundary', () => {
    const pixels = stamped({ x: 2, y: 3 }, { x: 9, y: 3 }, 1)
    for (let x = 2; x <= 8; x++) {
      expect(pixels.has(`${x},2`) || pixels.has(`${x},3`)).toBe(true)
    }
  })

  it('keeps a 1px diagonal connected', () => {
    const pixels = stamped({ x: 0.5, y: 0.5 }, { x: 20.5, y: 13.5 }, 1)
    for (let x = 0; x <= 20; x++) {
      expect([...pixels].some((key) => key.startsWith(`${x},`))).toBe(true)
    }
  })

  it('stamps size x size squares', () => {
    expect(stamped({ x: 5, y: 5 }, { x: 5, y: 5 }, 2).size).toBe(4)
    expect(stamped({ x: 5.5, y: 5.5 }, { x: 5.5, y: 5.5 }, 3).size).toBe(9)
  })
})

describe('stepBrushSize', () => {
  it('has a 1..128 range', () => {
    expect(BRUSH_SIZE_MIN).toBe(1)
    expect(BRUSH_SIZE_MAX).toBe(128)
  })

  it('moves by 1 up to 8, then by 4 going up', () => {
    expect(stepBrushSize(1, 1)).toBe(2)
    expect(stepBrushSize(7, 1)).toBe(8)
    expect(stepBrushSize(8, 1)).toBe(12)
    expect(stepBrushSize(12, 1)).toBe(16)
  })

  it('moves by 4 above 8, then by 1 going down', () => {
    expect(stepBrushSize(16, -1)).toBe(12)
    expect(stepBrushSize(12, -1)).toBe(8)
    expect(stepBrushSize(8, -1)).toBe(7)
    expect(stepBrushSize(2, -1)).toBe(1)
  })

  it('snaps an off-grid size to a multiple of 4', () => {
    expect(stepBrushSize(37, 1)).toBe(40)
    expect(stepBrushSize(37, -1)).toBe(36)
    expect(stepBrushSize(9, 1)).toBe(12)
    expect(stepBrushSize(9, -1)).toBe(8)
  })

  it('clamps to 1..128', () => {
    expect(stepBrushSize(1, -1)).toBe(1)
    expect(stepBrushSize(128, 1)).toBe(128)
    expect(stepBrushSize(126, 1)).toBe(128)
    expect(stepBrushSize(500, -1)).toBe(124)
    expect(stepBrushSize(0, 1)).toBe(2)
  })

  it('clampBrushSize bounds values', () => {
    expect(clampBrushSize(-5)).toBe(1)
    expect(clampBrushSize(300)).toBe(128)
    expect(clampBrushSize(50)).toBe(50)
  })
})
