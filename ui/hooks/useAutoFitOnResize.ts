'use client'

import { useCallback, useEffect, useRef } from 'react'

import { fitCanvasToViewport } from '@/components/canvas/canvasViewport'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

export function useAutoFitOnResize() {
  const observerRef = useRef<ResizeObserver | null>(null)
  const frameRef = useRef<number | null>(null)

  const disconnect = useCallback(() => {
    observerRef.current?.disconnect()
    observerRef.current = null

    if (frameRef.current !== null) {
      cancelAnimationFrame(frameRef.current)
      frameRef.current = null
    }
  }, [])

  useEffect(() => disconnect, [disconnect])

  return useCallback(
    (el: HTMLDivElement | null) => {
      disconnect()
      if (!el || typeof ResizeObserver === 'undefined') return

      const observer = new ResizeObserver(() => {
        if (frameRef.current !== null) return

        frameRef.current = requestAnimationFrame(() => {
          frameRef.current = null
          if (useEditorUiStore.getState().autoFitEnabled) fitCanvasToViewport()
        })
      })
      observerRef.current = observer
      observer.observe(el)
    },
    [disconnect],
  )
}
