'use client'

import { useEffect } from 'react'

import { useGetConfig } from '@/lib/api/default/default'
import { applyCrashReportingSetting } from '@/lib/crashReporting'

/** Applies the saved crash-report setting once the config is available. */
export function CrashReporting() {
  // `/config` answers 503 until the app has started; keep polling until then.
  const { data: config } = useGetConfig({ query: { retry: true, retryDelay: 1500 } })
  const enabled = config?.telemetry?.crash_reports

  useEffect(() => {
    if (enabled !== undefined) applyCrashReportingSetting(enabled)
  }, [enabled])

  return null
}
