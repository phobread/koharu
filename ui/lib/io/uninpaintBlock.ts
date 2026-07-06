import { isTextNode } from '@/hooks/useCurrentPage'
import { getConfig } from '@/lib/api/default/default'
import type { Page } from '@/lib/api/schemas'
import { applyOp, invalidateScene, queueAutoRender } from '@/lib/io/scene'
import { ops } from '@/lib/ops'

/** Extra pixels cleared around a block — segment masks bleed a little past
 * the detector box. */
const CLEAR_MARGIN_PX = 4

type Rect = { x0: number; y0: number; x1: number; y1: number }

type BlockTransform = {
  x: number
  y: number
  width: number
  height: number
  rotationDeg?: number
}

/** Axis-aligned bounds of a block box (accounting for rotation about its
 * centre), expanded by the clear margin and clamped to the page. */
const blockClearRect = (page: Page, t: BlockTransform): Rect | null => {
  const rad = ((t.rotationDeg ?? 0) * Math.PI) / 180
  const cos = Math.abs(Math.cos(rad))
  const sin = Math.abs(Math.sin(rad))
  const halfW = (t.width * cos + t.height * sin) / 2
  const halfH = (t.width * sin + t.height * cos) / 2
  const cx = t.x + t.width / 2
  const cy = t.y + t.height / 2
  const x0 = Math.max(0, Math.floor(cx - halfW - CLEAR_MARGIN_PX))
  const y0 = Math.max(0, Math.floor(cy - halfH - CLEAR_MARGIN_PX))
  const x1 = Math.min(page.width, Math.ceil(cx + halfW + CLEAR_MARGIN_PX))
  const y1 = Math.min(page.height, Math.ceil(cy + halfH + CLEAR_MARGIN_PX))
  return x1 > x0 && y1 > y0 ? { x0, y0, x1, y1 } : null
}

/** Fill the block's box following its rotation — filling the axis-aligned
 * bounds of a slanted block would spill into the AABB's corners. `margin`
 * expands the box in its own (local) frame. */
const fillRotatedRect = (ctx: CanvasRenderingContext2D, t: BlockTransform, margin: number) => {
  ctx.save()
  ctx.translate(t.x + t.width / 2, t.y + t.height / 2)
  ctx.rotate(((t.rotationDeg ?? 0) * Math.PI) / 180)
  ctx.fillRect(
    -t.width / 2 - margin,
    -t.height / 2 - margin,
    t.width + 2 * margin,
    t.height + 2 * margin,
  )
  ctx.restore()
}

/**
 * Remove the given blocks from the inpainting: black out their area in the
 * `segment` mask and PUT it back with the affected region, which re-runs the
 * inpainter there in the same backend transaction — restoring the original
 * art (the inpainter only paints where the mask is white). Used for falsely
 * detected blocks and for text worth keeping as-is ("...", sound effects).
 *
 * Also clears each block's translation (keeping the OCR text) so the next
 * render doesn't paint translated text back over the restored art, and
 * queues that re-render so the Translated view reflects the restore.
 */
export async function uninpaintBlocks(page: Page, nodeIds: string[], segmentPng: Uint8Array) {
  const rects: Rect[] = []
  const transforms: BlockTransform[] = []
  const clearIds: string[] = []
  for (const id of nodeIds) {
    const node = page.nodes[id]
    if (!node || !isTextNode(node) || !node.transform) continue
    const rect = blockClearRect(page, node.transform)
    if (!rect) continue
    rects.push(rect)
    transforms.push(node.transform)
    if (node.kind.text.translation) clearIds.push(id)
  }
  if (rects.length === 0) return

  // Every other text block's box is protected from the clear: the margin (and
  // any box overlap) must not wipe a neighbour's mask ink — most visibly after
  // a split, where the halves share a seam and un-inpainting one used to
  // restore a strip of original text inside the half being kept.
  const wanted = new Set(nodeIds)
  const keep: BlockTransform[] = []
  for (const [id, node] of Object.entries(page.nodes)) {
    if (wanted.has(id)) continue
    if (!isTextNode(node) || !node.transform) continue
    keep.push(node.transform)
  }

  // Redraw the current mask and clear the block areas.
  const bitmap = await createImageBitmap(new Blob([segmentPng as unknown as BlobPart]))
  const canvas = document.createElement('canvas')
  canvas.width = page.width
  canvas.height = page.height
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error('canvas 2d context unavailable')
  ctx.fillStyle = '#000'
  ctx.fillRect(0, 0, page.width, page.height)
  ctx.drawImage(bitmap, 0, 0, page.width, page.height)
  bitmap.close()
  // Build the clear shape on its own layer — the cleared blocks' rotated
  // rects (+margin) minus every kept block's rect — then stamp it on black.
  const clearLayer = document.createElement('canvas')
  clearLayer.width = page.width
  clearLayer.height = page.height
  const cctx = clearLayer.getContext('2d')
  if (!cctx) throw new Error('canvas 2d context unavailable')
  cctx.fillStyle = '#000'
  for (const t of transforms) fillRotatedRect(cctx, t, CLEAR_MARGIN_PX)
  cctx.globalCompositeOperation = 'destination-out'
  for (const t of keep) fillRotatedRect(cctx, t, 0)
  ctx.drawImage(clearLayer, 0, 0)

  const png = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('mask encode failed'))), 'image/png')
  })

  // Re-inpaint the union of the cleared areas so the original art returns.
  const x0 = Math.min(...rects.map((r) => r.x0))
  const y0 = Math.min(...rects.map((r) => r.y0))
  const x1 = Math.max(...rects.map((r) => r.x1))
  const y1 = Math.max(...rects.map((r) => r.y1))
  const config = await getConfig()
  const inpainter = config.pipeline?.inpainter || 'lama-manga'
  const params = new URLSearchParams({
    pipeline: inpainter,
    x: String(x0),
    y: String(y0),
    width: String(x1 - x0),
    height: String(y1 - y0),
  })
  const res = await fetch(`/api/v1/pages/${page.id}/masks/segment?${params}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'image/png' },
    body: png,
  })
  if (!res.ok) throw new Error(`mask PUT failed: ${res.status}`)

  // Empty string, not null: a JSON `"translation": null` deserialises to the
  // patch's outer None on the backend and is silently dropped.
  for (const id of clearIds) {
    await applyOp(ops.updateNode(page.id, id, { data: { text: { translation: '' } } as never }))
  }

  await invalidateScene()
  queueAutoRender(page.id)
}
