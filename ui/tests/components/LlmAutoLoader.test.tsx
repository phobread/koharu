import { act, waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it } from 'vitest'

import { LlmAutoLoader } from '@/components/LlmAutoLoader'
import type { LlmTarget } from '@/lib/api/schemas'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

const target: LlmTarget = {
  kind: 'provider',
  providerId: 'claude',
  modelId: 'claude-opus-4-5-20251101',
}

describe('LlmAutoLoader', () => {
  beforeEach(async () => {
    server.use(http.get('/api/v1/config', () => HttpResponse.json({ editor: { client: '' } })))
    await act(async () => {
      await useEditorUiStore.persist.rehydrate()
    })
    useEditorUiStore.setState({
      selectedTarget: target,
      selectedLanguage: 'en-US',
    })
  })

  it('auto-loads the persisted LLM target on startup', async () => {
    const loadRequests: unknown[] = []
    server.use(
      http.get('/api/v1/llm/catalog', () =>
        HttpResponse.json({
          localModels: [],
          providers: [
            {
              id: 'claude',
              name: 'Claude',
              hasApiKey: true,
              requiresApiKey: true,
              requiresBaseUrl: false,
              status: 'ready',
              baseUrl: null,
              error: null,
              models: [{ name: 'Claude Opus 4.5', languages: ['en-US'], target }],
            },
          ],
        }),
      ),
      http.get('/api/v1/llm/current', () =>
        HttpResponse.json({ status: 'empty', target: null, error: null }),
      ),
      http.put('/api/v1/llm/current', async ({ request }) => {
        loadRequests.push(await request.json())
        return new HttpResponse(null, { status: 204 })
      }),
    )

    renderWithQuery(<LlmAutoLoader />)

    await waitFor(() => expect(loadRequests).toHaveLength(1))
    expect(loadRequests[0]).toEqual({ target })
  })

  it('never auto-selects or loads a local model when nothing is saved', async () => {
    useEditorUiStore.setState({ selectedTarget: undefined, selectedLanguage: undefined })
    const loadRequests: unknown[] = []
    server.use(
      http.get('/api/v1/llm/catalog', () =>
        HttpResponse.json({
          localModels: [
            {
              name: 'vntl-llama3-8b-v2',
              languages: ['ja-JP'],
              target: { kind: 'local', modelId: 'vntl-llama3-8b-v2' },
            },
          ],
          providers: [],
        }),
      ),
      http.get('/api/v1/llm/current', () =>
        HttpResponse.json({ status: 'empty', target: null, error: null }),
      ),
      http.put('/api/v1/llm/current', async ({ request }) => {
        loadRequests.push(await request.json())
        return new HttpResponse(null, { status: 204 })
      }),
    )

    renderWithQuery(<LlmAutoLoader />)

    // Auto-loading a local model here would silently start a multi-GB
    // weights download; the selection must stay empty instead.
    await new Promise((r) => setTimeout(r, 150))
    expect(loadRequests).toHaveLength(0)
    expect(useEditorUiStore.getState().selectedTarget).toBeUndefined()
  })

  it('auto-loads a persisted target before catalog discovery lists it', async () => {
    const loadRequests: unknown[] = []
    server.use(
      http.get('/api/v1/llm/catalog', () =>
        HttpResponse.json({
          localModels: [],
          providers: [],
        }),
      ),
      http.get('/api/v1/llm/current', () =>
        HttpResponse.json({ status: 'empty', target: null, error: null }),
      ),
      http.put('/api/v1/llm/current', async ({ request }) => {
        loadRequests.push(await request.json())
        return new HttpResponse(null, { status: 204 })
      }),
    )

    renderWithQuery(<LlmAutoLoader />)

    await waitFor(() => expect(loadRequests).toHaveLength(1))
    expect(loadRequests[0]).toEqual({ target })
  })
})
