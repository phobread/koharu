import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { CanvasDims, CanvasDrawingConfig } from '@/hooks/useCanvasDrawing'
import { useMaskDrawing } from '@/hooks/useMaskDrawing'
import type { Page } from '@/lib/api/schemas'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import type { ToolMode } from '@/lib/types'

const mocks = vi.hoisted(() => ({
  getConfig: vi.fn(),
  invalidateScene: vi.fn(),
  queueAutoRender: vi.fn(),
  drawingOptions: null as CanvasDrawingConfig | null,
}))

vi.mock('@/lib/api/default/default', () => ({
  getConfig: mocks.getConfig,
}))

vi.mock('@/lib/io/scene', () => ({
  invalidateScene: mocks.invalidateScene,
  queueAutoRender: mocks.queueAutoRender,
}))

vi.mock('@/hooks/useCanvasDrawing', () => ({
  useCanvasDrawing: (
    _dims: CanvasDims | null,
    _pointerToDocument: unknown,
    options: CanvasDrawingConfig,
  ) => {
    mocks.drawingOptions = options
    return {
      canvasRef: { current: null },
      visible: true,
      bind: () => ({}),
    }
  },
}))

describe('useMaskDrawing', () => {
  beforeEach(() => {
    mocks.getConfig.mockResolvedValue({ pipeline: { inpainter: 'flux2-klein' } })
    mocks.invalidateScene.mockResolvedValue(undefined)
    mocks.queueAutoRender.mockReset()
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true }))
  })

  afterEach(() => {
    usePreferencesStore.setState({ repairWithLama: false })
    vi.unstubAllGlobals()
    vi.clearAllMocks()
    mocks.drawingOptions = null
  })

  it('queues a translated-layer refresh after localized inpainting', async () => {
    const page = { id: 'page-8', width: 100, height: 100, nodes: {} } as Page
    renderHook(() =>
      useMaskDrawing({
        mode: 'repairBrush',
        page,
        pointerToDocument: vi.fn(),
        showMask: true,
        enabled: true,
      }),
    )

    const finalize = mocks.drawingOptions?.onFinalizeFullCanvas
    expect(finalize).toBeTypeOf('function')
    await act(async () => {
      await finalize?.(new Uint8Array([1, 2, 3]), { x: 10, y: 20, width: 30, height: 40 })
    })

    expect(mocks.invalidateScene).toHaveBeenCalledOnce()
    expect(mocks.queueAutoRender).toHaveBeenCalledWith('page-8')
  })

  async function strokeEngine(mode: ToolMode) {
    const page = { id: 'page-3', width: 100, height: 100, nodes: {} } as Page
    renderHook(() =>
      useMaskDrawing({ mode, page, pointerToDocument: vi.fn(), showMask: true, enabled: true }),
    )
    await act(async () => {
      await mocks.drawingOptions?.onFinalizeFullCanvas?.(new Uint8Array([1]), {
        x: 1,
        y: 2,
        width: 3,
        height: 4,
      })
    })
    const url = String(vi.mocked(fetch).mock.calls[0]?.[0])
    return new URL(url, 'http://localhost').searchParams.get('pipeline')
  }

  it('repairs with the pipeline inpainter by default', async () => {
    expect(await strokeEngine('repairBrush')).toBe('flux2-klein')
  })

  it('repairs with LaMa when the repair brush is switched to it', async () => {
    usePreferencesStore.setState({ repairWithLama: true })
    expect(await strokeEngine('repairBrush')).toBe('lama-manga')
  })

  it('keeps the pipeline inpainter for eraser strokes', async () => {
    usePreferencesStore.setState({ repairWithLama: true })
    expect(await strokeEngine('eraser')).toBe('flux2-klein')
  })
})
