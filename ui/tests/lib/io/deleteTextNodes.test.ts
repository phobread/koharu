import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import { deleteTextNodes } from '@/lib/io/scene'
import { queryClient } from '@/lib/queryClient'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { server } from '../../msw/server'

const box = (id: string) => ({
  id,
  transform: { x: 0, y: 0, width: 10, height: 10, rotationDeg: 0 },
  visible: true,
  kind: { text: { text: id } },
})

const scene = {
  epoch: 3,
  scene: {
    project: { name: 'P' },
    pages: {
      p1: {
        id: 'p1',
        name: 'p1',
        width: 100,
        height: 100,
        nodes: {
          img: { id: 'img', visible: true, kind: { image: { role: 'source' } } },
          a: box('a'),
          b: box('b'),
          c: box('c'),
        },
      },
    },
  },
}

beforeEach(() => {
  queryClient.clear()
  queryClient.setQueryData(getGetSceneJsonQueryKey(), scene)
  useSelectionStore.getState().setPage('p1')
})

describe('deleteTextNodes', () => {
  it('removes the boxes in one batch with indices that track the shrinking list', async () => {
    const applied = vi.fn()
    server.use(
      http.get('/api/v1/scene.json', () => HttpResponse.json(scene)),
      http.post('/api/v1/history/apply', async ({ request }) => {
        applied(await request.json())
        return HttpResponse.json({ epoch: 4 })
      }),
    )
    useSelectionStore.getState().selectMany(['a', 'c', 'gone', 'img'])

    const removed = await deleteTextNodes('p1', useSelectionStore.getState().nodeIds)

    expect(removed).toBe(2)
    expect(applied).toHaveBeenCalledTimes(1)
    const op = applied.mock.calls[0][0]
    expect(op.batch.ops.map((o: any) => [o.removeNode.id, o.removeNode.prev_index])).toEqual([
      ['a', 1],
      ['c', 2],
    ])
    // Deleted boxes leave the selection; ids that were not deleted stay.
    expect([...useSelectionStore.getState().nodeIds].sort()).toEqual(['gone', 'img'])
  })

  it('sends a single delete as a plain removal and skips the call when nothing matches', async () => {
    const applied = vi.fn()
    server.use(
      http.get('/api/v1/scene.json', () => HttpResponse.json(scene)),
      http.post('/api/v1/history/apply', async ({ request }) => {
        applied(await request.json())
        return HttpResponse.json({ epoch: 4 })
      }),
    )

    expect(await deleteTextNodes('p1', ['b'])).toBe(1)
    expect(applied.mock.calls[0][0].removeNode.id).toBe('b')

    expect(await deleteTextNodes('p1', ['missing'])).toBe(0)
    expect(applied).toHaveBeenCalledTimes(1)
  })
})
