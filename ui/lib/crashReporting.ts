import * as Sentry from '@sentry/nextjs'

// Web UI crash reporting, gated on Settings → Privacy → "Send crash reports"
// (`config.telemetry.crash_reports`). The SDK only starts once the setting is
// known to be on, so nothing is sent before the config has loaded. Builds
// without a DSN (anything but official releases) never report.

let started = false

export function applyCrashReportingSetting(
  enabled: boolean,
  dsn: string | undefined = process.env.NEXT_PUBLIC_SENTRY_DSN,
): void {
  if (!dsn) return
  if (enabled && !started) {
    Sentry.init({ dsn, sendDefaultPii: false, sampleRate: 0.1 })
    started = true
    return
  }
  const client = Sentry.getClient()
  if (client) client.getOptions().enabled = enabled
}
