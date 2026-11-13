import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { RenderControlsPanel } from '@/components/panels/RenderControlsPanel'
import * as sceneActions from '@/lib/io/scene'
import { queryClient } from '@/lib/queryClient'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

vi.mock('@/lib/io/scene', async () => {
  const actual = await vi.importActual<any>('@/lib/io/scene')
  return { ...actual, applyOp: vi.fn(), queueAutoRender: vi.fn() }
})

const textBox = (id: string, rendered: number, style: Record<string, unknown> | null = null) => ({
  id,
  transform: { x: 100, y: 100, width: 200, height: 100, rotationDeg: 0 },
  visible: true,
  kind: { text: { translation: 'Hello there', renderedFontSizePx: rendered, style } },
})

function serveBoxes(boxes: ReturnType<typeof textBox>[]) {
  server.use(
    http.get('/api/v1/fonts', () => HttpResponse.json([])),
    http.get('/api/v1/scene.json', () =>
      HttpResponse.json({
        epoch: 1,
        scene: {
          pages: {
            p1: { id: 'p1', name: 'P1', nodes: Object.fromEntries(boxes.map((b) => [b.id, b])) },
          },
          project: { name: 'Proj' },
        },
      }),
    ),
  )
}

describe('Even out sizes', () => {
  beforeEach(() => {
    queryClient.clear()
    useSelectionStore.getState().setPage('p1')
    useSelectionStore.getState().clear()
    vi.clearAllMocks()
  })

  it('sets every box on the page to the middle size in one undo step', async () => {
    serveBoxes([textBox('a', 10), textBox('b', 20, { fontFamilies: ['Arial'] }), textBox('c', 40)])
    renderWithQuery(<RenderControlsPanel />)

    const button = await screen.findByTestId('render-even-sizes')
    await waitFor(() => expect(button).toBeEnabled())
    expect(button).toHaveTextContent('20')
    await userEvent.click(button)

    await waitFor(() => expect(sceneActions.applyOp).toHaveBeenCalledTimes(1))
    const { batch } = (sceneActions.applyOp as any).mock.calls[0][0]
    expect(batch.ops).toHaveLength(3)
    const patch = (id: string) =>
      batch.ops.find((op: any) => op.updateNode.id === id).updateNode.patch

    // Smaller text: the box grows and becomes the layout boundary.
    expect(patch('a').data.text.style.fontSize).toBe(20)
    expect(patch('a').data.text.lockLayoutBox).toBe(true)
    expect(patch('a').transform.width).toBeGreaterThan(400)
    // Already the middle size: only the size is pinned, other styling is kept.
    expect(patch('b').transform).toBeUndefined()
    expect(patch('b').data.text.style).toMatchObject({ fontSize: 20, fontFamilies: ['Arial'] })
    // Bigger text: the box shrinks.
    expect(patch('c').transform.width).toBeLessThan(200)
    expect(sceneActions.queueAutoRender).toHaveBeenCalledWith('p1')
  })

  it('works on the selected boxes only', async () => {
    serveBoxes([textBox('a', 10), textBox('b', 20), textBox('c', 40), textBox('d', 40)])
    renderWithQuery(<RenderControlsPanel />)
    const button = await screen.findByTestId('render-even-sizes')
    await waitFor(() => expect(button).toBeEnabled())
    useSelectionStore.getState().selectMany(['a', 'b'])
    await waitFor(() => expect(button).toHaveTextContent('15'))
    await userEvent.click(button)

    await waitFor(() => expect(sceneActions.applyOp).toHaveBeenCalledTimes(1))
    const { batch } = (sceneActions.applyOp as any).mock.calls[0][0]
    expect(batch.ops.map((op: any) => op.updateNode.id).sort()).toEqual(['a', 'b'])
  })

  it('is off when there is nothing to even out', async () => {
    serveBoxes([textBox('a', 20)])
    renderWithQuery(<RenderControlsPanel />)
    expect(await screen.findByTestId('render-even-sizes')).toBeDisabled()
  })
})
