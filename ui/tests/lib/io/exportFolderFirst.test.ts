import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import { queryClient } from '@/lib/queryClient'

import { server } from '../../msw/server'

// The desktop app: several pages are saved into a folder picked first.
vi.mock('@/lib/backend', async () => {
  const actual = await vi.importActual<typeof import('@/lib/backend')>('@/lib/backend')
  return { ...actual, isTauri: () => true }
})
vi.mock('@/lib/io/saveBlob', async () => {
  const actual = await vi.importActual<typeof import('@/lib/io/saveBlob')>('@/lib/io/saveBlob')
  return {
    ...actual,
    saveBlob: vi.fn().mockResolvedValue(true),
    saveBlobToDirectory: vi.fn().mockResolvedValue(true),
    pickSaveDirectory: vi.fn(),
    prepareExportDirectory: vi.fn(async () => 'D:/Manga/P/Rendered'),
  }
})

import { exportCurrentProjectAs } from '@/lib/io/pagesIo'
import { pickSaveDirectory, saveBlob, saveBlobToDirectory } from '@/lib/io/saveBlob'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

const asMock = (fn: unknown) => fn as ReturnType<typeof vi.fn>

const zip = () =>
  HttpResponse.arrayBuffer(new Uint8Array([0]).buffer, {
    headers: { 'content-type': 'application/zip' },
  })

const page = (id: string) => ({ id, name: id, width: 10, height: 10, nodes: {} })

beforeEach(() => {
  useEditorUiStore.getState().clearError()
  queryClient.setQueryData(getGetSceneJsonQueryKey(), {
    epoch: 0,
    scene: {
      pages: { a: page('a'), b: page('b'), c: page('c') },
      project: { name: 'P' } as never,
    },
  })
})

describe('exporting several pages in the desktop app', () => {
  it('asks for the folder before building the export, then saves into it', async () => {
    const events: string[] = []
    asMock(pickSaveDirectory).mockImplementation(async () => {
      events.push('pick')
      return 'D:/Out'
    })
    server.use(
      http.post('/api/v1/projects/current/export', () => {
        events.push('export')
        return zip()
      }),
    )

    expect(await exportCurrentProjectAs('best')).toBe(true)

    expect(events).toEqual(['pick', 'export'])
    expect(pickSaveDirectory).toHaveBeenCalledWith('D:/Manga/P/Rendered')
    expect(saveBlob).not.toHaveBeenCalled()
    const [, , folder] = asMock(saveBlobToDirectory).mock.calls[0]
    expect(folder).toBe('D:/Out')
  })

  it('builds nothing when the folder pick is cancelled', async () => {
    asMock(pickSaveDirectory).mockResolvedValue(undefined)
    let exports = 0
    server.use(
      http.post('/api/v1/projects/current/export', () => {
        exports += 1
        return zip()
      }),
    )

    expect(await exportCurrentProjectAs('best')).toBe(false)

    expect(exports).toBe(0)
    expect(saveBlobToDirectory).not.toHaveBeenCalled()
  })

  it('shows a notice that stays while it works, and takes it down after', async () => {
    asMock(pickSaveDirectory).mockResolvedValue('D:/Out')
    let shownWhileWorking: string | undefined
    server.use(
      http.post('/api/v1/projects/current/export', () => {
        shownWhileWorking = useEditorUiStore.getState().error?.message
        return zip()
      }),
    )

    await exportCurrentProjectAs('best', ['a', 'b'])

    expect(shownWhileWorking).toBe('Exporting 2 pages…')
    expect(useEditorUiStore.getState().error).toBeUndefined()
  })

  it('replaces the notice with the error when the export fails', async () => {
    asMock(pickSaveDirectory).mockResolvedValue('D:/Out')
    server.use(
      http.post('/api/v1/projects/current/export', () =>
        HttpResponse.json(
          { message: 'no pages have the requested layer populated' },
          { status: 400 },
        ),
      ),
    )

    expect(await exportCurrentProjectAs('rendered')).toBe(false)

    const error = useEditorUiStore.getState().error
    expect(error?.notice).toBeUndefined()
    expect(error?.message).toMatch(/No rendered images to export yet/)
  })

  it('keeps the save dialog after the build for a single page', async () => {
    server.use(http.post('/api/v1/projects/current/export', () => zip()))

    await exportCurrentProjectAs('best', ['a'])

    expect(pickSaveDirectory).not.toHaveBeenCalled()
    expect(saveBlob).toHaveBeenCalledTimes(1)
    expect(useEditorUiStore.getState().error).toBeUndefined()
  })
})
