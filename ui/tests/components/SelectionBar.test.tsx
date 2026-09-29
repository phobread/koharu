import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { SelectionBar } from '@/components/canvas/SelectionBar'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

const box = (id: string) => ({
  id,
  transform: { x: 0, y: 0, width: 10, height: 10, rotationDeg: 0 },
  visible: true,
  kind: { text: { text: id } },
})

const scene = {
  epoch: 1,
  scene: {
    project: { name: 'P' },
    pages: {
      p1: {
        id: 'p1',
        name: 'p1',
        width: 100,
        height: 100,
        nodes: { a: box('a'), b: box('b'), c: box('c') },
      },
    },
  },
}

beforeEach(() => {
  server.use(http.get('/api/v1/scene.json', () => HttpResponse.json(scene)))
  useSelectionStore.getState().setPage('p1')
})

describe('SelectionBar', () => {
  it('stays hidden for a single selected box', async () => {
    useSelectionStore.getState().selectMany(['a'])
    renderWithQuery(<SelectionBar pageId='p1' />)
    await waitFor(() => expect(screen.queryByTestId('selection-bar')).not.toBeInTheDocument())
  })

  it('deletes every selected box with one click and one undo step', async () => {
    const applied = vi.fn()
    server.use(
      http.post('/api/v1/history/apply', async ({ request }) => {
        applied(await request.json())
        return HttpResponse.json({ epoch: 2 })
      }),
    )
    useSelectionStore.getState().selectMany(['a', 'c'])
    renderWithQuery(<SelectionBar pageId='p1' />)

    await userEvent.click(await screen.findByTestId('selection-delete'))

    await waitFor(() => expect(applied).toHaveBeenCalledTimes(1))
    expect(applied.mock.calls[0][0].batch.ops.map((o: any) => o.removeNode.id)).toEqual(['a', 'c'])
  })

  it('clears the selection', async () => {
    useSelectionStore.getState().selectMany(['a', 'b'])
    renderWithQuery(<SelectionBar pageId='p1' />)
    await userEvent.click(await screen.findByTestId('selection-clear'))
    expect(useSelectionStore.getState().nodeIds.size).toBe(0)
  })
})
