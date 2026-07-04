import { describe, expect, it } from 'vitest'

import type { Transform } from '@/lib/api/schemas'
import {
  joinMergedText,
  mergeTextBlocks,
  splitTextBlock,
  splitTextBlockAt,
  splitTextValue,
  splitTextValueAt,
  splitTextValueNear,
} from '@/lib/splitBlock'

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

describe('splitTextValueAt', () => {
  it('splits at the offset and trims whitespace around the cut', () => {
    expect(splitTextValueAt('Hello there world', 6)).toEqual(['Hello', 'there world'])
    expect(splitTextValueAt('one  two', 4)).toEqual(['one', 'two'])
  })

  it('returns null when either side would be empty', () => {
    expect(splitTextValueAt('word', 0)).toBeNull()
    expect(splitTextValueAt('word', 4)).toBeNull()
    expect(splitTextValueAt('word  ', 5)).toBeNull() // only whitespace after the cut
    expect(splitTextValueAt('', 0)).toBeNull()
    expect(splitTextValueAt(null, 0)).toBeNull()
  })
})

describe('splitTextValueNear', () => {
  it('prefers the line boundary closest to the ratio', () => {
    expect(splitTextValueNear('a\nb\nc\nd', 0.25)).toEqual(['a', 'b\nc\nd'])
    expect(splitTextValueNear('a\nb\nc\nd', 0.75)).toEqual(['a\nb\nc', 'd'])
  })

  it('falls back to the word gap closest to the ratio', () => {
    expect(splitTextValueNear('one two three four', 0.5)).toEqual(['one two', 'three four'])
    expect(splitTextValueNear('one two three four', 0.9)).toEqual(['one two three', 'four'])
  })

  it('splits CJK text (no word gaps) at the character offset', () => {
    expect(splitTextValueNear('あいうえ', 0.5)).toEqual(['あい', 'うえ'])
  })

  it('always yields a non-empty first half for single tokens', () => {
    expect(splitTextValueNear('word', 0.5)).toEqual(['wo', 'rd'])
    expect(splitTextValueNear('', 0.5)).toEqual(['', ''])
  })
})

describe('splitTextBlockAt', () => {
  const base = (w: number, h: number): Transform => ({
    x: 100,
    y: 200,
    width: w,
    height: h,
    rotationDeg: 0,
  })

  it('stacks horizontal text top/bottom at a caret-proportional cut', () => {
    // Caret after "Hello there. " (offset 13 of 26) → ~50% cut.
    const split = splitTextBlockAt(
      base(120, 200),
      { text: 'あ\nい', translation: 'Hello there. How are you?' },
      { field: 'translation', offset: 13 },
      'horizontal',
    )
    expect(split).not.toBeNull()
    expect(split!.axis).toBe('topBottom')
    expect(split!.a.translation).toBe('Hello there.')
    expect(split!.b.translation).toBe('How are you?')
    // Halves tile the original box exactly.
    expect(split!.a.transform.y).toBe(200)
    expect(split!.a.transform.y + split!.a.transform.height).toBe(split!.b.transform.y)
    expect(split!.b.transform.y + split!.b.transform.height).toBe(400)
    expect(split!.a.transform.width).toBe(120)
    expect(split!.b.transform.width).toBe(120)
    // ~half the caret ratio: 13/25 → 0.52 of 200px.
    expect(split!.a.transform.height).toBeCloseTo(200 * (13 / 25), 5)
    // Counterpart OCR text splits near the same ratio.
    expect(split!.a.text).toBe('あ')
    expect(split!.b.text).toBe('い')
  })

  it('puts the first half on the right for vertical (RTL-column) text', () => {
    const split = splitTextBlockAt(
      base(200, 120),
      { text: 'あい うえ', translation: null },
      { field: 'text', offset: 2 },
      'vertical',
    )
    expect(split).not.toBeNull()
    expect(split!.axis).toBe('leftRight')
    expect(split!.a.text).toBe('あい')
    expect(split!.b.text).toBe('うえ')
    // First half occupies the right side; halves tile exactly.
    expect(split!.b.transform.x).toBe(100)
    expect(split!.b.transform.x + split!.b.transform.width).toBe(split!.a.transform.x)
    expect(split!.a.transform.x + split!.a.transform.width).toBe(300)
    expect(split!.a.transform.height).toBe(120)
    expect(split!.b.transform.height).toBe(120)
  })

  it('clamps the geometric cut so neither half collapses', () => {
    const text = 'a bbbbbbbbbbbbbbbbbb' // caret right after "a " → tiny ratio
    const split = splitTextBlockAt(
      base(100, 100),
      { translation: text },
      { field: 'translation', offset: 2 },
      'horizontal',
    )
    expect(split).not.toBeNull()
    expect(split!.a.transform.height).toBeCloseTo(20, 5) // clamped to 20%
    expect(split!.b.transform.height).toBeCloseTo(80, 5)
  })

  it('returns null for degenerate caret positions', () => {
    const data = { translation: 'Hello world' }
    expect(
      splitTextBlockAt(base(100, 100), data, { field: 'translation', offset: 0 }, 'horizontal'),
    ).toBeNull()
    expect(
      splitTextBlockAt(base(100, 100), data, { field: 'translation', offset: 11 }, 'horizontal'),
    ).toBeNull()
    expect(
      splitTextBlockAt(base(100, 100), {}, { field: 'translation', offset: 3 }, 'horizontal'),
    ).toBeNull()
  })

  it('preserves rotation on both halves', () => {
    const split = splitTextBlockAt(
      { x: 0, y: 0, width: 100, height: 100, rotationDeg: 15 },
      { translation: 'one two' },
      { field: 'translation', offset: 3 },
      'horizontal',
    )
    expect(split!.a.transform.rotationDeg).toBe(15)
    expect(split!.b.transform.rotationDeg).toBe(15)
  })
})

describe('mergeTextBlocks', () => {
  it('reconstructs the original box from a caret split (round trip)', () => {
    const original = { x: 100, y: 200, width: 120, height: 200, rotationDeg: 0 }
    const split = splitTextBlockAt(
      original,
      { text: 'あ\nい', translation: 'Hello there. How are you?' },
      { field: 'translation', offset: 13 },
      'horizontal',
    )!
    const merged = mergeTextBlocks([split.a, split.b])!
    expect(merged.transform).toEqual(original)
    expect(merged.translation).toBe('Hello there. How are you?')
  })

  it('reconstructs the original box from a vertical split, in reading order', () => {
    const original = { x: 100, y: 200, width: 200, height: 120, rotationDeg: 0 }
    const split = splitTextBlockAt(
      original,
      { text: 'あい うえ', translation: null },
      { field: 'text', offset: 2 },
      'vertical',
    )!
    // a is the right half (reads first); merge in reading order a, b.
    const merged = mergeTextBlocks([split.a, split.b])!
    expect(merged.transform).toEqual(original)
    expect(merged.text).toBe('あいうえ') // CJK joins without a separator
  })

  it('unions non-adjacent boxes and joins latin text with spaces', () => {
    const merged = mergeTextBlocks([
      {
        transform: { x: 10, y: 10, width: 50, height: 40, rotationDeg: 0 },
        translation: 'first part',
      },
      {
        transform: { x: 80, y: 30, width: 60, height: 60, rotationDeg: 0 },
        translation: 'second part',
      },
    ])!
    expect(merged.transform).toMatchObject({ x: 10, y: 10, width: 130, height: 80 })
    expect(merged.translation).toBe('first part second part')
  })

  it('returns null for fewer than two blocks', () => {
    expect(mergeTextBlocks([])).toBeNull()
    expect(
      mergeTextBlocks([
        { transform: { x: 0, y: 0, width: 10, height: 10, rotationDeg: 0 }, text: 'x' },
      ]),
    ).toBeNull()
  })
})

describe('joinMergedText', () => {
  it('joins latin with spaces, CJK without, and skips empties', () => {
    expect(joinMergedText(['one', 'two'])).toBe('one two')
    expect(joinMergedText(['こんにちは', '世界'])).toBe('こんにちは世界')
    expect(joinMergedText(['', null, 'only'])).toBe('only')
    expect(joinMergedText([null, undefined])).toBeNull()
  })
})
