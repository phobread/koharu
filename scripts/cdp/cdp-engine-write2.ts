// Slice-4 write test, precise targeting: pair engine <label>s with their
// comboboxes by index, drive the Inpainter one, confirm /config persists via the
// queue, then restore the FULL pipeline to baseline (also undoing any earlier
// accidental change).
const wsUrl = process.argv[2]
const ws = new WebSocket(wsUrl)
let id = 0
const pending = new Map<number, (v: any) => void>()
const exceptions: string[] = []
function send(method: string, params?: any): Promise<any> {
  const msgId = ++id
  return new Promise((resolve) => { pending.set(msgId, resolve); ws.send(JSON.stringify({ id: msgId, method, params })) })
}
ws.addEventListener('message', (ev) => {
  const msg = JSON.parse(ev.data as string)
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)!(msg.result); pending.delete(msg.id); return }
  if (msg.method === 'Runtime.exceptionThrown') exceptions.push(msg.params?.exceptionDetails?.exception?.description ?? 'exc')
})
async function evAsync(expression: string) {
  const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })
  if (r?.exceptionDetails) return { __error: r.exceptionDetails.exception?.description }
  return r?.result?.value
}
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms))
await new Promise((r) => ws.addEventListener('open', r))
await send('Runtime.enable')

const baseline = JSON.parse(await evAsync(`fetch('/api/v1/config').then(r=>r.json()).then(c=>JSON.stringify(c.pipeline))`))
console.log('BASELINE_pipeline:', JSON.stringify(baseline))

await evAsync(`(() => { const dlg=document.querySelector('[role="dialog"]'); const nav=dlg&&[...dlg.querySelectorAll('button')].find(b=>/^\\s*engines?\\s*$/i.test(b.textContent||'')); nav&&nav.click(); })()`)
await wait(500)

const open = await evAsync(`(() => {
  const dlg=document.querySelector('[role="dialog"]'); if(!dlg) return 'no-dialog';
  const labels=[...dlg.querySelectorAll('label')];
  const combos=[...dlg.querySelectorAll('[role="combobox"]')];
  const i = labels.findIndex(l=>/inpaint/i.test(l.textContent||''));
  if (i<0) return 'no-inpaint-label';
  if (!combos[i]) return 'no-combo-at:'+i+'/'+combos.length;
  const c=combos[i]; c.scrollIntoView();
  c.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,button:0})); c.click();
  return 'opened@'+i+':'+(c.textContent||'');
})()`)
console.log('OPEN_inpainter:', open)
await wait(500)
const opts = await evAsync(`JSON.stringify([...document.querySelectorAll('[role="option"]')].map(o=>o.textContent))`)
console.log('OPTIONS:', opts)
const pick = await evAsync(`(() => {
  const opts=[...document.querySelectorAll('[role="option"]')]; if(!opts.length) return 'no-options';
  const curName=${JSON.stringify(baseline.inpainter)};
  const p=opts.find(o=>!new RegExp(curName.replace(/[-\\/]/g,'.'),'i').test(o.textContent||''))||opts[0];
  p.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,button:0})); p.click();
  return 'picked:'+(p.textContent||'');
})()`)
console.log('PICK:', pick)
await wait(1200)
const after = JSON.parse(await evAsync(`fetch('/api/v1/config').then(r=>r.json()).then(c=>JSON.stringify(c.pipeline))`))
console.log('AFTER_inpainter:', after.inpainter, '| baseline:', baseline.inpainter, '| CHANGED:', after.inpainter !== baseline.inpainter)

// restore the full pipeline to baseline (undo the test change + any earlier stray change)
const body = JSON.stringify({ pipeline: {
  detector: baseline.detector, fontDetector: baseline.font_detector, segmenter: baseline.segmenter,
  bubbleSegmenter: baseline.bubble_segmenter, ocr: baseline.ocr, translator: baseline.translator,
  inpainter: baseline.inpainter, renderer: baseline.renderer } })
const rs = await evAsync(`fetch('/api/v1/config',{method:'PATCH',headers:{'Content-Type':'application/json'},body:${JSON.stringify(body)}}).then(r=>r.status)`)
const restored = JSON.parse(await evAsync(`fetch('/api/v1/config').then(r=>r.json()).then(c=>JSON.stringify(c.pipeline))`))
console.log('RESTORE_status:', rs, '| restored==baseline:', JSON.stringify(restored)===JSON.stringify(baseline))
console.log('EXCEPTIONS:', exceptions.length, JSON.stringify(exceptions.slice(0,5)))
ws.close()
