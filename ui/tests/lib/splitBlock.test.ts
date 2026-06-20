import { describe, expect, it } from 'vitest'

import type { Transform } from '@/lib/api/schemas'
import { splitTextBlock, splitTextValue } from '@/lib/splitBlock'

describe('splitTextValue', () => {
  it('splits multi-line text by line at the midpoint', () => {
    expect(splitTextValue('Line one\nLine two')).toEqual(['Line one', 'Line two'])
    expect(splitTextValue('a\nb\nc')).toEqual(['a\nb', 'c'])
  })

  it('splits a single line by sentence', () => {
    expect(splitTextValue('Hello there. How are you?')).toEqual(['Hello there.', 'How are you?'])
  })

  it('splits a single sentence by word at the midpoint', () => {
    expect(splitTextValue('one two three four')).toEqual(['one two', 'three four'])
  })

  it('keeps a single token in the first half', () => {
    expect(splitTextValue('word')).toEqual(['word', ''])
  })

  it('returns empty halves for blank input', () => {
    expect(splitTextValue('')).toEqual(['', ''])
    expect(splitTextValue(null)).toEqual(['', ''])
    expect(splitTextValue(undefined)).toEqual(['', ''])
  })
})

describe('splitTextBlock', () => {
  const base = (w: number, h: number): Transform => ({
    x: 100,
    y: 200,
    width: w,
    height: h,
    rotationDeg: 0,
  })

  it('cuts a wide box into left/right halves that tile exactly', () => {
    const split = splitTextBlock(base(200, 80), { translation: 'A\nB' })
    expect(split.axis).toBe('leftRight')
    expect(split.a.transform).toMatchObject({ x: 100, y: 200, width: 100, height: 80 })
    expect(split.b.transform).toMatchObject({ x: 200, y: 200, width: 100, height: 80 })
    // No gap/overlap: a.x + a.width === b.x, and they span the original width.
    expect(split.a.transform.x + split.a.transform.width).toBe(split.b.transform.x)
    expect(split.b.transform.x + split.b.transform.width).toBe(300)
    expect(split.a.translation).toBe('A')
    expect(split.b.translation).toBe('B')
  })

  it('cuts a tall box into top/bottom halves', () => {
    const split = splitTextBlock(base(60, 200), { translation: 'top\nbottom' })
    expect(split.axis).toBe('topBottom')
    expect(split.a.transform).toMatchObject({ x: 100, y: 200, width: 60, height: 100 })
    expect(split.b.transform).toMatchObject({ x: 100, y: 300, width: 60, height: 100 })
    expect(split.a.translation).toBe('top')
    expect(split.b.translation).toBe('bottom')
  })

  it('tiles odd-sized boxes without a gap or overshoot', () => {
    const horizontal = splitTextBlock(base(101, 40), {})
    expect(horizontal.a.transform.x + horizontal.a.transform.width).toBe(horizontal.b.transform.x)
    expect(horizontal.b.transform.x + horizontal.b.transform.width).toBe(201)

    const vertical = splitTextBlock(base(40, 101), {})
    expect(vertical.a.transform.y + vertical.a.transform.height).toBe(vertical.b.transform.y)
    expect(vertical.b.transform.y + vertical.b.transform.height).toBe(301)
  })

  it('preserves rotation on both halves', () => {
    const split = splitTextBlock({ x: 0, y: 0, width: 100, height: 40, rotationDeg: 15 }, {})
    expect(split.a.transform.rotationDeg).toBe(15)
    expect(split.b.transform.rotationDeg).toBe(15)
  })

  it('divides source text and translation independently', () => {
    const split = splitTextBlock(base(200, 50), {
      text: 'あ\nい',
      translation: 'first line\nsecond line',
    })
    expect(split.a.text).toBe('あ')
    expect(split.b.text).toBe('い')
    expect(split.a.translation).toBe('first line')
    expect(split.b.translation).toBe('second line')
  })
})
