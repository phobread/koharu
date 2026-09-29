import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it } from 'vitest'

import { FreeUpSpace, formatBytes } from '@/components/FreeUpSpace'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

const usage = (over: Record<string, unknown> = {}) => ({
  projectsBytes: 5_960_000_000,
  unusedImages: 1860,
  unusedImageBytes: 5_250_000_000,
  thumbnails: 41,
  thumbnailBytes: 2_700_000,
  failed: 0,
  skipped: [],
  ...over,
})

let requests: Array<{ apply: boolean }>

function answer(measure: object, applied: object = usage({ projectsBytes: 700_000_000 })) {
  requests = []
  server.use(
    http.post('/api/v1/storage/cleanup', async ({ request }) => {
      const body = (await request.json()) as { apply: boolean }
      requests.push(body)
      return HttpResponse.json(body.apply ? applied : measure)
    }),
  )
}

beforeEach(() => answer(usage()))

describe('FreeUpSpace', () => {
  it('measures first and deletes only after the confirmation', async () => {
    renderWithQuery(<FreeUpSpace />)

    const button = await screen.findByTestId('free-up-space')
    await waitFor(() => expect(button).toBeEnabled())
    expect(requests).toEqual([{ apply: false }])
    expect(screen.getByTestId('projects-size')).toHaveTextContent('storage.projectsUse')

    // Cancel: nothing is deleted.
    await userEvent.click(button)
    await userEvent.click(await screen.findByRole('button', { name: 'common.cancel' }))
    expect(requests).toEqual([{ apply: false }])

    await userEvent.click(button)
    await userEvent.click(await screen.findByTestId('free-up-space-confirm'))
    expect(await screen.findByRole('status')).toHaveTextContent('storage.freed')
    // Deleted once, then measured again.
    await waitFor(() =>
      expect(requests).toEqual([{ apply: false }, { apply: true }, { apply: false }]),
    )
  })

  it('offers nothing when there is less than a megabyte to free', async () => {
    answer(usage({ unusedImages: 0, unusedImageBytes: 0, thumbnailBytes: 20_000 }))
    renderWithQuery(<FreeUpSpace />)
    await waitFor(() => expect(requests).toHaveLength(1))
    expect(await screen.findByTestId('free-up-space')).toBeDisabled()
  })

  it('the home-page line shows the button only when there is something to free', async () => {
    answer(usage({ unusedImages: 0, unusedImageBytes: 0, thumbnailBytes: 0 }))
    renderWithQuery(<FreeUpSpace compact />)
    expect(await screen.findByTestId('projects-size')).toBeInTheDocument()
    expect(screen.queryByTestId('free-up-space')).not.toBeInTheDocument()
  })

  it('names projects it leaves alone, and reports a failed clean-up', async () => {
    answer(usage({ skipped: [{ id: 'badend', reason: 'open' }] }))
    server.use(
      http.post('/api/v1/storage/cleanup', async ({ request }) => {
        const body = (await request.json()) as { apply: boolean }
        requests.push(body)
        return body.apply
          ? new HttpResponse(null, { status: 500 })
          : HttpResponse.json(usage({ skipped: [{ id: 'badend', reason: 'open' }] }))
      }),
    )
    renderWithQuery(<FreeUpSpace />)
    expect(await screen.findByText('storage.skippedOpen')).toBeInTheDocument()

    const button = await screen.findByTestId('free-up-space')
    await waitFor(() => expect(button).toBeEnabled())
    await userEvent.click(button)
    await userEvent.click(await screen.findByTestId('free-up-space-confirm'))
    expect(await screen.findByRole('status')).toHaveTextContent('storage.failed')
  })

  it('formats sizes the way the button shows them', () => {
    expect(formatBytes(5_250_000_000)).toBe('5.25 GB')
    expect(formatBytes(312_400_000)).toBe('312 MB')
    expect(formatBytes(48_200)).toBe('48 KB')
  })
})
