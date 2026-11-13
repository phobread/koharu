import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  PICK_IGNORE_ATTRIBUTE,
  colorAtPoint,
  pickColorFromPage,
  pickJustCancelled,
} from '@/lib/pickColor'

type Rgba = [number, number, number, number]

/** A canvas filled with one colour, laid out at `rect` on the page. */
function layer(
  rgba: Rgba,
  rect = { left: 0, top: 0, width: 100, height: 100 },
  style: Partial<CSSStyleDeclaration> = {},
) {
  const canvas = document.createElement('canvas')
  canvas.width = 10
  canvas.height = 10
  Object.assign(canvas.style, style)
  canvas.getBoundingClientRect = () => ({ ...rect, x: rect.left, y: rect.top }) as DOMRect
  canvas.getContext = (() => ({
    getImageData: () => ({ data: Uint8ClampedArray.from(rgba) }),
  })) as unknown as HTMLCanvasElement['getContext']
  document.body.appendChild(canvas)
  return canvas
}

function pointerDown(x: number, y: number, target: EventTarget = document.body) {
  const event = new MouseEvent('pointerdown', {
    clientX: x,
    clientY: y,
    bubbles: true,
    cancelable: true,
  })
  target.dispatchEvent(event)
  return event
}

afterEach(() => {
  document.body.innerHTML = ''
  vi.restoreAllMocks()
})

describe('colorAtPoint', () => {
  it('reads the colour drawn under the point', () => {
    layer([18, 52, 86, 255])
    expect(colorAtPoint(50, 50)).toBe('#123456')
  })

  it('has nothing to give where no picture is drawn', () => {
    layer([18, 52, 86, 255], { left: 0, top: 0, width: 100, height: 100 })
    expect(colorAtPoint(150, 50)).toBeNull()
  })

  it('looks through hidden and empty layers', () => {
    layer([255, 0, 0, 255])
    layer([0, 255, 0, 255], undefined, { opacity: '0' })
    layer([0, 0, 255, 0])
    expect(colorAtPoint(10, 10)).toBe('#FF0000')
  })

  it('paints by z-index before document order', () => {
    // The mask layer comes first in the page but sits above the picture.
    layer([255, 255, 255, 255], undefined, { zIndex: '20' })
    layer([0, 0, 0, 255])
    expect(colorAtPoint(10, 10)).toBe('#FFFFFF')
  })

  it('blends a see-through layer with what is below it', () => {
    layer([0, 0, 0, 255])
    layer([255, 255, 255, 255], undefined, { zIndex: '20', opacity: '0.8' })
    expect(colorAtPoint(10, 10)).toBe('#CCCCCC')
  })
})

describe('pickColorFromPage', () => {
  it('gives the colour under the next click and keeps that click from the page', async () => {
    layer([18, 52, 86, 255])
    const pageSawIt = vi.fn()
    document.body.addEventListener('pointerdown', pageSawIt)
    document.body.addEventListener('click', pageSawIt)

    const picked = pickColorFromPage()
    const down = pointerDown(50, 50)
    document.body.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))

    expect(await picked).toBe('#123456')
    expect(down.defaultPrevented).toBe(true)
    expect(pageSawIt).not.toHaveBeenCalled()
  })

  it('stops on Escape', async () => {
    layer([18, 52, 86, 255])
    const picked = pickColorFromPage()
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    expect(await picked).toBeNull()

    // Picking is over: a later click belongs to the page again.
    const pageSawIt = vi.fn()
    document.body.addEventListener('pointerdown', pageSawIt)
    pointerDown(50, 50)
    expect(pageSawIt).toHaveBeenCalledTimes(1)
  })

  it('stops, without eating the click, when the colour picker itself is clicked', async () => {
    layer([18, 52, 86, 255])
    const panel = document.createElement('div')
    panel.setAttribute(PICK_IGNORE_ATTRIBUTE, '')
    document.body.appendChild(panel)
    const panelSawIt = vi.fn()
    panel.addEventListener('pointerdown', panelSawIt)

    const picked = pickColorFromPage()
    pointerDown(50, 50, panel)

    expect(await picked).toBeNull()
    expect(panelSawIt).toHaveBeenCalledTimes(1)

    // The same click may reach the eyedropper button: it must not start again.
    expect(pickJustCancelled()).toBe(true)
    panel.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    expect(pickJustCancelled()).toBe(true)
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(pickJustCancelled()).toBe(false)
  })

  it('ends an earlier pick when a new one starts', async () => {
    const first = pickColorFromPage()
    const second = pickColorFromPage()
    expect(await first).toBeNull()
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    expect(await second).toBeNull()
  })
})
