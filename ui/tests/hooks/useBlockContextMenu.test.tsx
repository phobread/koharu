import { act, renderHook } from '@testing-library/react'
import type React from 'react'
import { describe, expect, it, vi } from 'vitest'

import { useBlockContextMenu } from '@/hooks/useBlockContextMenu'
import type { Page } from '@/lib/api/schemas'

const page = {
  id: 'p1',
  name: '026.jpg',
  width: 400,
  height: 300,
  nodes: {
    box: {
      id: 'box',
      visible: true,
      transform: { x: 100, y: 100, width: 80, height: 40, rotationDeg: 0 },
      kind: { text: { text: 'hi' } },
    },
  },
} as unknown as Page

/** A right-click whose page point is its client point. */
function rightClick(x: number, y: number) {
  return {
    clientX: x,
    clientY: y,
    preventDefault: vi.fn(),
  } as unknown as React.MouseEvent<HTMLElement> & { preventDefault: ReturnType<typeof vi.fn> }
}

function setup(menuOnPicture?: boolean) {
  const onSelect = vi.fn()
  const hook = renderHook(() =>
    useBlockContextMenu({
      page,
      pointerToDocument: (event) => ({ x: event.clientX, y: event.clientY }),
      onSelect,
      onRemove: vi.fn(),
      onSplit: vi.fn(),
      menuOnPicture,
    }),
  )
  return { ...hook, onSelect }
}

describe('useBlockContextMenu', () => {
  it('opens no menu on the picture when the page has no action for it', () => {
    const { result, onSelect } = setup()
    const event = rightClick(300, 200)
    act(() => result.current.handleContextMenu(event))
    expect(event.preventDefault).toHaveBeenCalled()
    expect(result.current.contextMenuNodeId).toBeNull()
    expect(result.current.contextMenuPoint).toBeNull()
    expect(onSelect).toHaveBeenCalledWith(null)
  })

  it('opens the menu on the picture and remembers the point when asked to', () => {
    const { result, onSelect } = setup(true)
    const event = rightClick(300, 200)
    act(() => result.current.handleContextMenu(event))
    expect(event.preventDefault).not.toHaveBeenCalled()
    expect(result.current.contextMenuNodeId).toBeNull()
    expect(result.current.contextMenuPoint).toEqual({ x: 300, y: 200 })
    expect(onSelect).toHaveBeenCalledWith(null)

    act(() => result.current.clearContextMenu())
    expect(result.current.contextMenuPoint).toBeNull()
  })

  it('a right-click on a box is about the box, not the picture under it', () => {
    const { result, onSelect } = setup(true)
    act(() => result.current.handleContextMenu(rightClick(300, 200)))
    const event = rightClick(120, 110)
    act(() => result.current.handleContextMenu(event))
    expect(event.preventDefault).not.toHaveBeenCalled()
    expect(result.current.contextMenuNodeId).toBe('box')
    expect(result.current.contextMenuPoint).toBeNull()
    expect(onSelect).toHaveBeenLastCalledWith('box')
  })
})
