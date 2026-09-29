import { describe, expect, it } from 'vitest'

import { isDeleteSelectionKey } from '@/components/canvas/TextBlockLayer'
import { textNodesTouching } from '@/hooks/useBlockDrafting'

function keydown(key: string, target: Element, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init })
  Object.defineProperty(event, 'target', { value: target })
  return event
}

describe('isDeleteSelectionKey', () => {
  it('accepts Delete and Backspace aimed at the page', () => {
    expect(isDeleteSelectionKey(keydown('Delete', document.body))).toBe(true)
    expect(isDeleteSelectionKey(keydown('Backspace', document.body))).toBe(true)
    expect(isDeleteSelectionKey(keydown('a', document.body))).toBe(false)
  })

  it('ignores typing, IME composition and shortcut chords', () => {
    const textarea = document.createElement('textarea')
    expect(isDeleteSelectionKey(keydown('Backspace', textarea))).toBe(false)
    expect(isDeleteSelectionKey(keydown('Backspace', document.body, { isComposing: true }))).toBe(
      false,
    )
    expect(isDeleteSelectionKey(keydown('Backspace', document.body, { ctrlKey: true }))).toBe(false)
  })

  it('leaves the page list and open menus alone', () => {
    const nav = document.createElement('div')
    nav.setAttribute('data-testid', 'navigator-panel')
    const card = document.createElement('div')
    nav.appendChild(card)
    const menu = document.createElement('div')
    menu.setAttribute('role', 'menu')
    const item = document.createElement('div')
    menu.appendChild(item)
    expect(isDeleteSelectionKey(keydown('Delete', card))).toBe(false)
    expect(isDeleteSelectionKey(keydown('Delete', item))).toBe(false)
  })
})

describe('textNodesTouching', () => {
  const page = {
    id: 'p',
    name: 'p',
    width: 200,
    height: 200,
    nodes: {
      left: {
        id: 'left',
        visible: true,
        transform: { x: 10, y: 10, width: 20, height: 20, rotationDeg: 0 },
        kind: { text: {} },
      },
      right: {
        id: 'right',
        visible: true,
        transform: { x: 150, y: 10, width: 20, height: 20, rotationDeg: 0 },
        kind: { text: {} },
      },
      // A tall thin box turned 90 degrees reaches sideways instead.
      turned: {
        id: 'turned',
        visible: true,
        transform: { x: 95, y: 100, width: 10, height: 80, rotationDeg: 90 },
        kind: { text: {} },
      },
      art: { id: 'art', visible: true, kind: { image: {} } },
    },
  } as never

  it('selects every box the rectangle touches, not only boxes fully inside', () => {
    expect(textNodesTouching(page, { x: 25, y: 25, width: 10, height: 10 })).toEqual(['left'])
    expect(textNodesTouching(page, { x: 0, y: 0, width: 200, height: 40 }).sort()).toEqual([
      'left',
      'right',
    ])
  })

  it('uses the rotated bounds of turned boxes', () => {
    // Unrotated it would span x 95-105; turned it spans x 60-140 at y 135-145.
    expect(textNodesTouching(page, { x: 60, y: 136, width: 5, height: 5 })).toEqual(['turned'])
    expect(textNodesTouching(page, { x: 96, y: 100, width: 5, height: 5 })).toEqual([])
  })
})
