// Minimal CDP smoke check: connect to the running webview, capture any uncaught
// exceptions for a moment, and evaluate a health snapshot of the rendered app.
const wsUrl = process.argv[2]
if (!wsUrl) throw new Error('usage: bun cdp-smoke.ts <ws-url>')

const ws = new WebSocket(wsUrl)
let id = 0
const pending = new Map<number, (v: any) => void>()
const exceptions: string[] = []
const consoleErrors: string[] = []

function send(method: string, params?: any): Promise<any> {
  const msgId = ++id
  return new Promise((resolve) => {
    pending.set(msgId, resolve)
    ws.send(JSON.stringify({ id: msgId, method, params }))
  })
}

ws.addEventListener('message', (ev) => {
  const msg = JSON.parse(ev.data as string)
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)!(msg.result)
    pending.delete(msg.id)
    return
  }
  if (msg.method === 'Runtime.exceptionThrown') {
    exceptions.push(msg.params?.exceptionDetails?.exception?.description ?? JSON.stringify(msg.params?.exceptionDetails))
  }
  if (msg.method === 'Runtime.consoleAPICalled' && msg.params?.type === 'error') {
    consoleErrors.push((msg.params.args ?? []).map((a: any) => a.value ?? a.description ?? '').join(' '))
  }
})

await new Promise((r) => ws.addEventListener('open', r))
await send('Runtime.enable')
// Give any load-time errors a beat to surface.
await new Promise((r) => setTimeout(r, 1500))

const snapshot = await send('Runtime.evaluate', {
  expression: `(() => {
    const bodyLen = document.body ? document.body.innerText.length : 0;
    const html = document.documentElement.innerHTML;
    const errorBoundary = /Something went wrong|Application error|A client-side exception/i.test(html);
    // Does the main menubar (settings lives here) exist?
    const hasMenubar = !!document.querySelector('[role="menubar"], [data-tauri-drag-region]');
    // Canvas / workspace present => app booted a project view.
    const hasWorkspace = !!document.querySelector('[data-text-block-layer], [data-testid="navigator-panel"]');
    return JSON.stringify({ bodyLen, errorBoundary, hasMenubar, hasWorkspace });
  })()`,
  returnByValue: true,
})

console.log('EVAL:', snapshot?.result?.value ?? JSON.stringify(snapshot))
console.log('EXCEPTIONS:', exceptions.length, JSON.stringify(exceptions.slice(0, 5)))
console.log('CONSOLE_ERRORS:', consoleErrors.length, JSON.stringify(consoleErrors.slice(0, 5)))
ws.close()
