import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { WelcomeScreen } from '@/components/WelcomeScreen'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

vi.mock('@/lib/io/openFiles', () => ({
  openImageFiles: vi.fn().mockResolvedValue([]),
  openImageFolder: vi.fn().mockResolvedValue([]),
  openKhrFile: vi.fn().mockResolvedValue(null),
}))

beforeEach(() => {
  useEditorUiStore.setState({ favouriteProjectsOnly: false })
  useEditorUiStore.getState().clearError()
})

type StarCall = { id: string; favourite: boolean }

/** A project list of `ids`, of which `starred` are favourites. Returns the
 * star requests and the open requests the screen sends. */
function projects(
  ids: string[],
  starred: string[] | undefined,
  opts: { failSaving?: boolean } = {},
) {
  let stars = starred
  const calls: StarCall[] = []
  const opened: string[] = []
  server.use(
    http.get('/api/v1/projects', () =>
      HttpResponse.json({
        projects: ids.map((id, i) => ({
          id,
          name: id.toUpperCase(),
          path: `/tmp/${id}`,
          updatedAtMs: 1000 - i,
        })),
        ...(stars ? { favourites: stars } : {}),
      }),
    ),
    http.put('/api/v1/projects/current', async ({ request }) => {
      const { id } = (await request.json()) as { id: string }
      opened.push(id)
      return HttpResponse.json({ id, name: id, path: `/tmp/${id}`, updatedAtMs: 0 })
    }),
    http.put('/api/v1/projects/:id/favourite', async ({ params, request }) => {
      const id = String(params.id)
      const { favourite } = (await request.json()) as { favourite: boolean }
      calls.push({ id, favourite })
      if (opts.failSaving) return HttpResponse.json({ message: 'disk full' }, { status: 500 })
      stars = (stars ?? []).filter((s) => s !== id)
      if (favourite) stars.push(id)
      return new HttpResponse(null, { status: 204 })
    }),
  )
  return { calls, opened }
}

const star = (id: string) => screen.getByTestId(`welcome-star-project-${id}`)
const pressed = (el: HTMLElement) => el.getAttribute('aria-pressed') === 'true'
const cards = () => screen.queryAllByTestId(/^welcome-project-/).map((el) => el.dataset.testid)

describe('WelcomeScreen favourites', () => {
  it('shows the saved stars, and a click on a star saves it without opening the project', async () => {
    const { calls, opened } = projects(['one', 'two'], ['one'])
    renderWithQuery(<WelcomeScreen />)
    await screen.findByTestId('welcome-project-one')
    expect(pressed(star('one'))).toBe(true)
    expect(pressed(star('two'))).toBe(false)
    // The star sits beside the cover's button, not inside it.
    expect(screen.getByTestId('welcome-project-two')).not.toContainElement(star('two'))

    await userEvent.click(star('two'))
    await waitFor(() => expect(calls).toEqual([{ id: 'two', favourite: true }]))
    await waitFor(() => expect(pressed(star('two'))).toBe(true))
    expect(pressed(star('one'))).toBe(true)

    await userEvent.click(star('one'))
    await waitFor(() => expect(calls).toHaveLength(2))
    expect(calls[1]).toEqual({ id: 'one', favourite: false })
    await waitFor(() => expect(pressed(star('one'))).toBe(false))

    expect(opened).toEqual([])
    // Starring doesn't reorder the list either.
    expect(cards()).toEqual(['welcome-project-one', 'welcome-project-two'])
  })

  it('the filter leaves only starred projects, and is kept while the app runs', async () => {
    projects(['one', 'two', 'three'], ['two'])
    const first = renderWithQuery(<WelcomeScreen />)
    await screen.findByTestId('welcome-project-one')
    const filter = screen.getByTestId('welcome-favourites-only')
    expect(pressed(filter)).toBe(false)

    await userEvent.click(filter)
    expect(pressed(filter)).toBe(true)
    expect(cards()).toEqual(['welcome-project-two'])
    expect(screen.queryByTestId('welcome-favourites-empty')).not.toBeInTheDocument()

    // Coming back from a project: still filtered.
    first.unmount()
    renderWithQuery(<WelcomeScreen />)
    await screen.findByTestId('welcome-project-two')
    expect(cards()).toEqual(['welcome-project-two'])

    // Unstarring the last one shows the empty line, with the way back.
    await userEvent.click(star('two'))
    await waitFor(() => expect(screen.getByTestId('welcome-favourites-empty')).toBeInTheDocument())
    expect(cards()).toEqual([])
    await userEvent.click(screen.getByTestId('welcome-favourites-only'))
    expect(cards()).toHaveLength(3)
  })

  it('a list without stars (older answer) shows every project unstarred', async () => {
    projects(['one', 'two'], undefined)
    renderWithQuery(<WelcomeScreen />)
    await screen.findByTestId('welcome-project-one')
    expect(pressed(star('one'))).toBe(false)
    expect(pressed(star('two'))).toBe(false)
  })

  it('puts the star back and reports it when saving fails', async () => {
    const { calls } = projects(['one'], [], { failSaving: true })
    renderWithQuery(<WelcomeScreen />)
    await screen.findByTestId('welcome-project-one')

    await userEvent.click(star('one'))
    await waitFor(() => expect(calls).toEqual([{ id: 'one', favourite: true }]))
    await waitFor(() => expect(useEditorUiStore.getState().error).toBeDefined())
    await waitFor(() => expect(pressed(star('one'))).toBe(false))
  })
})
