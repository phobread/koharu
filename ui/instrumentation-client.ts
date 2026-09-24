import * as Sentry from '@sentry/nextjs'

// Sentry is initialised by `applyCrashReportingSetting` (lib/crashReporting.ts)
// once the user's crash-report setting has loaded; until then this is a no-op.
export const onRouterTransitionStart = Sentry.captureRouterTransitionStart
