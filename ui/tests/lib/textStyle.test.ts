import { describe, expect, it } from 'vitest'

import type { TextStyle } from '@/lib/api/schemas'
import { DEFAULT_TEXT_COLOR, effectiveTextColor, mergeTextStyle } from '@/lib/textStyle'

const style: TextStyle = {
  fontFamilies: ['Arial'],
  fontSize: 24,
  color: [1, 2, 3, 255],
  effect: { bold: true, italic: false },
  stroke: { enabled: true, color: [255, 255, 255, 255], widthPx: 4 },
  textAlign: 'left',
}

describe('effectiveTextColor', () => {
  it('prefers manual style color and otherwise previews auto as black', () => {
    expect(effectiveTextColor(style)).toEqual([1, 2, 3, 255])
    expect(effectiveTextColor({ ...style, color: null })).toEqual(DEFAULT_TEXT_COLOR)
    expect(effectiveTextColor(null)).toEqual(DEFAULT_TEXT_COLOR)
  })

  it('honours pure black and white as manual picks (v4 semantics)', () => {
    expect(effectiveTextColor({ ...style, color: [0, 0, 0, 255] })).toEqual([0, 0, 0, 255])
    expect(effectiveTextColor({ ...style, color: [255, 255, 255, 255] })).toEqual([
      255, 255, 255, 255,
    ])
  })
})

describe('mergeTextStyle', () => {
  it('keeps current values for fields absent from the update', () => {
    const next = mergeTextStyle(style, { fontSize: 30 })
    expect(next.fontSize).toBe(30)
    expect(next.fontFamilies).toEqual(['Arial'])
    expect(next.stroke).toEqual(style.stroke)
    expect(next.effect).toEqual(style.effect)
    expect(next.textAlign).toBe('left')
    expect(next.color).toEqual([1, 2, 3, 255])
  })

  it('clears a field back to auto when the update is explicitly null', () => {
    expect(mergeTextStyle(style, { fontSize: null }).fontSize).toBeNull()
    expect(mergeTextStyle(style, { stroke: null }).stroke).toBeNull()
    expect(mergeTextStyle(style, { effect: null }).effect).toBeNull()
    expect(mergeTextStyle(style, { textAlign: null }).textAlign).toBeNull()
    expect(mergeTextStyle(style, { color: null }).color).toBeNull()
    // Clearing one field leaves the others untouched.
    const next = mergeTextStyle(style, { stroke: null })
    expect(next.fontSize).toBe(24)
    expect(next.color).toEqual([1, 2, 3, 255])
  })

  it('leaves color in auto mode when a style is created for another field', () => {
    // Block had no style; setting only the size must not freeze a colour.
    const next = mergeTextStyle(null, { fontSize: 18 })
    expect(next.color).toBeNull()
    expect(next.fontSize).toBe(18)
    expect(next.stroke).toBeNull()
  })
})
