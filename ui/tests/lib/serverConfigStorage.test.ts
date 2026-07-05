import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { server } from '../msw/server'

/**
 * The module keeps a singleton cache, so each test imports a fresh copy.
 */
async function freshStorage() {
  vi.resetModules()
  const mod = await import('@/lib/stores/serverConfigStorage')
  return mod.serverConfigStorage
}

describe('serverConfigStorage', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('retries a transiently failing config read instead of hydrating defaults', async () => {
    let calls = 0
    server.use(
      http.get('/api/v1/config', () => {
        calls += 1
        if (calls < 3) return HttpResponse.json({ message: 'starting up' }, { status: 500 })
        return HttpResponse.json({
          editor: { client: JSON.stringify({ 'koharu-editor': '{"state":{"a":1}}' }) },
        })
      }),
    )

    const storage = await freshStorage()
    const value = await storage.getItem('koharu-editor')
    expect(calls).toBe(3)
    expect(value).toBe('{"state":{"a":1}}')
  })

  it('does not fall back to the legacy localStorage blob on read failure', async () => {
    // The legacy blob only holds the old preferences store; treating it as
    // the full truth would hydrate every other store as empty and let a
    // later flush overwrite their real server-side settings.
    localStorage.setItem('koharu-config', '{"state":{"legacy":true}}')
    server.use(
      http.get('/api/v1/config', () => HttpResponse.json({ message: 'down' }, { status: 500 })),
    )

    const storage = await freshStorage()
    await expect(storage.getItem('koharu-editor')).rejects.toThrow()
  }, 20000)
})
