import { waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { TextBlockLayer } from '@/components/canvas/TextBlockLayer'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

vi.mock('@/lib/io/scene', async () => {
  const actual = await vi.importActual<any>('@/lib/io/scene')
  return { ...actual, applyOp: vi.fn(), queueAutoRender: vi.fn() }
})

// Text laid out in its bubble reaches well past the detector's box.
const sprite = { x: 80, y: 60, width: 260, height: 170, rotationDeg: 0 }

const node = (lockLayoutBox: boolean) => ({
  id: 'a',
  visible: true,
  transform: { x: 100, y: 100, width: 200, height: 100, rotationDeg: 0 },
  kind: {
    text: { text: 'a', translation: 'Hello there', spriteTransform: sprite, lockLayoutBox },
  },
})

const serveScene = (lockLayoutBox: boolean) =>
  server.use(
    http.get('/api/v1/scene.json', () =>
      HttpResponse.json({
        epoch: 1,
        scene: {
          pages: {
            p1: {
              id: 'p1',
              name: 'P1',
              width: 400,
              height: 400,
              nodes: { a: node(lockLayoutBox) },
            },
          },
          project: { name: 'Proj' },
        },
      }),
    ),
  )

const boxElement = (container: HTMLElement) =>
  [...container.querySelectorAll<HTMLElement>('div')].find((el) =>
    el.style.transform.startsWith('translate('),
  )

describe('box of text laid out in its bubble', () => {
  beforeEach(() => useSelectionStore.getState().setPage('p1'))

  it('is drawn around the text', async () => {
    serveScene(false)
    const { container } = renderWithQuery(<TextBlockLayer scale={1} />)
    await waitFor(() => expect(boxElement(container)).toBeTruthy())
    const el = boxElement(container)!
    expect(el.style.transform).toContain('translate(80px, 60px)')
    expect(el.style.width).toBe('260px')
    expect(el.style.height).toBe('170px')
  })

  it('keeps a locked box where the user put it', async () => {
    serveScene(true)
    const { container } = renderWithQuery(<TextBlockLayer scale={1} />)
    await waitFor(() => expect(boxElement(container)).toBeTruthy())
    const el = boxElement(container)!
    expect(el.style.transform).toContain('translate(100px, 100px)')
    expect(el.style.width).toBe('200px')
  })
})
