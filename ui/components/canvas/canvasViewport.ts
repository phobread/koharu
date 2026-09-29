'use client'

import { useEditorUiStore } from '@/lib/stores/editorUiStore'

const canvasViewportRef: { current: HTMLDivElement | null } = { current: null }
let docSize: { width: number; height: number } | null = null

export function setCanvasViewport(element: HTMLDivElement | null) {
  canvasViewportRef.current = element
}

export function setCanvasDocumentSize(width: number, height: number) {
  docSize = { width, height }
}

export function fitCanvasToViewport() {
  const viewport = canvasViewportRef.current
  if (!docSize || !viewport) return
  const rect = viewport.getBoundingClientRect()
  if (!rect.width || !rect.height || !docSize.width || !docSize.height) return
  const scaleW = ((rect.width - 10) / docSize.width) * 100
  const scaleH = ((rect.height - 10) / docSize.height) * 100
  const fit = Math.max(10, Math.min(100, Math.min(scaleW, scaleH)))
  useEditorUiStore.getState().setAutoFitEnabled(true)
  useEditorUiStore.getState().setScale(fit)
}

export function resetCanvasScale() {
  useEditorUiStore.getState().setAutoFitEnabled(false)
  useEditorUiStore.getState().setScale(100)
}

/**
 * Zoom so `box` (document pixels) fills most of the viewport, up to 100 %,
 * and scroll it to the middle, keeping `reserveRight` pixels free on the
 * right for the box editor that floats beside it.
 */
export function zoomCanvasToBox(
  box: { x: number; y: number; width: number; height: number },
  reserveRight = 0,
): boolean {
  const viewport = canvasViewportRef.current
  if (!viewport || box.width <= 0 || box.height <= 0) return false
  const view = viewport.getBoundingClientRect()
  const margin = 80
  const availableWidth = Math.max(40, view.width - reserveRight - margin)
  const availableHeight = Math.max(40, view.height - margin)
  const scale = Math.min((availableWidth / box.width) * 100, (availableHeight / box.height) * 100)
  const store = useEditorUiStore.getState()
  store.setAutoFitEnabled(false)
  store.setScale(Math.max(10, Math.min(100, scale)))
  // Scroll once the resized canvas has laid out.
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      const canvas = viewport.querySelector('[data-testid="workspace-canvas"]')
      if (!canvas) return
      const applied = useEditorUiStore.getState().scale / 100
      const page = canvas.getBoundingClientRect()
      const current = viewport.getBoundingClientRect()
      const boxCenterX = page.left + (box.x + box.width / 2) * applied
      const boxCenterY = page.top + (box.y + box.height / 2) * applied
      viewport.scrollLeft += boxCenterX - (current.left + (current.width - reserveRight) / 2)
      viewport.scrollTop += boxCenterY - (current.top + current.height / 2)
    }),
  )
  return true
}
