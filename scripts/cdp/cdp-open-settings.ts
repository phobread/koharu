// Open the Settings dialog via the menubar and walk the tabs that use the
// refactored config-write code (engines, providers, runtime), capturing any
// exception the mount/render/effects throw.
const wsUrl = process.argv[2]
const ws = new WebSocket(wsUrl)
let id = 0
const pending = new Map<number, (v: any) => void>()
const exceptions: string[] = []

function send(method: string, params?: any): Promise<any> {
  const msgId = ++id
  return new Promise((resolve) => {
    pending.set(msgId, resolve)
    ws.send(JSON.stringify({ id: msgId, method, params }))
  })
}
ws.addEventListener('message', (ev) => {
  const msg = JSON.parse(ev.data as string)
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)!(msg.result); pending.delete(msg.id); return }
  if (msg.method === 'Runtime.exceptionThrown') {
    exceptions.push(msg.params?.exceptionDetails?.exception?.description ?? JSON.stringify(msg.params?.exceptionDetails))
  }
})
async function ev16(expression: string) {
  const r = await send('Runtime.evaluate', { expression, returnByValue: true })
  if (r?.exceptionDetails) return { error: r.exceptionDetails.exception?.description }
  return r?.result?.value
}
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms))

await new Promise((r) => ws.addEventListener('open', r))
await send('Runtime.enable')

// 1) open the first menubar menu
const step1 = await ev16(`(() => {
  const t = document.querySelector('[role="menubar"] button, [role="menubar"] [role="menuitem"]');
  if (!t) return 'no-menubar-trigger';
  t.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true}));
  t.click();
  return t.textContent || 'clicked';
})()`)
await wait(500)

// 2) click the item whose text matches Settings
const step2 = await ev16(`(() => {
  const items = [...document.querySelectorAll('[role="menuitem"],[role="menuitemradio"],[role="menuitemcheckbox"]')];
  const s = items.find(i => /settings|设置|設定|설정|configuraci|paramètres|einstellungen/i.test(i.textContent||''));
  if (!s) return 'no-settings-item:'+items.map(i=>i.textContent).join('|').slice(0,120);
  s.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true}));
  s.click();
  return 'clicked:'+s.textContent;
})()`)
await wait(700)

// 3) dialog present? walk tabs by their label text and check the pane renders.
const step3 = await ev16(`(() => {
  const dlg = document.querySelector('[role="dialog"]');
  if (!dlg) return JSON.stringify({dialog:false});
  const clickTab = (re) => {
    const btns = [...dlg.querySelectorAll('button')];
    const b = btns.find(x => re.test(x.textContent||''));
    if (b) { b.click(); return true; }
    return false;
  };
  const results = {};
  results.dialog = true;
  results.engines = clickTab(/engine/i);
  return JSON.stringify(results);
})()`)
await wait(500)
const paneEngines = await ev16(`(() => {
  const dlg = document.querySelector('[role="dialog"]');
  return dlg ? dlg.innerText.length : 0;
})()`)
// switch to providers/API keys
await ev16(`(() => {
  const dlg = document.querySelector('[role="dialog"]'); if(!dlg) return;
  const b=[...dlg.querySelectorAll('button')].find(x=>/api key|provider|keys/i.test(x.textContent||'')); b&&b.click();
})()`)
await wait(500)
const paneProviders = await ev16(`(() => {
  const dlg=document.querySelector('[role="dialog"]'); if(!dlg) return JSON.stringify({dialog:false});
  const inputs=dlg.querySelectorAll('input');
  return JSON.stringify({dialog:true, textLen:dlg.innerText.length, inputCount:inputs.length});
})()`)
// switch to runtime/storage (has the plain data-path input the queue persists)
await ev16(`(() => {
  const dlg=document.querySelector('[role="dialog"]'); if(!dlg) return;
  const b=[...dlg.querySelectorAll('button')].find(x=>/runtime|storage|data/i.test(x.textContent||'')); b&&b.click();
})()`)
await wait(500)
const paneRuntime = await ev16(`(() => {
  const dlg=document.querySelector('[role="dialog"]'); if(!dlg) return JSON.stringify({dialog:false});
  return JSON.stringify({dialog:true, textLen:dlg.innerText.length, inputCount:dlg.querySelectorAll('input').length});
})()`)

console.log('STEP1_openMenu:', step1)
console.log('STEP2_clickSettings:', step2)
console.log('STEP3_dialogTabs:', step3)
console.log('PANE_engines_textLen:', paneEngines)
console.log('PANE_providers:', paneProviders)
console.log('PANE_runtime:', paneRuntime)
console.log('EXCEPTIONS:', exceptions.length, JSON.stringify(exceptions.slice(0, 6)))
ws.close()
