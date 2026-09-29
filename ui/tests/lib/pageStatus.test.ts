import { describe, expect, it } from 'vitest'

import { exportSummary, pageStatus } from '@/lib/pageStatus'

const node = (kind: Record<string, unknown>) => ({
  id: Math.random().toString(),
  visible: true,
  kind,
})
const text = (t: Record<string, unknown>) => node({ text: t })
const image = (role: string) => node({ image: { role, blob: 'b' } })
const mask = (role: string) => node({ mask: { role, blob: 'b' } })
const page = (...nodes: ReturnType<typeof node>[]) =>
  ({
    id: 'p',
    name: 'p',
    width: 10,
    height: 10,
    nodes: Object.fromEntries(nodes.map((n) => [n.id, n])),
  }) as never

describe('pageStatus', () => {
  it('a page never detected is not started', () => {
    expect(pageStatus(page(image('source')))).toEqual({ kind: 'notStarted' })
  })

  it('a finished page is done', () => {
    const done = page(
      image('source'),
      mask('segment'),
      text({ text: '원문', translation: 'done' }),
      image('inpainted'),
      image('rendered'),
    )
    expect(pageStatus(done)).toEqual({ kind: 'done' })
  })

  it('lists what is missing, leaving hand-filled boxes alone', () => {
    const p = page(
      image('source'),
      mask('segment'),
      text({ text: null }),
      text({ text: '대사' }),
      // Typed in by hand without OCR: needs nothing.
      text({ translation: 'THERE~' }),
      image('inpainted'),
    )
    expect(pageStatus(p)).toEqual({ kind: 'needs', steps: ['ocr', 'translate', 'render'] })
  })

  it('a page whose boxes were all deleted is not "not started"', () => {
    const cleared = page(image('source'), mask('segment'), image('inpainted'), image('rendered'))
    expect(pageStatus(cleared)).toEqual({ kind: 'done' })
  })
})

describe('exportSummary', () => {
  const pages = (entries: Record<string, ReturnType<typeof page>>) => entries as never

  it('names the pages that went out without their translation, by page number', () => {
    const summary = exportSummary(
      pages({
        done: page(image('source'), image('inpainted'), image('rendered')),
        // Translated, but the render is missing: the translation is not in the image.
        unrendered: page(image('source'), image('inpainted'), text({ translation: 'Hi' })),
        cleaned: page(image('source'), image('inpainted'), text({ text: '대사' })),
        original: page(image('source')),
      }),
    )
    expect(summary).toEqual({ count: 4, unrendered: [2], cleaned: [3], original: [4] })
  })

  it('counts only the requested pages, keeping their numbers in the project', () => {
    const summary = exportSummary(
      pages({
        one: page(image('source'), image('rendered')),
        two: page(image('source')),
        three: page(image('source'), image('inpainted')),
      }),
      ['three'],
    )
    expect(summary).toEqual({ count: 1, unrendered: [], cleaned: [3], original: [] })
  })

  it('an emptied translation is not a translation', () => {
    const summary = exportSummary(
      pages({ a: page(image('source'), image('inpainted'), text({ translation: '  ' })) }),
    )
    expect(summary.cleaned).toEqual([1])
  })
})
