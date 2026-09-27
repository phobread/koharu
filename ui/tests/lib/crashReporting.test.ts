import { beforeEach, describe, expect, it, vi } from 'vitest'

const sentry = vi.hoisted(() => ({ init: vi.fn(), getClient: vi.fn() }))
vi.mock('@sentry/nextjs', () => sentry)

const DSN = 'https://key@example.ingest.sentry.io/1'

// Fresh module per test: `started` is module state.
async function load() {
  vi.resetModules()
  return (await import('@/lib/crashReporting')).applyCrashReportingSetting
}

describe('applyCrashReportingSetting', () => {
  beforeEach(() => {
    sentry.init.mockReset()
    sentry.getClient.mockReset()
  })

  it('never starts Sentry without a DSN', async () => {
    const apply = await load()
    apply(true, undefined)
    expect(sentry.init).not.toHaveBeenCalled()
  })

  it('does not start Sentry while crash reports are off', async () => {
    const apply = await load()
    apply(false, DSN)
    expect(sentry.init).not.toHaveBeenCalled()
  })

  it('starts once, without personal data, when crash reports are on', async () => {
    const apply = await load()
    apply(true, DSN)
    apply(true, DSN)
    expect(sentry.init).toHaveBeenCalledTimes(1)
    expect(sentry.init).toHaveBeenCalledWith(
      expect.objectContaining({ dsn: DSN, sendDefaultPii: false }),
    )
  })

  it('turning crash reports off disables the running client', async () => {
    const apply = await load()
    apply(true, DSN)
    const options = { enabled: true }
    sentry.getClient.mockReturnValue({ getOptions: () => options })

    apply(false, DSN)
    expect(options.enabled).toBe(false)

    apply(true, DSN)
    expect(options.enabled).toBe(true)
    expect(sentry.init).toHaveBeenCalledTimes(1)
  })
})
