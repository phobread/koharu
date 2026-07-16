# CDP smoke-test scripts (dev tooling, 2026-07-16)

Drive the running KoharuFORK webview over Chrome DevTools Protocol for live
verification of UI changes. Written during the config-write-family work; kept
because the pattern is reusable.

Usage:
1. `Stop-Process -Name KoharuFORK`; relaunch with
   `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`.
2. Get the page ws url: `curl http://127.0.0.1:9222/json` (type == "page").
3. `bun scripts/cdp/<script>.ts <ws-url>`

- `cdp-smoke.ts` — render-health snapshot + uncaught-exception/console-error capture.
- `cdp-open-settings.ts` — opens Settings via the menubar, walks Engines /
  Providers / Runtime tabs, reports per-pane render state + exceptions.
- `cdp-engine-write2.ts` — end-to-end config-write test: changes the Inpainter
  select, verifies /config persisted it, restores the full pipeline to baseline.

Gotchas learned: Radix Select portals its options to `document` (query
`[role="option"]` globally); pair engine labels↔comboboxes BY INDEX (ancestor
text matching grabs the wrong one); dialog tab state persists across scripts on
the same page.
