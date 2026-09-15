import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Node, Page, TextData } from '@/lib/api/schemas'
import { applyOp, queueAutoRender } from '@/lib/io/scene'
import { applyBlockMerge, applyBlockSplit } from '@/lib/io/splitNode'
import { splitTextBlock, splitTextBlockAt } from '@/lib/splitBlock'

vi.mock('@/lib/io/scene', () => ({
  applyOp: vi.fn().mockResolvedValue(undefined),
  queueAutoRender: vi.fn(),
}))

const transform = { x: 0, y: 0, width: 200, height: 100, rotationDeg: 0 }
const textNode = (id: string, text: TextData): Node => ({
  id,
  visible: true,
  transform,
  kind: { text },
})
const pageWith = (...nodes: Node[]): Page => ({
  id: 'page',
  name: 'Page',
  width: 800,
  height: 600,
  nodes: Object.fromEntries(nodes.map((node) => [node.id, node])),
})

describe('rich-text split and merge commands', () => {
  beforeEach(() => vi.clearAllMocks())

  it('keeps formatting on the second repeated word and inserts it beside the first half', async () => {
    const data = {
      translation: 'go go',
      writingDirection: 'vertical' as const,
      styleRanges: [{ start: 3, end: 5, style: { bold: true } }],
    }
    const page = pageWith(
      textNode('original', data),
      textNode('following', { translation: 'later' }),
    )
    const split = splitTextBlockAt(
      transform,
      data,
      { field: 'translation', offset: 3 },
      'vertical',
    )!
    await applyBlockSplit(page, 'original', split)
    expect(applyOp).toHaveBeenCalledWith({
      batch: {
        label: 'Split block',
        ops: [
          expect.objectContaining({
            updateNode: expect.objectContaining({
              id: 'original',
              patch: expect.objectContaining({
                data: { text: expect.objectContaining({ translation: 'go', styleRanges: [] }) },
              }),
            }),
          }),
          {
            addNode: {
              page: 'page',
              at: 1,
              node: expect.objectContaining({
                kind: {
                  text: expect.objectContaining({
                    translation: 'go',
                    writingDirection: 'vertical',
                    styleRanges: [{ start: 0, end: 2, style: { bold: true } }],
                  }),
                },
              }),
            },
          },
        ],
      },
    })
    expect(queueAutoRender).toHaveBeenCalledWith('page')
  })

  it('rebases repeated CJK and emoji using UTF-8 bytes', async () => {
    const data = {
      translation: '猫🙂 猫🙂',
      styleRanges: [{ start: 8, end: 15, style: { italic: true } }],
    }
    await applyBlockSplit(
      pageWith(textNode('original', data)),
      'original',
      splitTextBlock(transform, data),
    )
    expect(applyOp).toHaveBeenCalledWith(
      expect.objectContaining({
        batch: expect.objectContaining({
          ops: [
            expect.objectContaining({
              updateNode: expect.objectContaining({
                patch: expect.objectContaining({
                  data: { text: expect.objectContaining({ styleRanges: [] }) },
                }),
              }),
            }),
            expect.objectContaining({
              addNode: expect.objectContaining({
                node: expect.objectContaining({
                  kind: {
                    text: expect.objectContaining({
                      translation: '猫🙂',
                      styleRanges: [{ start: 0, end: 7, style: { italic: true } }],
                    }),
                  },
                }),
              }),
            }),
          ],
        }),
      }),
    )
  })

  it('preserves styles when splitting from the source-text caret', async () => {
    const data = {
      text: '가나 다라',
      translation: '🙂🙂',
      styleRanges: [{ start: 4, end: 8, style: { bold: true } }],
    }
    const split = splitTextBlockAt(transform, data, { field: 'text', offset: 3 }, 'horizontal')!
    await applyBlockSplit(pageWith(textNode('original', data)), 'original', split)
    expect(applyOp).toHaveBeenCalledWith(
      expect.objectContaining({
        batch: expect.objectContaining({
          ops: [
            expect.anything(),
            expect.objectContaining({
              addNode: expect.objectContaining({
                node: expect.objectContaining({
                  kind: {
                    text: expect.objectContaining({
                      translation: '🙂',
                      styleRanges: [{ start: 0, end: 4, style: { bold: true } }],
                    }),
                  },
                }),
              }),
            }),
          ],
        }),
      }),
    )
  })

  it('merges repeated fragments in page order with independently trimmed style ranges', async () => {
    const first = textNode('first', {
      translation: ' go ',
      styleRanges: [{ start: 1, end: 3, style: { bold: true } }],
    })
    const second = textNode('second', {
      translation: '  go  ',
      styleRanges: [{ start: 2, end: 4, style: { italic: true } }],
    })
    await applyBlockMerge(pageWith(first, second), ['second', 'first'])
    expect(applyOp).toHaveBeenCalledWith(
      expect.objectContaining({
        batch: expect.objectContaining({
          ops: [
            {
              updateNode: {
                page: 'page',
                id: 'first',
                patch: expect.objectContaining({
                  data: {
                    text: expect.objectContaining({
                      translation: 'go go',
                      styleRanges: [
                        { start: 0, end: 2, style: { bold: true } },
                        { start: 3, end: 5, style: { italic: true } },
                      ],
                    }),
                  },
                }),
              },
            },
            { removeNode: { page: 'page', id: 'second', prev_node: second, prev_index: 1 } },
          ],
        }),
      }),
    )
  })
})
