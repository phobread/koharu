import { renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  closeProject: vi.fn(),
  reopenLastProject: vi.fn(),
}))
vi.mock('@/lib/io/scene', () => mocks)

import { useMouseNavigation } from '@/hooks/useMouseNavigation'

const BACK = 3
const FORWARD = 4

/** Press and release a mouse button; returns whether the release was default-prevented. */
function press(button: number): boolean {
  window.dispatchEvent(new MouseEvent('mousedown', { button, cancelable: true }))
  const up = new MouseEvent('mouseup', { button, cancelable: true })
  window.dispatchEvent(up)
  return up.defaultPrevented
}

beforeEach(() => {
  mocks.closeProject.mockReset().mockResolvedValue(undefined)
  mocks.reopenLastProject.mockReset().mockResolvedValue(true)
  document.body.innerHTML = ''
})

describe('useMouseNavigation', () => {
  it('back leaves the open project; forward does nothing inside one', () => {
    renderHook(() => useMouseNavigation(true))

    expect(press(BACK)).toBe(true)
    expect(mocks.closeProject).toHaveBeenCalledTimes(1)
    press(FORWARD)
    expect(mocks.reopenLastProject).not.toHaveBeenCalled()
  })

  it('forward reopens the last project from the project list; back does nothing there', () => {
    renderHook(() => useMouseNavigation(false))

    expect(press(FORWARD)).toBe(true)
    expect(mocks.reopenLastProject).toHaveBeenCalledTimes(1)
    press(BACK)
    expect(mocks.closeProject).not.toHaveBeenCalled()
  })

  it('leaves an open dialog or menu alone, but still blocks webview history', () => {
    renderHook(() => useMouseNavigation(true))
    for (const html of [
      '<div data-slot="dialog-content"></div>',
      '<div data-slot="alert-dialog-content"></div>',
      '<div role="menu"></div>',
    ]) {
      document.body.innerHTML = html
      expect(press(BACK)).toBe(true)
    }
    expect(mocks.closeProject).not.toHaveBeenCalled()

    // The floating box editor is not a modal: back still works over it.
    document.body.innerHTML = '<div role="dialog"></div>'
    press(BACK)
    expect(mocks.closeProject).toHaveBeenCalledTimes(1)
  })

  it('ignores the other buttons and stops listening when unmounted', () => {
    const { unmount } = renderHook(() => useMouseNavigation(true))
    expect(press(0)).toBe(false)
    expect(press(1)).toBe(false)
    expect(mocks.closeProject).not.toHaveBeenCalled()

    unmount()
    expect(press(BACK)).toBe(false)
    expect(mocks.closeProject).not.toHaveBeenCalled()
  })
})
