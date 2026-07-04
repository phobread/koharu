import { isTextNode } from '@/hooks/useCurrentPage'
import { getConfig } from '@/lib/api/default/default'
import type { Page } from '@/lib/api/schemas'
import { invalidateScene } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

/** Extra pixels cleared around a block — segment masks bleed a little past
 * the detector box. */
const CLEAR_MARGIN_PX = 4

type Rect = { x0: number; y0: number; x1: number; y1: number }

/** Axis-aligned bounds of a block box (accounting for rotation about its
 * centre), expanded by the clear margin and clamped to the page. */
const blockClearRect = (
  page: Page,
  t: { x: number; y: number; width: number; height: number; rotationDeg?: number },
): Rect | null => {
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

/**
 * Remove the given blocks from the inpainting: black out their area in the
 * `segment` mask and PUT it back with the affected region, which re-runs the
 * inpainter there in the same backend transaction — restoring the original
 * art (the inpainter only paints where the mask is white). Used for falsely
 * detected blocks and for text worth keeping as-is ("...", sound effects).
 */
export async function uninpaintBlocks(page: Page, nodeIds: string[], segmentPng: Uint8Array) {
  const rects: Rect[] = []
  for (const id of nodeIds) {
    const node = page.nodes[id]
    if (!node || !isTextNode(node) || !node.transform) continue
    const rect = blockClearRect(page, node.transform)
    if (rect) rects.push(rect)
  }
  if (rects.length === 0) return

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
  ctx.fillStyle = '#000'
  for (const r of rects) ctx.fillRect(r.x0, r.y0, r.x1 - r.x0, r.y1 - r.y0)

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
  await invalidateScene()
  useEditorUiStore.getState().setShowInpaintedImage(true)
}
