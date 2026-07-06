/**
 * Geometry for editing boxes rotated about their centre (slanted text).
 *
 * Convention matches the screen/CSS one everywhere in the app: y grows down
 * and positive angles rotate clockwise. All functions reduce exactly to the
 * axis-aligned behaviour at 0°.
 */

export type Box = { x: number; y: number; width: number; height: number }
export type ResizeEdge = { top: boolean; bottom: boolean; left: boolean; right: boolean }

/** Rotate a vector by `deg` (clockwise-positive, y-down). */
export function rotateVec(x: number, y: number, deg: number): [number, number] {
  const r = (deg * Math.PI) / 180
  const c = Math.cos(r)
  const s = Math.sin(r)
  return [x * c - y * s, x * s + y * c]
}

/** Map a screen-space pointer delta into the box's local (unrotated) frame. */
export function toLocalDelta(mx: number, my: number, deg: number): [number, number] {
  return rotateVec(mx, my, -deg)
}

/**
 * Signs of the anchor point — the corner or edge midpoint *opposite* the
 * dragged one, in half-extent units relative to the box centre.
 */
function anchorSigns(edge: ResizeEdge): [number, number] {
  const ax = edge.left ? 0.5 : edge.right ? -0.5 : 0
  const ay = edge.top ? 0.5 : edge.bottom ? -0.5 : 0
  return [ax, ay]
}

/** Re-centre `box` resized to `w`×`h` so the anchor stays fixed on screen. */
function anchored(box: Box, edge: ResizeEdge, deg: number, w: number, h: number): Box {
  const [ax, ay] = anchorSigns(edge)
  const [oldAx, oldAy] = rotateVec(ax * box.width, ay * box.height, deg)
  const [newAx, newAy] = rotateVec(ax * w, ay * h, deg)
  const cx = box.x + box.width / 2 + (oldAx - newAx)
  const cy = box.y + box.height / 2 + (oldAy - newAy)
  return { x: cx - w / 2, y: cy - h / 2, width: w, height: h }
}

/**
 * Edge-resize a rotated box by a screen-space pointer delta. Width/height
 * change along the box's local axes; the opposite corner/edge stays fixed in
 * screen space.
 */
export function resizeRotatedBox(
  box: Box,
  edge: ResizeEdge,
  mx: number,
  my: number,
  deg: number,
  minSize: number,
): Box {
  const [lx, ly] = toLocalDelta(mx, my, deg)
  let w = box.width
  let h = box.height
  if (edge.right) w += lx
  if (edge.left) w -= lx
  if (edge.bottom) h += ly
  if (edge.top) h -= ly
  w = Math.max(minSize, w)
  h = Math.max(minSize, h)
  return anchored(box, edge, deg, w, h)
}

/**
 * Uniform corner-scale factor from a screen-space drag (the dominant local
 * axis drives it, aspect stays locked — Canva-style).
 */
export function cornerScaleFactor(
  box: Box,
  edge: ResizeEdge,
  mx: number,
  my: number,
  deg: number,
  minFactor: number,
): number {
  const [lx, ly] = toLocalDelta(mx, my, deg)
  const wR = (edge.right ? box.width + lx : box.width - lx) / box.width
  const hR = (edge.bottom ? box.height + ly : box.height - ly) / box.height
  const factor = Math.abs(wR - 1) >= Math.abs(hR - 1) ? wR : hR
  return Math.max(factor, minFactor)
}

/** Scale a rotated box uniformly about the anchor opposite the dragged corner. */
export function scaleRotatedBox(box: Box, edge: ResizeEdge, factor: number, deg: number): Box {
  return anchored(box, edge, deg, box.width * factor, box.height * factor)
}

/** Bring an angle into [-180, 180). */
export function normalizeRotationDeg(deg: number): number {
  return ((((deg + 180) % 360) + 360) % 360) - 180
}

/** How close (degrees) a rotation drag sticks to the cardinal angles. */
export const ROTATE_SNAP_DEG = 3
/** Angle step when Shift is held during a rotation drag. */
export const ROTATE_STEP_DEG = 15

/**
 * Post-process a rotation-drag angle: Shift quantises to 15° steps, and the
 * cardinal angles (0 / ±90 / 180) act magnetic within ±3° so straightening a
 * box by hand is effortless. Fine off-cardinal angles remain available via
 * the quick editor's number input.
 */
export function snapRotationDeg(deg: number, stepped: boolean): number {
  let next = normalizeRotationDeg(deg)
  if (stepped) next = normalizeRotationDeg(Math.round(next / ROTATE_STEP_DEG) * ROTATE_STEP_DEG)
  for (const cardinal of [0, 90, -90, 180, -180]) {
    if (Math.abs(next - cardinal) < ROTATE_SNAP_DEG) return normalizeRotationDeg(cardinal)
  }
  return next
}
