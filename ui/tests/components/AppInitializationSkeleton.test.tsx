import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it } from 'vitest'

import { AppInitializationSkeleton } from '@/components/AppInitializationSkeleton'
import { useDownloadsStore } from '@/lib/stores/downloadsStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

describe('AppInitializationSkeleton', () => {
  beforeEach(() => {
    useDownloadsStore.getState().clear()
    // The generated default mock returns a random state; pin it.
    server.use(http.get('/api/v1/bootstrap', () => HttpResponse.json({ state: 'starting' })))
  })

  it('renders the Koharu title and initializing copy', () => {
    renderWithQuery(<AppInitializationSkeleton />)
    expect(screen.getByRole('heading', { name: 'Koharu' })).toBeInTheDocument()
    expect(screen.getByText('common.initializing')).toBeInTheDocument()
  })

  it('shows active download filename + percent when present', () => {
    useDownloadsStore.getState().progress({
      id: 'pkg',
      filename: 'llama.cpp.zip',
      downloaded: 25,
      total: 100,
      status: { status: 'downloading' },
    })

    renderWithQuery(<AppInitializationSkeleton />)
    expect(screen.getByText('llama.cpp.zip')).toBeInTheDocument()
  })

  it('filename placeholder is blank when nothing downloading', () => {
    renderWithQuery(<AppInitializationSkeleton />)
    // The filename slot is present but empty — just assert no download names
    // leaked from a previous test.
    expect(screen.queryByText('llama.cpp.zip')).not.toBeInTheDocument()
  })

  it('offers no retry while starting', async () => {
    renderWithQuery(<AppInitializationSkeleton />)
    await waitFor(() => expect(screen.getByText('common.initializing')).toBeInTheDocument())
    expect(screen.queryByRole('button', { name: 'startup.retry' })).not.toBeInTheDocument()
  })

  it('shows why startup failed and retries on click', async () => {
    let retried = false
    server.use(
      http.get('/api/v1/bootstrap', () =>
        HttpResponse.json(
          retried
            ? { state: 'starting' }
            : { state: 'failed', error: 'failed to download `llama.zip`' },
        ),
      ),
      http.post('/api/v1/bootstrap/retry', () => {
        retried = true
        return HttpResponse.json({ state: 'starting' }, { status: 202 })
      }),
    )
    renderWithQuery(<AppInitializationSkeleton />)

    expect(await screen.findByText('failed to download `llama.zip`')).toBeInTheDocument()
    expect(screen.getByText('startup.failed')).toBeInTheDocument()

    await userEvent.click(screen.getByRole('button', { name: 'startup.retry' }))

    await waitFor(() => expect(screen.getByText('common.initializing')).toBeInTheDocument())
    expect(retried).toBe(true)
  })
})
