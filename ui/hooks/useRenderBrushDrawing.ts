'use client'

import { useRef, type RefObject } from 'react'

import { useCanvasDrawing, type CanvasDims } from '@/hooks/useCanvasDrawing'
import type { PointerToDocumentFn } from '@/hooks/usePointerToDocument'
import type { Page } from '@/lib/api/schemas'
import { invalidateScene } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import type { ToolMode } from '@/lib/types'

type RenderBrushOptions = {
  mode: ToolMode
  page: Page | null
  pointerToDocument: PointerToDocumentFn
  enabled: boolean
  action: 'paint' | 'erase'
  targetCanvasRef?: RefObject<HTMLCanvasElement | null>
}

/**
 * Color-brush over the `Mask { role: brushInpaint }` node. Stroke finalize
 * PUTs the updated mask to `/api/v1/pages/{id}/masks/brushInpaint`.
 */
export function useRenderBrushDrawing({
  page,
  pointerToDocument,
  enabled,
  action,
  targetCanvasRef,
}: RenderBrushOptions) {
  const isErasing = action === 'erase'
  const dims: CanvasDims | null = page
    ? { width: page.width, height: page.height, key: page.id }
    : null

  // Strokes commit full-canvas snapshots, so PUTs must land in stroke order —
  // an earlier slow request finishing last would overwrite the newer strokes
  // it doesn't contain. Same queue pattern as the repair brush.
  const putQueueRef = useRef<Promise<void>>(Promise.resolve())

  return useCanvasDrawing(dims, pointerToDocument, {
    getColor: () => (isErasing ? '#000000' : usePreferencesStore.getState().brushConfig.color),
    blendMode: isErasing ? 'destination-out' : 'source-over',
    getBrushSize: () => usePreferencesStore.getState().brushConfig.size,
    enabled,
    targetCanvasRef,
    clearAfterStroke: true,
    onFinalize: async () => {},
    onFinalizeFullCanvas: async (fullPng) => {
      if (!page) return
      // Bind the id now: the queue may drain after the user switched pages,
      // and the snapshot belongs to the page it was drawn on.
      const pageId = page.id
      putQueueRef.current = putQueueRef.current.then(async () => {
        try {
          const res = await fetch(`/api/v1/pages/${pageId}/masks/brushInpaint`, {
            method: 'PUT',
            headers: { 'Content-Type': 'image/png' },
            body: fullPng as unknown as BodyInit,
          })
          if (!res.ok) throw new Error(`brush PUT failed: ${res.status}`)
          await invalidateScene()
          useEditorUiStore.getState().setShowBrushLayer(true)
        } catch (e) {
          useEditorUiStore.getState().showError(String(e))
        }
      })
      await putQueueRef.current
    },
  })
}
