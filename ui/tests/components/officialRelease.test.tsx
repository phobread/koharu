import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import i18next from 'i18next'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { getGetConfigQueryKey, getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import enUS from '@/public/locales/en-US/translation.json'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

const mocks = vi.hoisted(() => ({ tauri: true, folder: vi.fn() }))

vi.mock('@/lib/backend', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/backend')>()),
  isTauri: () => mocks.tauri,
}))
vi.mock('@/lib/io/openFiles', () => ({
  openImageFiles: vi.fn().mockResolvedValue({ kind: 'paths', paths: [] }),
  openImageFolder: mocks.folder,
  openKhrFile: vi.fn().mockResolvedValue(null),
}))
// The desktop title bar asks the window whether it is maximised.
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ isMaximized: async () => false }),
}))

import { MenuBar } from '@/components/MenuBar'
import { WelcomeScreen } from '@/components/WelcomeScreen'
import { officialPagesNotice } from '@/lib/io/pagesIo'

const page = (id: string) => ({ id, name: id, width: 1, height: 1, nodes: {} })
const sceneWith = (...ids: string[]) => ({
  epoch: 0,
  scene: {
    project: { name: 'P' } as never,
    pages: Object.fromEntries(ids.map((id) => [id, page(id)])),
  },
})

async function english() {
  const i18n = i18next.createInstance()
  await i18n.init({ lng: 'en-US', resources: { 'en-US': { translation: enUS } } })
  return i18n.t
}

beforeEach(() => {
  mocks.tauri = true
  mocks.folder.mockReset()
  queryClient.clear()
})

describe('official release notice', () => {
  it('says how many pages got a release page and which found none', async () => {
    const t = await english()
    queryClient.setQueryData(getGetSceneJsonQueryKey(), sceneWith('a', 'b', 'c'))
    const res = (matched: string[], unmatchedPages: string[]) => ({
      matched: matched.map((p) => ({ page: p, file: `${p} eng.jpg` })),
      unmatchedPages,
      unusedFiles: [],
      rerender: [],
    })
    expect(officialPagesNotice(res(['a', 'b', 'c'], []), t)).toBe(
      'Official release added to 3 of 3 pages.',
    )
    expect(officialPagesNotice(res(['a', 'c'], ['b']), t)).toBe(
      'Official release added to 2 of 3 pages. No match for page 2.',
    )
    expect(officialPagesNotice(res([], ['a', 'b', 'c']), t)).toBe(
      'No page matched the official release. Its pages must be the same size and picture as yours.',
    )
  })
})

describe('Add Official Release (File menu)', () => {
  beforeEach(() => {
    server.use(
      http.get('/api/v1/scene.json', () => HttpResponse.json(sceneWith('p1', 'p2'))),
      http.get('/api/v1/config', () => HttpResponse.json({})),
    )
    queryClient.setQueryData(getGetSceneJsonQueryKey(), sceneWith('p1', 'p2'))
    queryClient.setQueryData(getGetConfigQueryKey(), {})
  })

  it('pairs the picked folder with the pages and tells what happened', async () => {
    const posts: unknown[] = []
    server.use(
      http.post('/api/v1/pages/official/from-paths', async ({ request }) => {
        posts.push(await request.json())
        return HttpResponse.json({
          matched: [{ page: 'p1', file: '001 eng.jpg' }],
          unmatchedPages: ['p2'],
          unusedFiles: [],
          rerender: [],
        })
      }),
    )
    mocks.folder.mockResolvedValue({
      kind: 'paths',
      paths: ['C:/eng/001 eng.jpg', 'C:/eng/002 eng.jpg'],
    })
    renderWithQuery(<MenuBar />)
    await userEvent.click(screen.getByTestId('menu-file-trigger'))
    await userEvent.click(await screen.findByTestId('menu-file-add-official'))

    await waitFor(() =>
      expect(posts).toEqual([{ paths: ['C:/eng/001 eng.jpg', 'C:/eng/002 eng.jpg'] }]),
    )
    // (Test i18n returns keys: "added to 1 of 2 pages. No match for page 2.")
    await waitFor(() =>
      expect(useEditorUiStore.getState().error?.message).toBe('official.added official.unmatched'),
    )
  })

  it('does nothing when the folder picker is cancelled, and is off outside the desktop app', async () => {
    let posted = false
    server.use(
      http.post('/api/v1/pages/official/from-paths', () => {
        posted = true
        return HttpResponse.json({})
      }),
    )
    mocks.folder.mockResolvedValue({ kind: 'paths', paths: [] })
    const { unmount } = renderWithQuery(<MenuBar />)
    await userEvent.click(screen.getByTestId('menu-file-trigger'))
    await userEvent.click(await screen.findByTestId('menu-file-add-official'))
    await waitFor(() => expect(mocks.folder).toHaveBeenCalled())
    expect(posted).toBe(false)
    unmount()

    mocks.tauri = false
    renderWithQuery(<MenuBar />)
    await userEvent.click(screen.getByTestId('menu-file-trigger'))
    expect(await screen.findByTestId('menu-file-add-official')).toHaveAttribute('data-disabled')
  })
})

describe('New project with raw and official folders', () => {
  it('names the project after the raw folder, imports it, then adds the release', async () => {
    const calls: Array<[string, unknown]> = []
    server.use(
      http.get('/api/v1/projects', () => HttpResponse.json({ projects: [] })),
      http.post('/api/v1/projects', async ({ request }) => {
        const body = (await request.json()) as { name: string }
        calls.push(['create', body])
        return HttpResponse.json({ id: 'x', name: body.name, path: '/tmp/x', updatedAtMs: 0 })
      }),
      http.post('/api/v1/pages/from-paths', async ({ request }) => {
        calls.push(['raw', await request.json()])
        return HttpResponse.json({ pages: ['p1'] })
      }),
      http.post('/api/v1/pages/official/from-paths', async ({ request }) => {
        calls.push(['official', await request.json()])
        return HttpResponse.json({
          matched: [{ page: 'p1', file: '001 eng.jpg' }],
          unmatchedPages: [],
          unusedFiles: [],
          rerender: [],
        })
      }),
    )
    renderWithQuery(<WelcomeScreen />)
    await userEvent.click(screen.getByRole('button', { name: /welcome\.new/i }))

    mocks.folder.mockResolvedValueOnce({
      kind: 'paths',
      paths: ['D:/Manhwa/BadEnd 3/001.jpg', 'D:/Manhwa/BadEnd 3/002.jpg'],
    })
    await userEvent.click(await screen.findByTestId('new-project-raw-folder'))
    const name = screen.getByPlaceholderText(/welcome\.newDialogPlaceholder/i)
    await waitFor(() => expect(name).toHaveValue('BadEnd 3'))
    expect(screen.getByTestId('new-project-raw-folder')).toHaveTextContent('welcome.folderPicked')

    mocks.folder.mockResolvedValueOnce({
      kind: 'paths',
      paths: ['D:/Official/001 eng.jpg'],
    })
    await userEvent.click(screen.getByTestId('new-project-official-folder'))
    await waitFor(() =>
      expect(screen.getByTestId('new-project-official-folder')).toHaveTextContent(
        'welcome.folderPicked',
      ),
    )

    await userEvent.click(screen.getByRole('button', { name: /welcome\.newDialogSubmit/i }))
    await waitFor(() =>
      expect(calls).toEqual([
        ['create', { name: 'BadEnd 3' }],
        [
          'raw',
          { paths: ['D:/Manhwa/BadEnd 3/001.jpg', 'D:/Manhwa/BadEnd 3/002.jpg'], replace: false },
        ],
        ['official', { paths: ['D:/Official/001 eng.jpg'] }],
      ]),
    )
  })

  it('shows no folder fields outside the desktop app', async () => {
    mocks.tauri = false
    server.use(http.get('/api/v1/projects', () => HttpResponse.json({ projects: [] })))
    renderWithQuery(<WelcomeScreen />)
    await userEvent.click(screen.getByRole('button', { name: /welcome\.new/i }))
    await screen.findByPlaceholderText(/welcome\.newDialogPlaceholder/i)
    expect(screen.queryByTestId('new-project-raw-folder')).toBeNull()
  })
})
