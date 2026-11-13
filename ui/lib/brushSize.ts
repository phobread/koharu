/** Brush size limits and stepping, shared by the slider and the [ / ] keys. */
export const BRUSH_SIZE_MIN = 1
export const BRUSH_SIZE_MAX = 128
/** Up to this size the keys move by 1; above it they move by BRUSH_SIZE_COARSE_STEP. */
export const BRUSH_SIZE_FINE_LIMIT = 8
export const BRUSH_SIZE_COARSE_STEP = 4
/** Below this on-screen ring diameter (px) the system crosshair is shown too. */
export const BRUSH_RING_MIN_VISIBLE_PX = 8

/**
 * Mask strokes thinner than this are drawn as whole pixels. A smooth 1-2px
 * line is partly transparent everywhere, and the server keeps a mask pixel
 * only above half white (and counts it erased only at pure black).
 */
export const BRUSH_HARD_EDGE_BELOW = 4

/**
 * Draws the segment as `size` x `size` squares on whole pixels, so every pixel
 * it touches gets the full colour.
 */
export function stampLine(
  ctx: Pick<CanvasRenderingContext2D, 'fillRect'>,
  from: { x: number; y: number },
  to: { x: number; y: number },
  size: number,
): void {
  const side = Math.max(1, Math.round(size))
  const dx = to.x - from.x
  const dy = to.y - from.y
  // Half-pixel steps leave no gaps on diagonals.
  const steps = Math.max(1, Math.ceil(Math.max(Math.abs(dx), Math.abs(dy)) * 2))
  for (let i = 0; i <= steps; i++) {
    const t = i / steps
    ctx.fillRect(
      Math.round(from.x + dx * t - side / 2),
      Math.round(from.y + dy * t - side / 2),
      side,
      side,
    )
  }
}

export function clampBrushSize(size: number): number {
  return Math.min(BRUSH_SIZE_MAX, Math.max(BRUSH_SIZE_MIN, size))
}

/**
 * Next size for the `]` (direction 1) or `[` (direction -1) key: steps of 1 up
 * to 8, then multiples of 4. An off-grid size snaps to the next multiple of 4.
 */
export function stepBrushSize(size: number, direction: 1 | -1): number {
  const current = clampBrushSize(Math.round(size))
  const step = BRUSH_SIZE_COARSE_STEP
  let next: number
  if (direction > 0) {
    next = current < BRUSH_SIZE_FINE_LIMIT ? current + 1 : (Math.floor(current / step) + 1) * step
  } else {
    next = current <= BRUSH_SIZE_FINE_LIMIT ? current - 1 : (Math.ceil(current / step) - 1) * step
  }
  return clampBrushSize(next)
}
