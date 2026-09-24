import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it, vi } from 'vitest'

import { SettingsDialog } from '@/components/SettingsDialog'
import type { AppConfig, ConfigPatch } from '@/lib/api/schemas'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

const applyCrashReportingSetting = vi.hoisted(() => vi.fn())
vi.mock('@/lib/crashReporting', () => ({ applyCrashReportingSetting }))

const config: AppConfig = {
  data: { path: '/tmp/koharu' },
  http: { connect_timeout: 20, read_timeout: 300, max_retries: 3 },
  pipeline: {},
  providers: [],
  telemetry: { crash_reports: true },
  mcp: { enabled: true },
}

describe('SettingsDialog privacy tab', () => {
  it('saves the crash-report and MCP switches', async () => {
    const patches: ConfigPatch[] = []
    server.use(
      http.get('/api/v1/config', () => HttpResponse.json(config)),
      http.patch('/api/v1/config', async ({ request }) => {
        const patch = (await request.json()) as ConfigPatch
        patches.push(patch)
        return HttpResponse.json({
          ...config,
          telemetry: { crash_reports: patch.telemetry?.crashReports ?? true },
          mcp: { enabled: patch.mcp?.enabled ?? true },
        })
      }),
    )
    renderWithQuery(<SettingsDialog open onOpenChange={() => {}} defaultTab='privacy' />)

    const crashReports = await screen.findByRole('switch', {
      name: 'settings.crashReportsToggle',
    })
    const mcp = screen.getByRole('switch', { name: 'settings.mcpToggle' })
    expect(crashReports).toBeChecked()
    expect(mcp).toBeChecked()

    await userEvent.click(crashReports)
    await waitFor(() => expect(patches).toHaveLength(1))
    expect(patches[0]).toMatchObject({ telemetry: { crashReports: false }, mcp: { enabled: true } })
    await waitFor(() => expect(applyCrashReportingSetting).toHaveBeenCalledWith(false))

    await userEvent.click(mcp)
    await waitFor(() => expect(patches).toHaveLength(2))
    expect(patches[1]).toMatchObject({
      telemetry: { crashReports: false },
      mcp: { enabled: false },
    })
    expect(mcp).not.toBeChecked()
  })
})
