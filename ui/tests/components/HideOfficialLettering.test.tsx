import { fireEvent, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

// Only the canvas and its right-click menu are under test: the rails, the
// toolbar and the box layer have their own tests, and jsdom paints nothing.
vi.mock('@/components/canvas/ToolRail', () => ({ ToolRail: () => null }))
vi.mock('@/components/canvas/SubToolRail', () => ({ SubToolRail: () => null }))
vi.mock('@/components/canvas/CanvasToolbar', () => ({ CanvasToolbar: () => null }))
vi.mock('@/components/canvas/SelectionBar', () => ({ SelectionBar: () => null }))
vi.mock('@/components/canvas/TextBlockLayer', () => ({ TextBlockLayer: () => null }))
vi.mock('@/hooks/useBlobData', () => ({ useBlobData: () => undefined }))
vi.mock('@/hooks/useKeyboardShortcuts', () => ({ useKeyboardShortcuts: () => {} }))

import { Workspace } from '@/components/canvas/Workspace'

const image = (id: string, role: string) => ({
  id,
  visible: role !== 'official',
  transform: { x: 0, y: 0, width: 0, height: 0, rotationDeg: 0 },
  kind: { image: { role, blob: `blob-${id}`, opacity: 1, naturalWidth: 400, naturalHeight: 300 } },
})

const scene = (official: boolean) => ({
  epoch: 1,
  scene: {
    project: { name: 'P' },
    pages: {
      p1: {
        id: 'p1',
        name: '026.jpg',
        width: 400,
        height: 300,
        nodes: {
          src: image('src', 'source'),
          ...(official ? { off: image('off', 'official') } : {}),
          cleaned: image('cleaned', 'inpainted'),
          box: {
            id: 'box',
            visible: true,
            transform: { x: 100, y: 100, width: 80, height: 40, rotationDeg: 0 },
            kind: { text: { text: 'hi' } },
          },
        },
      },
    },
  },
})

function open(official: boolean) {
  const data = scene(official)
  server.use(http.get('/api/v1/scene.json', () => HttpResponse.json(data)))
  queryClient.setQueryData(getGetSceneJsonQueryKey(), data)
  useSelectionStore.getState().setPage('p1')
  renderWithQuery(<Workspace />)
  return screen.getByTestId('workspace-canvas')
}

const ITEM = 'workspace-hide-official-lettering'

describe("Hide the release's lettering here (canvas right-click)", () => {
  beforeEach(() => {
    // jsdom has no 2D canvas; the drawing hooks take a missing context in stride.
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null)
    useEditorUiStore.setState({ scale: 100, mode: 'select' })
    useEditorUiStore.getState().clearError()
  })

  it('hides the lettering at the point right-clicked on a page with a release', async () => {
    const posts: unknown[] = []
    server.use(
      http.post('/api/v1/pages/:id/official/hide-lettering', async ({ request, params }) => {
        posts.push({ page: params.id, body: await request.json() })
        return HttpResponse.json({ status: 'hidden', node: 'n-new' })
      }),
    )
    const canvas = open(true)

    // jsdom lays nothing out: the canvas sits at 0,0 and the zoom is 100%,
    // so the client point is the page point.
    fireEvent.contextMenu(canvas, { clientX: 250, clientY: 230 })
    const item = await screen.findByTestId(ITEM)
    expect(item).toHaveTextContent('workspace.hideOfficialLettering')
    expect(item).not.toHaveAttribute('data-disabled')
    await userEvent.click(item)

    await waitFor(() => expect(posts).toEqual([{ page: 'p1', body: { x: 250, y: 230 } }]))
    await waitFor(() =>
      expect(useEditorUiStore.getState().error).toMatchObject({
        message: 'official.hidden',
        notice: true,
      }),
    )
  })

  it('is offered but off on a text box, which is the owner’s already', async () => {
    const canvas = open(true)
    fireEvent.contextMenu(canvas, { clientX: 120, clientY: 110 })
    expect(await screen.findByTestId(ITEM)).toHaveAttribute('data-disabled')
  })

  it('is not offered on a page without a release: the picture has no menu there', async () => {
    const canvas = open(false)
    // The picture: no menu at all, as before.
    fireEvent.contextMenu(canvas, { clientX: 250, clientY: 230 })
    expect(screen.queryByRole('menu')).toBeNull()
    // A box: the box menu, without the action.
    fireEvent.contextMenu(canvas, { clientX: 120, clientY: 110 })
    await screen.findByText('workspace.deleteBlock')
    expect(screen.queryByTestId(ITEM)).toBeNull()
  })
})
