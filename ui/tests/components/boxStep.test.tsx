import { fireEvent, screen, waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { boxStepField, TextBlockLayer } from '@/components/canvas/TextBlockLayer'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

vi.mock('@/lib/io/scene', async () => {
  const actual = await vi.importActual<any>('@/lib/io/scene')
  return { ...actual, applyOp: vi.fn(), queueAutoRender: vi.fn() }
})

function keydown(key: string, target: Element, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init })
  Object.defineProperty(event, 'target', { value: target })
  return event
}

describe('boxStepField', () => {
  it('steps from the page or the canvas', () => {
    expect(boxStepField(keydown('Tab', document.body))).toBe('')
    const viewport = document.createElement('div')
    viewport.setAttribute('data-testid', 'workspace-viewport')
    const inner = document.createElement('div')
    viewport.appendChild(inner)
    expect(boxStepField(keydown('Tab', inner))).toBe('')
    expect(boxStepField(keydown('a', document.body))).toBeNull()
  })

  it('follows from the box editor text fields only', () => {
    const editor = document.createElement('div')
    editor.setAttribute('data-testid', 'block-quick-editor')
    const ocr = document.createElement('textarea')
    ocr.setAttribute('data-testid', 'quick-editor-ocr')
    const size = document.createElement('input')
    editor.append(ocr, size)
    expect(boxStepField(keydown('Tab', ocr))).toBe('quick-editor-ocr')
    expect(boxStepField(keydown('Tab', size))).toBeNull()
    expect(boxStepField(keydown('Tab', document.createElement('textarea')))).toBeNull()
  })

  it('leaves IME, chords, menus and other panels alone', () => {
    expect(boxStepField(keydown('Tab', document.body, { isComposing: true }))).toBeNull()
    expect(boxStepField(keydown('Tab', document.body, { ctrlKey: true }))).toBeNull()
    const menu = document.createElement('div')
    menu.setAttribute('role', 'menu')
    const item = document.createElement('div')
    menu.appendChild(item)
    expect(boxStepField(keydown('Tab', item))).toBeNull()
    expect(boxStepField(keydown('Tab', document.createElement('button')))).toBeNull()
  })
})

describe('Tab through boxes', () => {
  beforeEach(() => {
    useSelectionStore.getState().setPage('p1')
    const node = (id: string, x: number) => ({
      id,
      visible: true,
      transform: { x, y: 0, width: 10, height: 10, rotationDeg: 0 },
      kind: { text: { text: id } },
    })
    server.use(
      http.get('/api/v1/scene.json', () =>
        HttpResponse.json({
          epoch: 1,
          scene: {
            pages: {
              p1: {
                id: 'p1',
                name: 'P1',
                width: 100,
                height: 100,
                nodes: { a: node('a', 0), b: node('b', 20), c: node('c', 40) },
              },
            },
            project: { name: 'Proj' },
          },
        }),
      ),
    )
  })

  it('selects the next and previous box, wrapping around', async () => {
    renderWithQuery(<TextBlockLayer scale={1} />)
    const ids = () => [...useSelectionStore.getState().nodeIds]

    await waitFor(() => {
      fireEvent.keyDown(window, { key: 'Tab' })
      expect(ids()).toEqual(['a'])
    })
    fireEvent.keyDown(window, { key: 'Tab' })
    expect(ids()).toEqual(['b'])
    fireEvent.keyDown(window, { key: 'Tab', shiftKey: true })
    expect(ids()).toEqual(['a'])
    fireEvent.keyDown(window, { key: 'Tab', shiftKey: true })
    expect(ids()).toEqual(['c'])
    fireEvent.keyDown(window, { key: 'Tab' })
    expect(ids()).toEqual(['a'])
    expect(useSelectionStore.getState().quickEdit).toBe(true)
  })

  it('moves the cursor to the same field of the next box editor', async () => {
    renderWithQuery(<TextBlockLayer scale={1} />)
    await waitFor(() => {
      fireEvent.keyDown(window, { key: 'Tab' })
      expect(useSelectionStore.getState().nodeIds.has('a')).toBe(true)
    })
    const ocr = await screen.findByTestId('quick-editor-ocr')
    expect(ocr).toHaveValue('a')
    ocr.focus()
    fireEvent.keyDown(ocr, { key: 'Tab' })
    await waitFor(() => {
      const next = screen.getByTestId('quick-editor-ocr')
      expect(next).toHaveValue('b')
      expect(document.activeElement).toBe(next)
    })
  })
})
