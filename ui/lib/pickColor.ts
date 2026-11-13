'use client'

/**
 * An eyedropper that works inside the app window.
 *
 * The browser's own `EyeDropper` exists in the desktop app's WebView but
 * cancels itself the moment it is opened, so its button did nothing. This
 * reads the colour from what is drawn under the pointer instead: the page
 * pictures (`<img>`) and the drawing layers (`<canvas>`), blended in the order
 * they are painted.
 */

type Rgba = [number, number, number, number]

let scratch: HTMLCanvasElement | null = null

function imagePixel(image: HTMLImageElement, u: number, v: number): Rgba | null {
  if (!image.complete || image.naturalWidth === 0 || image.naturalHeight === 0) return null
  scratch ??= document.createElement('canvas')
  scratch.width = 1
  scratch.height = 1
  const ctx = scratch.getContext('2d', { willReadFrequently: true })
  if (!ctx) return null
  const sx = Math.min(image.naturalWidth - 1, Math.floor(u * image.naturalWidth))
  const sy = Math.min(image.naturalHeight - 1, Math.floor(v * image.naturalHeight))
  try {
    ctx.clearRect(0, 0, 1, 1)
    ctx.drawImage(image, sx, sy, 1, 1, 0, 0, 1, 1)
    const d = ctx.getImageData(0, 0, 1, 1).data
    return [d[0], d[1], d[2], d[3] / 255]
  } catch {
    // A picture from another origin can be shown but not read.
    return null
  }
}

function canvasPixel(canvas: HTMLCanvasElement, u: number, v: number): Rgba | null {
  if (canvas.width === 0 || canvas.height === 0) return null
  const ctx = canvas.getContext('2d')
  if (!ctx) return null
  const sx = Math.min(canvas.width - 1, Math.floor(u * canvas.width))
  const sy = Math.min(canvas.height - 1, Math.floor(v * canvas.height))
  try {
    const d = ctx.getImageData(sx, sy, 1, 1).data
    return [d[0], d[1], d[2], d[3] / 255]
  } catch {
    return null
  }
}

/** How see-through an element is drawn: its own opacity times its ancestors'. */
function drawnOpacity(element: Element): number {
  let opacity = 1
  for (let node: Element | null = element; node; node = node.parentElement) {
    const style = getComputedStyle(node)
    if (style.display === 'none' || style.visibility === 'hidden') return 0
    opacity *= Number.parseFloat(style.opacity || '1')
    if (opacity <= 0) return 0
  }
  return opacity
}

/** The z-index an element is painted at: its own, or its nearest ancestor's. */
function paintLevel(element: Element): number {
  for (let node: Element | null = element; node; node = node.parentElement) {
    const z = Number.parseInt(getComputedStyle(node).zIndex, 10)
    if (Number.isFinite(z)) return z
  }
  return 0
}

const toHex = (value: number) =>
  Math.max(0, Math.min(255, Math.round(value)))
    .toString(16)
    .padStart(2, '0')

/**
 * The colour drawn at a point of the window, as `#RRGGBB`, or `null` where no
 * picture or canvas is drawn.
 */
export function colorAtPoint(x: number, y: number): string | null {
  const layers: { level: number; order: number; pixel: Rgba }[] = []
  let order = 0
  for (const element of document.querySelectorAll('img, canvas')) {
    order += 1
    const rect = element.getBoundingClientRect()
    if (rect.width <= 0 || rect.height <= 0) continue
    const u = (x - rect.left) / rect.width
    const v = (y - rect.top) / rect.height
    if (u < 0 || u >= 1 || v < 0 || v >= 1) continue
    const opacity = drawnOpacity(element)
    if (opacity <= 0) continue
    const pixel =
      element instanceof HTMLCanvasElement
        ? canvasPixel(element, u, v)
        : imagePixel(element as HTMLImageElement, u, v)
    if (!pixel || pixel[3] <= 0) continue
    layers.push({
      level: paintLevel(element),
      order,
      pixel: [pixel[0], pixel[1], pixel[2], pixel[3] * opacity],
    })
  }
  if (layers.length === 0) return null

  // Bottom to top, each layer over what is below it.
  layers.sort((a, b) => a.level - b.level || a.order - b.order)
  let [r, g, b, a] = [0, 0, 0, 0]
  for (const { pixel } of layers) {
    const [pr, pg, pb, pa] = pixel
    const out = pa + a * (1 - pa)
    if (out <= 0) continue
    r = (pr * pa + r * a * (1 - pa)) / out
    g = (pg * pa + g * a * (1 - pa)) / out
    b = (pb * pa + b * a * (1 - pa)) / out
    a = out
  }
  if (a <= 0) return null
  return `#${toHex(r)}${toHex(g)}${toHex(b)}`.toUpperCase()
}

/** Marks the parts of the window a pick must not read through (the picker itself). */
export const PICK_IGNORE_ATTRIBUTE = 'data-color-pick-ignore'

let cancelActivePick: (() => void) | null = null
let stoppedByPickerClick = false

/**
 * True while the click that ended a pick, a click on the colour picker, is
 * still being handled. It may land on the eyedropper button, where it means
 * "stop", not "start again".
 */
export function pickJustCancelled(): boolean {
  return stoppedByPickerClick
}

function rememberPickerClickUntilItIsOver() {
  stoppedByPickerClick = true
  const options = { capture: true }
  const forget = () => {
    window.removeEventListener('click', afterClick, options)
    window.removeEventListener('pointerdown', forget, options)
    stoppedByPickerClick = false
  }
  // The button's own click handler runs after this capture listener.
  const afterClick = () => window.setTimeout(forget, 0)
  window.addEventListener('click', afterClick, options)
  // No click followed (the press was dragged away): the next press is new.
  window.addEventListener('pointerdown', forget, options)
}

/**
 * Waits for the next click and resolves with the colour under it. Resolves
 * with `null` when the pick is cancelled: Escape, a click where nothing is
 * drawn, a click on the colour picker itself, or another pick starting.
 *
 * The click is kept from the page, so it doesn't also draw, select or close
 * the picker.
 */
export function pickColorFromPage(): Promise<string | null> {
  cancelActivePick?.()
  return new Promise((resolve) => {
    const style = document.createElement('style')
    style.textContent = '* { cursor: crosshair !important; }'
    document.head.appendChild(style)

    const swallow = (event: Event) => {
      event.preventDefault()
      event.stopImmediatePropagation()
    }
    // The click that picks is made of several events; all of them stay here.
    const followUps = ['pointerup', 'mousedown', 'mouseup', 'click'] as const
    const options = { capture: true }
    let finished = false

    const finish = (color: string | null) => {
      if (finished) return
      finished = true
      cancelActivePick = null
      style.remove()
      window.removeEventListener('pointerdown', onPointerDown, options)
      window.removeEventListener('keydown', onKeyDown, options)
      resolve(color)
    }

    const onPointerDown = (event: PointerEvent) => {
      const target = event.target instanceof Element ? event.target : null
      if (target?.closest(`[${PICK_IGNORE_ATTRIBUTE}]`)) {
        // A click on the picker itself ends the pick and works as usual.
        rememberPickerClickUntilItIsOver()
        finish(null)
        return
      }
      swallow(event)
      // The rest of this click stays here too; its `click` event is the last.
      const release = () => {
        window.clearTimeout(timer)
        for (const type of followUps) window.removeEventListener(type, swallowRest, options)
      }
      const swallowRest = (rest: Event) => {
        swallow(rest)
        if (rest.type === 'click') release()
      }
      for (const type of followUps) window.addEventListener(type, swallowRest, options)
      // No `click` comes when the press is dragged off its target.
      const timer = window.setTimeout(release, 400)
      finish(colorAtPoint(event.clientX, event.clientY))
    }

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      swallow(event)
      finish(null)
    }

    cancelActivePick = () => finish(null)
    window.addEventListener('pointerdown', onPointerDown, options)
    window.addEventListener('keydown', onKeyDown, options)
  })
}
