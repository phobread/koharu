import { describe, expect, it } from 'vitest'

import type { FontPrediction, TextStyle } from '@/lib/api/schemas'
import { DEFAULT_TEXT_COLOR, effectiveTextColor, mergeTextStyle } from '@/lib/textStyle'

const prediction: FontPrediction = {
  textColor: [200, 30, 30],
  strokeWidthPx: 3,
} as FontPrediction

const style: TextStyle = {
  fontFamilies: ['Arial'],
  fontSize: 24,
  color: [1, 2, 3, 255],
  effect: { bold: true, italic: false },
  stroke: { enabled: true, color: [255, 255, 255, 255], widthPx: 4 },
  textAlign: 'left',
}

describe('effectiveTextColor', () => {
  it('prefers explicit style color, then prediction, then black', () => {
    expect(effectiveTextColor(style, prediction)).toEqual([1, 2, 3, 255])
    expect(effectiveTextColor(null, prediction)).toEqual([200, 30, 30, 255])
    expect(effectiveTextColor(null, null)).toEqual(DEFAULT_TEXT_COLOR)
  })
})

describe('mergeTextStyle', () => {
  it('keeps current values for fields absent from the update', () => {
    const next = mergeTextStyle(style, prediction, { fontSize: 30 })
    expect(next.fontSize).toBe(30)
    expect(next.fontFamilies).toEqual(['Arial'])
    expect(next.stroke).toEqual(style.stroke)
    expect(next.effect).toEqual(style.effect)
    expect(next.textAlign).toBe('left')
    expect(next.color).toEqual([1, 2, 3, 255])
  })

  it('clears a field back to auto when the update is explicitly null', () => {
    expect(mergeTextStyle(style, prediction, { fontSize: null }).fontSize).toBeNull()
    expect(mergeTextStyle(style, prediction, { stroke: null }).stroke).toBeNull()
    expect(mergeTextStyle(style, prediction, { effect: null }).effect).toBeNull()
    expect(mergeTextStyle(style, prediction, { textAlign: null }).textAlign).toBeNull()
    // Clearing one field leaves the others untouched.
    const next = mergeTextStyle(style, prediction, { stroke: null })
    expect(next.fontSize).toBe(24)
    expect(next.color).toEqual([1, 2, 3, 255])
  })

  it('resets color to the model-predicted value on explicit null', () => {
    expect(mergeTextStyle(style, prediction, { color: null }).color).toEqual([200, 30, 30, 255])
    // Without a prediction the reset falls back to black.
    expect(mergeTextStyle(style, null, { color: null }).color).toEqual(DEFAULT_TEXT_COLOR)
  })

  it('materialises the effective color when a style becomes explicit', () => {
    // Block had no style; setting only the size must not turn predicted red
    // into black.
    const next = mergeTextStyle(null, prediction, { fontSize: 18 })
    expect(next.color).toEqual([200, 30, 30, 255])
    expect(next.fontSize).toBe(18)
    expect(next.stroke).toBeNull()
  })
})
