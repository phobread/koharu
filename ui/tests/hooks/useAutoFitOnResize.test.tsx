import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { fitCanvasToViewport } from '@/components/canvas/canvasViewport'
import { useAutoFitOnResize } from '@/hooks/useAutoFitOnResize'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

vi.mock('@/components/canvas/canvasViewport', () => ({
  fitCanvasToViewport: vi.fn(),
}))

class ResizeObserverMock implements ResizeObserver {
  static instances: ResizeObserverMock[] = []

  observe = vi.fn()
  unobserve = vi.fn()
  disconnect = vi.fn()

  constructor(readonly callback: ResizeObserverCallback) {
    ResizeObserverMock.instances.push(this)
  }
}

let nextFrameId = 1
let queuedFrames = new Map<number, FrameRequestCallback>()

function flushFrames() {
  const frames = [...queuedFrames.values()]
  queuedFrames.clear()
  frames.forEach((frame) => frame(0))
}

describe('useAutoFitOnResize', () => {
  beforeEach(() => {
    ResizeObserverMock.instances = []
    nextFrameId = 1
    queuedFrames = new Map()
    vi.mocked(fitCanvasToViewport).mockClear()
    useEditorUiStore.getState().setAutoFitEnabled(true)

    globalThis.ResizeObserver = ResizeObserverMock
    globalThis.requestAnimationFrame = vi.fn((callback: FrameRequestCallback) => {
      const id = nextFrameId++
      queuedFrames.set(id, callback)
      return id
    })
    globalThis.cancelAnimationFrame = vi.fn((id: number) => {
      queuedFrames.delete(id)
    })
  })

  it('observes the attached element and fits after a resize when auto-fit is enabled', () => {
    const { result } = renderHook(() => useAutoFitOnResize())
    const element = document.createElement('div')

    act(() => result.current(element))
    const observer = ResizeObserverMock.instances[0]

    expect(observer.observe).toHaveBeenCalledWith(element)

    observer.callback([], observer)
    flushFrames()

    expect(fitCanvasToViewport).toHaveBeenCalledOnce()
  })

  it('does not fit after a resize when auto-fit is disabled', () => {
    const { result } = renderHook(() => useAutoFitOnResize())
    const element = document.createElement('div')
    act(() => result.current(element))
    useEditorUiStore.getState().setAutoFitEnabled(false)

    const observer = ResizeObserverMock.instances[0]
    observer.callback([], observer)
    flushFrames()

    expect(fitCanvasToViewport).not.toHaveBeenCalled()
  })

  it('coalesces multiple resize notifications into one animation frame', () => {
    const { result } = renderHook(() => useAutoFitOnResize())
    const element = document.createElement('div')
    act(() => result.current(element))

    const observer = ResizeObserverMock.instances[0]
    observer.callback([], observer)
    observer.callback([], observer)

    expect(requestAnimationFrame).toHaveBeenCalledOnce()
    flushFrames()
    expect(fitCanvasToViewport).toHaveBeenCalledOnce()
  })

  it.each(['detaching the ref', 'unmounting the hook'])(
    '%s disconnects and cancels a queued frame',
    (cleanup) => {
      const { result, unmount } = renderHook(() => useAutoFitOnResize())
      const element = document.createElement('div')
      act(() => result.current(element))

      const observer = ResizeObserverMock.instances[0]
      observer.callback([], observer)

      if (cleanup === 'detaching the ref') {
        act(() => result.current(null))
      } else {
        unmount()
      }

      expect(observer.disconnect).toHaveBeenCalledOnce()
      expect(cancelAnimationFrame).toHaveBeenCalledWith(1)
      flushFrames()
      expect(fitCanvasToViewport).not.toHaveBeenCalled()
    },
  )
})
