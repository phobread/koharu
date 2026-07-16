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

  it('Process all + export rendered runs all pages and exports after completion', async () => {
    const pipeline = {
      detector: 'detector',
      segmenter: 'segmenter',
      bubble_segmenter: 'bubble',
      font_detector: 'font',
      ocr: 'ocr',
      translator: 'translator',
      inpainter: 'inpainter',
      renderer: 'renderer',
    }
    const pipelineRequests: Array<Record<string, unknown>> = []
    let exportCalls = 0
    server.use(
      http.get('/api/v1/config', () => HttpResponse.json({ pipeline })),
      http.post('/api/v1/pipelines', async ({ request }) => {
        pipelineRequests.push((await request.json()) as Record<string, unknown>)
        return HttpResponse.json({ operationId: 'op-1' })
      }),
      http.get('/api/v1/operations', () =>
        HttpResponse.json({
          operations: [{ id: 'op-1', kind: 'pipeline', status: 'completed' }],
        }),
      ),
      http.post('/api/v1/projects/current/export', () => {
        exportCalls += 1
        return HttpResponse.arrayBuffer(new Uint8Array([0]).buffer, {
          headers: { 'content-type': 'application/zip' },
        })
      }),
    )
    queryClient.setQueryData(getGetConfigQueryKey(), { pipeline })
    useEditorUiStore.setState({ selectedLanguage: 'en-US' })
    usePreferencesStore.setState({ customSystemPrompt: 'write vividly' })

    renderWithQuery(<MenuBar />)
    await userEvent.click(screen.getByTestId('menu-process-trigger'))
    await userEvent.click(await screen.findByTestId('menu-process-all-export-rendered'))

    await waitFor(() => expect(pipelineRequests).toHaveLength(1))
    expect(pipelineRequests[0]).toMatchObject({
      steps: [
        'detector',
        'segmenter',
        'bubble',
        'font',
        'ocr',
        'translator',
        'inpainter',
        'renderer',
      ],
      targetLanguage: 'en-US',
      systemPrompt: 'write vividly',
    })
    expect(pipelineRequests[0]).not.toHaveProperty('pages')

    await waitFor(() => expect(exportCalls).toBe(1))
    expect(saveBlob).toHaveBeenCalledTimes(1)
  })
})
