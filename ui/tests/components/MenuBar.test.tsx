import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { MenuBar } from '@/components/MenuBar'
import { getGetConfigQueryKey, getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import { saveBlob } from '@/lib/io/saveBlob'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

vi.mock('@/lib/io/openFiles', () => ({
  openImageFiles: vi.fn().mockResolvedValue([]),
  openImageFolder: vi.fn().mockResolvedValue([]),
  openKhrFile: vi.fn().mockResolvedValue(null),
}))

vi.mock('@/lib/io/saveBlob', async () => {
  const actual = await vi.importActual<typeof import('@/lib/io/saveBlob')>('@/lib/io/saveBlob')
  return {
    ...actual,
    pickSaveDirectory: vi.fn().mockResolvedValue(undefined),
    saveBlob: vi.fn().mockResolvedValue(true),
    saveBlobToDirectory: vi.fn().mockResolvedValue(true),
  }
})

beforeEach(() => {
  // Default: config + scene exist so the menu enables scene-dependent items.
  server.use(
    http.get('/api/v1/scene.json', () =>
      HttpResponse.json({
        epoch: 0,
        scene: { pages: {}, project: { name: 'P' } as never },
      }),
    ),
    http.get('/api/v1/config', () => HttpResponse.json({})),
  )
  queryClient.setQueryData(getGetSceneJsonQueryKey(), {
    epoch: 0,
    scene: { pages: {}, project: { name: 'P' } },
  })
  queryClient.setQueryData(getGetConfigQueryKey(), {})
})

describe('MenuBar', () => {
  it('rebuilds masks using kept boxes without rerunning detection, OCR, or translation', async () => {
    const pipeline = {
      detector: 'detect',
      segmenter: 'segment',
      inpainter: 'inpaint',
      renderer: 'render',
    }
    const requests: Array<Record<string, unknown>> = []
    server.use(
      http.get('/api/v1/config', () => HttpResponse.json({ pipeline })),
      http.post('/api/v1/pipelines', async ({ request }) => {
        requests.push((await request.json()) as Record<string, unknown>)
        return HttpResponse.json({ operationId: 'repair' })
      }),
    )
    useSelectionStore.setState({ pageId: 'kept-page' })
    renderWithQuery(<MenuBar />)
    await userEvent.click(screen.getByTestId('menu-process-trigger'))
    await userEvent.hover(await screen.findByTestId('menu-rebuild-masks'))
    await userEvent.click(await screen.findByTestId('menu-rebuild-mask-current'))
    await waitFor(() => expect(requests).toHaveLength(1))
    expect(requests[0]).toMatchObject({
      steps: ['segment', 'inpaint', 'render'],
      pages: ['kept-page'],
    })
  })

  it('renders File / View / Process / Help triggers', async () => {
    renderWithQuery(<MenuBar />)
    expect(screen.getByTestId('menu-file-trigger')).toBeInTheDocument()
    expect(screen.getByTestId('menu-process-trigger')).toBeInTheDocument()
  })

  it('Close Project calls DELETE /projects/current and invalidates scene', async () => {
    let deleted = 0
    server.use(
      http.delete('/api/v1/projects/current', () => {
        deleted += 1
        return new HttpResponse(null, { status: 204 })
      }),
    )
    const invalidateSpy = vi.spyOn(queryClient, 'invalidateQueries')

    renderWithQuery(<MenuBar />)
    await userEvent.click(screen.getByTestId('menu-file-trigger'))
    const close = await screen.findByTestId('menu-file-close-project')
    expect(close).toHaveTextContent('Ctrl+W')
    await userEvent.click(close)

    await waitFor(() => expect(deleted).toBe(1))
    await waitFor(() => {
      const invalidatedKeys = invalidateSpy.mock.calls.map((c) => c[0]?.queryKey)
      expect(invalidatedKeys).toContainEqual(getGetSceneJsonQueryKey())
    })
  })

  it('shows the project name with a back arrow that closes the project', async () => {
    let deleted = 0
    server.use(
      http.delete('/api/v1/projects/current', () => {
        deleted += 1
        return new HttpResponse(null, { status: 204 })
      }),
    )
    renderWithQuery(<MenuBar />)
    expect(await screen.findByTestId('project-name')).toHaveTextContent('P')
    const back = screen.getByTestId('project-back')
    expect(back).toHaveAttribute('title', 'project.back (Ctrl+W)')
    await userEvent.click(back)
    await waitFor(() => expect(deleted).toBe(1))
  })

  it('hides the project name when no project is open', async () => {
    queryClient.clear()
    server.use(
      http.get('/api/v1/scene.json', () =>
        HttpResponse.json({ message: 'no project' }, { status: 400 }),
      ),
    )
    renderWithQuery(<MenuBar />)
    await waitFor(() => expect(queryClient.isFetching()).toBe(0))
    expect(screen.getByTestId('menu-file-trigger')).toBeInTheDocument()
    expect(screen.queryByTestId('project-back')).not.toBeInTheDocument()
  })

  it('Close Project is disabled when no project is open', async () => {
    // Clear seeded cache + point /scene.json at the 400 response so useScene
    // resolves to null.
    queryClient.clear()
    server.use(
      http.get('/api/v1/scene.json', () =>
        HttpResponse.json({ message: 'no project' }, { status: 400 }),
      ),
    )
    renderWithQuery(<MenuBar />)
    await waitFor(() => expect(queryClient.isFetching()).toBe(0))
    await userEvent.click(screen.getByTestId('menu-file-trigger'))
    const close = await screen.findByTestId('menu-file-close-project')
    expect(close).toHaveAttribute('data-disabled')
  })

  describe('processing only what is missing', () => {
    const pipeline = { detector: 'detector', ocr: 'ocr', renderer: 'renderer' }
    const pageScene = (ids: string[]) => {
      const pages: Record<string, unknown> = {}
      for (const id of ids) pages[id] = { id, name: id, width: 10, height: 10, nodes: {} }
      return { epoch: 0, scene: { pages, project: { name: 'P' } } }
    }
    let requests: Array<Record<string, unknown>>

    beforeEach(() => {
      requests = []
      const scene = pageScene(['p1', 'p2', 'p3'])
      server.use(
        http.get('/api/v1/scene.json', () => HttpResponse.json(scene)),
        http.get('/api/v1/config', () => HttpResponse.json({ pipeline })),
        http.post('/api/v1/pipelines', async ({ request }) => {
          requests.push((await request.json()) as Record<string, unknown>)
          return HttpResponse.json({ operationId: 'op-2', pageCount: 0 })
        }),
      )
      queryClient.setQueryData(getGetSceneJsonQueryKey(), scene)
      useSelectionStore.getState().setPage('p1')
      useEditorUiStore.getState().clearError()
    })

    it('Process unfinished pages keeps finished work and says when nothing is left', async () => {
      renderWithQuery(<MenuBar />)
      await userEvent.click(screen.getByTestId('menu-process-trigger'))
      await userEvent.click(await screen.findByTestId('menu-process-all'))

      await waitFor(() => expect(requests).toHaveLength(1))
      expect(requests[0]).toMatchObject({
        onlyMissing: true,
        steps: ['detector', 'ocr', 'renderer'],
      })
      expect(requests[0]).not.toHaveProperty('pages')
      await waitFor(() => expect(useEditorUiStore.getState().error?.notice).toBe(true))
      useEditorUiStore.getState().clearError()
    })

    it('Process selected pages sends the page-list selection in page order', async () => {
      useSelectionStore.getState().setSelectedPageIds(new Set(['p3', 'p1']))
      renderWithQuery(<MenuBar />)
      await userEvent.click(screen.getByTestId('menu-process-trigger'))
      await userEvent.click(await screen.findByTestId('menu-process-selected'))

      await waitFor(() => expect(requests).toHaveLength(1))
      expect(requests[0]).toMatchObject({ pages: ['p1', 'p3'], onlyMissing: true })
    })

    it('Process selected pages needs two or more selected pages', async () => {
      useSelectionStore.getState().setSelectedPageIds(new Set(['p1']))
      renderWithQuery(<MenuBar />)
      await userEvent.click(screen.getByTestId('menu-process-trigger'))
      expect(await screen.findByTestId('menu-process-selected')).toHaveAttribute('data-disabled')
    })

    it('the step ticks decide what Process runs', async () => {
      usePreferencesStore.getState().setProcessSteps({ translate: false, render: false })
      renderWithQuery(<MenuBar />)
      await userEvent.click(screen.getByTestId('menu-process-trigger'))
      expect(await screen.findByTestId('menu-process-step-render')).toHaveAttribute(
        'data-state',
        'unchecked',
      )
      await userEvent.click(screen.getByTestId('menu-process-all'))
      await waitFor(() => expect(requests).toHaveLength(1))
      expect(requests[0]).toMatchObject({ steps: ['detector', 'ocr'], onlyMissing: true })
      usePreferencesStore.getState().setProcessSteps({ translate: true, render: true })
    })

    it('with nothing ticked there is nothing to start', async () => {
      usePreferencesStore.getState().setProcessSteps({
        detect: false,
        ocr: false,
        translate: false,
        inpaint: false,
        render: false,
      })
      renderWithQuery(<MenuBar />)
      await userEvent.click(screen.getByTestId('menu-process-trigger'))
      expect(await screen.findByTestId('menu-process-all')).toHaveAttribute('data-disabled')
      expect(screen.getByTestId('menu-process-redo-all')).toHaveAttribute('data-disabled')
      usePreferencesStore.getState().setProcessSteps({
        detect: true,
        ocr: true,
        translate: true,
        inpaint: true,
        render: true,
      })
    })

    it('Redo all pages asks first, then redoes every step', async () => {
      renderWithQuery(<MenuBar />)
      await userEvent.click(screen.getByTestId('menu-process-trigger'))
      await userEvent.click(await screen.findByTestId('menu-process-redo-all'))
      expect(requests).toHaveLength(0)

      await userEvent.click(await screen.findByTestId('redo-all-confirm'))
      await waitFor(() => expect(requests).toHaveLength(1))
      expect(requests[0]).toMatchObject({ onlyMissing: false })
      expect(useEditorUiStore.getState().error).toBeUndefined()
    })
  })
})
