import { fireEvent, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

// `react-virtual` returns no items in jsdom (no layout): render every row.
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getTotalSize: () => count * 82,
    getVirtualItems: () =>
      Array.from({ length: count }, (_, i) => ({
        index: i,
        start: i * 82,
        end: (i + 1) * 82,
        size: 82,
        key: i,
      })),
  }),
}))

import { Navigator } from '@/components/Navigator'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

beforeEach(() => {
  useSelectionStore.getState().setPage(null)
  useEditorUiStore.getState().clearError()
})

type StarCall = { id: string; favourite: boolean }

/** A project with pages `ids`, of which `starred` are favourites. Returns the
 * star requests the page list sends. */
function project(ids: string[], starred: string[], opts: { failSaving?: boolean } = {}) {
  const pages: Record<string, unknown> = {}
  for (const id of ids) pages[id] = { id, name: id, width: 10, height: 10, nodes: {} }
  let stars = [...starred]
  const calls: StarCall[] = []
  server.use(
    http.get('/api/v1/scene.json', () =>
      HttpResponse.json({ epoch: 0, scene: { pages, project: { name: 'P' } } }),
    ),
    http.get('/api/v1/projects/current/favourites', () =>
      HttpResponse.json({ project: false, pages: stars }),
    ),
    http.put('/api/v1/pages/:id/favourite', async ({ params, request }) => {
      const id = String(params.id)
      const { favourite } = (await request.json()) as { favourite: boolean }
      calls.push({ id, favourite })
      if (opts.failSaving) return HttpResponse.json({ message: 'disk full' }, { status: 500 })
      stars = stars.filter((s) => s !== id)
      if (favourite) stars.push(id)
      // The backend answers in page order.
      return HttpResponse.json({ project: false, pages: ids.filter((p) => stars.includes(p)) })
    }),
  )
  return calls
}

const star = (index: number) => screen.getByTestId(`navigator-page-star-${index}`)
const pressed = (el: HTMLElement) => el.getAttribute('aria-pressed') === 'true'

describe('Navigator favourites', () => {
  it('shows the saved stars, and a click on a star saves it without opening the page', async () => {
    const calls = project(['a', 'b', 'c'], ['a'])
    renderWithQuery(<Navigator />)
    await screen.findByTestId('navigator-page-0')
    await waitFor(() => expect(pressed(star(0))).toBe(true))
    expect(pressed(star(1))).toBe(false)

    await userEvent.click(star(1))
    await waitFor(() => expect(calls).toEqual([{ id: 'b', favourite: true }]))
    await waitFor(() => expect(pressed(star(1))).toBe(true))
    expect(pressed(star(0))).toBe(true)
    // The row under the star was not clicked: nothing opened or selected.
    expect(useSelectionStore.getState().pageId).toBeNull()
    expect(useSelectionStore.getState().selectedPageIds.size).toBe(0)

    // A second click takes the star off again.
    await userEvent.click(star(1))
    await waitFor(() => expect(calls).toHaveLength(2))
    expect(calls[1]).toEqual({ id: 'b', favourite: false })
    await waitFor(() => expect(pressed(star(1))).toBe(false))
  })

  it("keys and presses on the star don't reach the row either", async () => {
    const calls = project(['a', 'b'], [])
    renderWithQuery(<Navigator />)
    await screen.findByTestId('navigator-page-0')

    // On the row itself Enter opens the page and Delete deletes: not from the star.
    fireEvent.keyDown(star(1), { key: 'Enter' })
    fireEvent.keyDown(star(1), { key: 'Delete' })
    fireEvent.pointerDown(star(1))
    fireEvent.mouseDown(star(1))
    expect(useSelectionStore.getState().pageId).toBeNull()
    expect(calls).toEqual([])

    // The row still opens its page when clicked anywhere else.
    await userEvent.click(screen.getByTestId('navigator-page-1'))
    expect(useSelectionStore.getState().pageId).toBe('b')
  })

  it('the filter leaves only starred pages, under their own page numbers', async () => {
    project(['a', 'b', 'c', 'd'], ['b', 'd'])
    renderWithQuery(<Navigator />)
    await screen.findByTestId('navigator-page-0')
    await waitFor(() => expect(pressed(star(1))).toBe(true))
    const filter = screen.getByTestId('navigator-favourites-only')
    expect(pressed(filter)).toBe(false)

    await userEvent.click(filter)
    expect(pressed(filter)).toBe(true)
    expect(screen.queryByTestId('navigator-page-0')).not.toBeInTheDocument()
    expect(screen.queryByTestId('navigator-page-2')).not.toBeInTheDocument()
    expect(screen.getByTestId('navigator-page-1')).toHaveAttribute('aria-label', 'Page 2: b')
    expect(screen.getByTestId('navigator-page-3')).toHaveAttribute('aria-label', 'Page 4: d')
    // The count is still the project's.
    expect(screen.getByTestId('navigator-panel')).toHaveAttribute('data-total-pages', '4')
    expect(screen.queryByTestId('navigator-favourites-empty')).not.toBeInTheDocument()

    // A shift-click range takes the pages on show, not the hidden one between.
    await userEvent.click(screen.getByTestId('navigator-page-1'))
    fireEvent.click(screen.getByTestId('navigator-page-3'), { shiftKey: true })
    expect([...useSelectionStore.getState().selectedPageIds].sort()).toEqual(['b', 'd'])

    // Taking a star off while filtered drops the page from the list.
    await userEvent.click(star(1))
    await waitFor(() => expect(screen.queryByTestId('navigator-page-1')).not.toBeInTheDocument())
    expect(screen.getByTestId('navigator-page-3')).toBeInTheDocument()

    await userEvent.click(filter)
    expect(screen.getByTestId('navigator-page-0')).toBeInTheDocument()
    expect(screen.getByTestId('navigator-page-2')).toBeInTheDocument()
  })

  it('says so when the filter is on and nothing is starred', async () => {
    project(['a', 'b'], [])
    renderWithQuery(<Navigator />)
    await screen.findByTestId('navigator-page-0')
    expect(screen.queryByTestId('navigator-favourites-empty')).not.toBeInTheDocument()

    await userEvent.click(screen.getByTestId('navigator-favourites-only'))
    expect(screen.getByTestId('navigator-favourites-empty')).toBeInTheDocument()
    expect(screen.queryByTestId('navigator-page-0')).not.toBeInTheDocument()
    // The way back is still there.
    await userEvent.click(screen.getByTestId('navigator-favourites-only'))
    expect(screen.getByTestId('navigator-page-0')).toBeInTheDocument()
  })

  it('ignores starred ids the project no longer has', async () => {
    project(['a', 'b'], ['gone', 'b'])
    renderWithQuery(<Navigator />)
    await screen.findByTestId('navigator-page-0')
    await waitFor(() => expect(pressed(star(1))).toBe(true))
    await userEvent.click(screen.getByTestId('navigator-favourites-only'))
    expect(screen.getAllByTestId(/^navigator-page-\d+$/)).toHaveLength(1)
  })

  it('puts the star back and reports it when saving fails', async () => {
    const calls = project(['a', 'b'], [], { failSaving: true })
    renderWithQuery(<Navigator />)
    await screen.findByTestId('navigator-page-0')

    await userEvent.click(star(0))
    await waitFor(() => expect(calls).toEqual([{ id: 'a', favourite: true }]))
    await waitFor(() => expect(useEditorUiStore.getState().error).toBeDefined())
    await waitFor(() => expect(pressed(star(0))).toBe(false))
  })
})
