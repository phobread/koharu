import { describe, expect, it } from 'vitest'

import {
  cornerScaleFactor,
  resizeRotatedBox,
  rotateVec,
  scaleRotatedBox,
  type Box,
  type ResizeEdge,
} from '@/lib/rotatedBox'

const box: Box = { x: 100, y: 50, width: 200, height: 80 }
const edge = (partial: Partial<ResizeEdge>): ResizeEdge => ({
  top: false,
  bottom: false,
  left: false,
  right: false,
  ...partial,
})

const expectBoxClose = (actual: Box, expected: Box) => {
  expect(actual.x).toBeCloseTo(expected.x, 3)
  expect(actual.y).toBeCloseTo(expected.y, 3)
  expect(actual.width).toBeCloseTo(expected.width, 3)
  expect(actual.height).toBeCloseTo(expected.height, 3)
}

describe('rotateVec', () => {
  it('rotates clockwise in y-down screen space', () => {
    const [x, y] = rotateVec(1, 0, 90)
    expect(x).toBeCloseTo(0)
    expect(y).toBeCloseTo(1)
  })
})

describe('resizeRotatedBox at 0°', () => {
  it('matches plain axis-aligned resize semantics', () => {
    // Drag right edge +30: width grows, left edge fixed.
    expectBoxClose(resizeRotatedBox(box, edge({ right: true }), 30, 999, 0, 4), {
      x: 100,
      y: 50,
      width: 230,
      height: 80,
    })
    // Drag left edge +30 (inwards): width shrinks, right edge fixed.
    expectBoxClose(resizeRotatedBox(box, edge({ left: true }), 30, 0, 0, 4), {
      x: 130,
      y: 50,
      width: 170,
      height: 80,
    })
    // Top-left corner treated as edges: both axes, opposite corner fixed.
    expectBoxClose(resizeRotatedBox(box, edge({ top: true, left: true }), 20, 10, 0, 4), {
      x: 120,
      y: 60,
      width: 180,
      height: 70,
    })
  })

  it('clamps to the minimum size while keeping the anchor fixed', () => {
    const out = resizeRotatedBox(box, edge({ left: true }), 500, 0, 0, 4)
    expectBoxClose(out, { x: 296, y: 50, width: 4, height: 80 })
  })
})

describe('resizeRotatedBox rotated', () => {
  it('keeps the opposite corner fixed in screen space at 90°', () => {
    // At 90° the box's local +x axis points down the screen. The anchor
    // (top-left in local terms) must not move on screen.
    const before = box
    const [ax, ay] = rotateVec(0.5 * before.width, 0.5 * before.height, 90)
    const anchorScreen = [
      before.x + before.width / 2 - ax,
      before.y + before.height / 2 - ay,
    ] as const

    // Screen drag straight down (+40) maps onto the local +x axis → width.
    const out = resizeRotatedBox(before, edge({ right: true, bottom: true }), 0, 40, 90, 4)
    expect(out.width).toBeCloseTo(240)
    expect(out.height).toBeCloseTo(80)

    const [nax, nay] = rotateVec(0.5 * out.width, 0.5 * out.height, 90)
    expect(out.x + out.width / 2 - nax).toBeCloseTo(anchorScreen[0], 3)
    expect(out.y + out.height / 2 - nay).toBeCloseTo(anchorScreen[1], 3)
  })
})

describe('corner scaling', () => {
  it('factor uses the dominant local axis and respects the floor', () => {
    expect(cornerScaleFactor(box, edge({ right: true, bottom: true }), 100, 8, 0, 0.1)).toBeCloseTo(
      1.5,
    )
    expect(cornerScaleFactor(box, edge({ right: true, bottom: true }), -400, 0, 0, 0.25)).toBe(0.25)
  })

  it('scaleRotatedBox at 0° anchors the opposite corner like the old math', () => {
    // Scaling ×1.5 from the bottom-right corner keeps the top-left in place.
    expectBoxClose(scaleRotatedBox(box, edge({ right: true, bottom: true }), 1.5, 0), {
      x: 100,
      y: 50,
      width: 300,
      height: 120,
    })
    // From the top-left corner, the bottom-right stays fixed.
    expectBoxClose(scaleRotatedBox(box, edge({ top: true, left: true }), 0.5, 0), {
      x: 200,
      y: 90,
      width: 100,
      height: 40,
    })
  })
})
