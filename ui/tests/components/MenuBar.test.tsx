import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import i18next from 'i18next'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { exportNotice, MenuBar } from '@/components/MenuBar'
import { getGetConfigQueryKey, getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import { saveBlob } from '@/lib/io/saveBlob'
import type { ExportSummary } from '@/lib/pageStatus'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'
import enUS from '@/public/locales/en-US/translation.json'

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
  describe('export', () => {
    it('the notice names the pages that went out without their translation', async () => {
      const i18n = i18next.createInstance()
      await i18n.init({ lng: 'en-US', resources: { 'en-US': { translation: enUS } } })
      const notice = (summary: Partial<ExportSummary>) =>
        exportNotice({ count: 1, unrendered: [], cleaned: [], original: [], ...summary }, i18n.t)

      expect(notice({})).toBe('Exported 1 page.')
      expect(notice({ count: 24, unrendered: [7], cleaned: [5, 9], original: [12] })).toBe(
        'Exported 24 pages. Not rendered yet, so without the translation: page 7.' +
          ' No translation yet, exported cleaned: pages 5, 9.' +
          ' Not cleaned yet, exported as the original: page 12.',
      )
      expect(notice({ count: 30, original: Array.from({ length: 12 }, (_, i) => i + 1) })).toBe(
        'Exported 30 pages. Not cleaned yet, exported as the original: pages 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 and 2 more.',
      )
    })

    let exports: Array<Record<string, unknown>>
    const layer = (role: string) => ({ id: role, visible: true, kind: { image: { role } } })
    const scene = {
      epoch: 0,
      scene: {
        project: { name: 'P' },
        pages: {
          p1: { id: 'p1', name: '1', width: 1, height: 1, nodes: { r: layer('rendered') } },
          p2: { id: 'p2', name: '2', width: 1, height: 1, nodes: { s: layer('source') } },
        },
      },
    }

    beforeEach(() => {
      exports = []
      server.use(
        http.get('/api/v1/scene.json', () => HttpResponse.json(scene)),
        http.post('/api/v1/projects/current/export', async ({ request }) => {
          exports.push((await request.json()) as Record<string, unknown>)
          return HttpResponse.arrayBuffer(new Uint8Array([0]).buffer, {
            headers: { 'content-type': 'application/zip' },
          })
        }),
      )
      queryClient.setQueryData(getGetSceneJsonQueryKey(), scene)
      useSelectionStore.getState().setPage('p2')
      useEditorUiStore.getState().clearError()
    })

    it('a click on Export all pages exports every page and says what went out', async () => {
      renderWithQuery(<MenuBar />)
      await userEvent.click(screen.getByTestId('menu-file-trigger'))
      await userEvent.click(await screen.findByTestId('menu-file-export'))

      await waitFor(() => expect(exports).toEqual([{ format: 'best' }]))
      await waitFor(() =>
        expect(useEditorUiStore.getState().error).toMatchObject({
          notice: true,
          message: 'menu.exportDone menu.exportOriginal',
        }),
      )
      // The menu closed instead of opening the submenu.
      expect(screen.queryByTestId('menu-export-page')).not.toBeInTheDocument()
      useEditorUiStore.getState().clearError()
    })

    it('the submenu exports this page, a PSD, the cleaned pages or the project archive', async () => {
      renderWithQuery(<MenuBar />)
      const pick = async (item: string) => {
        await userEvent.click(screen.getByTestId('menu-file-trigger'))
        await userEvent.hover(await screen.findByTestId('menu-file-export'))
        await userEvent.click(await screen.findByTestId(item))
      }
      await pick('menu-export-page')
      await pick('menu-export-psd')
      await pick('menu-export-cleaned')
      await pick('menu-export-khr')

      await waitFor(() => expect(exports).toHaveLength(4))
      expect(exports).toEqual([
        { format: 'best', pages: ['p2'] },
        { format: 'psd', pages: ['p2'] },
        { format: 'inpainted' },
        { format: 'khr' },
      ])
      expect(saveBlob).toHaveBeenCalled()
      useEditorUiStore.getState().clearError()
    })
  })

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
