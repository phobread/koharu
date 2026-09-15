import { describe, expect, it } from 'vitest'

import {
  applyTextRangeStyle,
  rebaseTextStyleRanges,
  styleAtOffset,
  utf16OffsetToUtf8,
} from '@/lib/richText'

describe('rich text ranges', () => {
  it('applies and splits independent properties across selections', () => {
    let ranges = applyTextRangeStyle([], 6, 11, { bold: true })
    ranges = applyTextRangeStyle(ranges, 0, 5, { color: [200, 20, 30, 255] })
    ranges = applyTextRangeStyle(ranges, 3, 8, { italic: true })

    expect(styleAtOffset(ranges, 1)).toEqual({ color: [200, 20, 30, 255] })
    expect(styleAtOffset(ranges, 4)).toEqual({
      color: [200, 20, 30, 255],
      italic: true,
    })
    expect(styleAtOffset(ranges, 7)).toEqual({ bold: true, italic: true })
    expect(styleAtOffset(ranges, 10)).toEqual({ bold: true })
  })

  it('clears formatting only inside the selected slice', () => {
    const ranges = applyTextRangeStyle(
      [{ start: 0, end: 10, style: { bold: true, color: [1, 2, 3, 255] } }],
      3,
      7,
      { bold: null, color: null, italic: null },
    )

    expect(styleAtOffset(ranges, 2)).toEqual({ bold: true, color: [1, 2, 3, 255] })
    expect(styleAtOffset(ranges, 4)).toEqual({})
    expect(styleAtOffset(ranges, 8)).toEqual({ bold: true, color: [1, 2, 3, 255] })
  })

  it('converts textarea UTF-16 offsets to renderer UTF-8 offsets', () => {
    const text = 'A猫🙂B'
    expect(utf16OffsetToUtf8(text, 1)).toBe(1)
    expect(utf16OffsetToUtf8(text, 2)).toBe(4)
    expect(utf16OffsetToUtf8(text, 4)).toBe(8)
    expect(utf16OffsetToUtf8(text, 5)).toBe(9)
  })

  it('keeps formatting attached through insertions', () => {
    const boldWorld = [{ start: 6, end: 11, style: { bold: true } }]
    expect(rebaseTextStyleRanges('Hello world', 'Hello brave world', boldWorld)).toEqual([
      { start: 12, end: 17, style: { bold: true } },
    ])
    expect(rebaseTextStyleRanges('Hello world', 'Hello woXrld', boldWorld)).toEqual([
      { start: 6, end: 12, style: { bold: true } },
    ])
  })

  it('shifts ranges by UTF-8 byte length for non-ASCII edits', () => {
    expect(
      rebaseTextStyleRanges('猫 says hi', '🙂猫 says hi', [
        { start: 8, end: 10, style: { italic: true } },
      ]),
    ).toEqual([{ start: 12, end: 14, style: { italic: true } }])
  })
})
