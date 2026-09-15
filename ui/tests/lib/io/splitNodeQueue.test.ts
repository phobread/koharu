import { afterEach, describe, expect, it, vi } from 'vitest'

import { applyCommand, getSceneJson } from '@/lib/api/default/default'
import type { Scene } from '@/lib/api/schemas'
import { applyOp } from '@/lib/io/scene'
import { splitBlock } from '@/lib/io/splitNode'

vi.mock('@/lib/api/default/default', () => ({
  applyCommand: vi.fn().mockResolvedValue({ epoch: 1 }),
  getSceneJson: vi.fn(),
  getGetSceneJsonQueryKey: () => ['scene'],
}))
vi.mock('@/lib/queryClient', () => ({
  queryClient: { invalidateQueries: vi.fn().mockResolvedValue(undefined) },
}))
afterEach(() => {
  vi.clearAllTimers()
  vi.useRealTimers()
  vi.clearAllMocks()
})

describe('queued block splitting', () => {
  it('waits for pending text saves before slicing the latest text and styles', async () => {
    vi.useFakeTimers()
    const data = { translation: 'go go', renderedDirection: 'vertical', styleRanges: [] } as any
    const scene = {
      pages: {
        page: {
          id: 'page',
          nodes: {
            original: {
              id: 'original',
              visible: true,
              transform: { x: 0, y: 0, width: 200, height: 100, rotationDeg: 0 },
              kind: { text: data },
            },
          },
        },
      },
    } as unknown as Scene
    let finishSave!: () => void
    const saved = new Promise<void>((resolve) => {
      finishSave = resolve
    })
    vi.mocked(applyCommand).mockImplementationOnce(async () => {
      await saved
      data.translation = 'go go plus'
      data.styleRanges = [{ start: 3, end: 10, style: { bold: true } }]
      return { epoch: 1 }
    })
    vi.mocked(getSceneJson).mockImplementation(async () => ({ epoch: 1, scene }))
    const typing = applyOp({
      updateNode: {
        page: 'page',
        id: 'original',
        patch: {
          data: { text: { translation: 'go go plus' } },
        },
      },
    })
    const split = splitBlock('page', 'original', { field: 'translation', offset: 3 })
    await Promise.resolve()
    expect(getSceneJson).not.toHaveBeenCalled()
    finishSave()
    await Promise.all([typing, split])
    const batch = (vi.mocked(applyCommand).mock.calls.at(-1)![0] as any).batch
    const first = batch.ops[0].updateNode.patch
    const second = batch.ops[1].addNode.node
    expect(first.data.text.translation).toBe('go')
    expect(second.kind.text.translation).toBe('go plus')
    expect(second.kind.text.styleRanges).toEqual([{ start: 0, end: 7, style: { bold: true } }])
    expect(second.kind.text.renderedDirection).toBe('vertical')
    expect(first.transform.x).toBeGreaterThan(second.transform.x)
  })
})
