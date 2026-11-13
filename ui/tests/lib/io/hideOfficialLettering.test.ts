import type { TFunction } from 'i18next'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { hideOfficialLetteringAt } from '@/lib/io/hideOfficialLettering'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

import { server } from '../../msw/server'

// Keys stand in for the sentences, as in the component tests.
const t = ((key: string) => key) as unknown as TFunction

const notice = () => useEditorUiStore.getState().error

type Calls = { hide: unknown[]; sceneReads: number; renders: Array<{ pages: string[] }> }

/** Answer the hide route with `reply` and record what the action does next. */
function backend(reply: () => Response): Calls {
  const calls: Calls = { hide: [], sceneReads: 0, renders: [] }
  server.use(
    http.post('/api/v1/pages/:id/official/hide-lettering', async ({ request, params }) => {
      calls.hide.push({ page: params.id, body: await request.json() })
      return reply()
    }),
    http.get('/api/v1/scene.json', () => {
      calls.sceneReads += 1
      return HttpResponse.json({ epoch: 2, scene: { project: { name: 'P' }, pages: {} } })
    }),
    http.get('/api/v1/config', () =>
      HttpResponse.json({ pipeline: { renderer: 'koharu-renderer' } }),
    ),
    http.post('/api/v1/pipelines', async ({ request }) => {
      calls.renders.push((await request.json()) as { pages: string[] })
      return HttpResponse.json({ operationId: 'op-1', pageCount: 1 })
    }),
  )
  return calls
}

describe('hideOfficialLetteringAt', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    useEditorUiStore.getState().clearError()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('sends the point, then shows the page again and renders it afresh', async () => {
    const calls = backend(() => HttpResponse.json({ status: 'hidden', node: 'n-new' }))

    await hideOfficialLetteringAt('p1', { x: 494.5, y: 2322 }, t)

    expect(calls.hide).toEqual([{ page: 'p1', body: { x: 494.5, y: 2322 } }])
    expect(notice()).toMatchObject({ message: 'official.hidden', notice: true })
    // The Translated view still shows the line until it is rendered again.
    expect(calls.renders).toHaveLength(0)
    await vi.advanceTimersByTimeAsync(600)
    expect(calls.renders.map((r) => r.pages)).toEqual([['p1']])
  })

  it('says so when there is no lettering of the release there, and changes nothing', async () => {
    const calls = backend(() => HttpResponse.json({ status: 'nothingThere', node: null }))

    await hideOfficialLetteringAt('p1', { x: 10, y: 20 }, t)

    expect(calls.hide).toHaveLength(1)
    expect(notice()).toMatchObject({ message: 'official.hideNothing', notice: true })
    await vi.advanceTimersByTimeAsync(600)
    expect(calls.renders).toHaveLength(0)
    expect(calls.sceneReads).toBe(0)
  })

  it('explains lettering that cannot be hidden on its own', async () => {
    const calls = backend(() => HttpResponse.json({ status: 'stillShown', node: null }))

    await hideOfficialLetteringAt('p1', { x: 10, y: 20 }, t)

    expect(notice()).toMatchObject({ message: 'official.hideStillShown', notice: true })
    await vi.advanceTimersByTimeAsync(600)
    expect(calls.renders).toHaveLength(0)
  })

  it('shows the backend error as an error', async () => {
    const calls = backend(() =>
      HttpResponse.json(
        { status: 400, message: 'this page has no official release underneath' },
        { status: 400 },
      ),
    )

    await hideOfficialLetteringAt('p1', { x: 10, y: 20 }, t)

    const shown = notice()
    expect(shown?.message).toBe('this page has no official release underneath')
    expect(shown?.notice).toBeFalsy()
    await vi.advanceTimersByTimeAsync(600)
    expect(calls.renders).toHaveLength(0)
  })
})
