import { describe, expect, it } from 'vitest'

import { pageStatus } from '@/lib/pageStatus'

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
