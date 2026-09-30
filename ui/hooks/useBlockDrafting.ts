'use client'

import { useDrag } from '@use-gesture/react'
import { useRef, useState } from 'react'

import type { DocumentPointer, PointerToDocumentFn } from '@/hooks/usePointerToDocument'
import type { Page } from '@/lib/api/schemas'
import { displayBox } from '@/lib/displayBox'
import { useSelectionStore } from '@/lib/stores/selectionStore'
import type { ToolMode } from '@/lib/types'

/**
 * Rectangle a user is drawing while `mode === 'block'`. Committed on stroke
 * end via `onCreateBlock` (which dispatches `Op::AddNode` with a text node).
 */
export type BlockDraft = {
  x: number
  y: number
  width: number
  height: number
}

type BlockDraftingOptions = {
  mode: ToolMode
  page: Page | null
  /** Select mode: a drag across the picture selects every box it touches. */
  areaSelect: boolean
  pointerToDocument: PointerToDocumentFn
  clearSelection: () => void
  onCreateBlock: (draft: BlockDraft) => void
}

export function useBlockDrafting({
  mode,
  page,
  areaSelect,
  pointerToDocument,
  clearSelection,
  onCreateBlock,
}: BlockDraftingOptions) {
  const dragStartRef = useRef<DocumentPointer | null>(null)
  const draftRef = useRef<BlockDraft | null>(null)
  const [draft, setDraft] = useState<BlockDraft | null>(null)
  const areaStartRef = useRef<{ point: DocumentPointer; base: string[] } | null>(null)
  const [area, setArea] = useState<BlockDraft | null>(null)

  const endArea = () => {
    if (!areaStartRef.current) return
    areaStartRef.current = null
    setArea(null)
  }

  // Drag-to-select: live selection of every box the rectangle touches.
  // Shift keeps the boxes that were already selected. Ctrl+drag pans the
  // view instead, and presses on a box belong to the box.
  const trackArea = (
    event: PointerEvent | MouseEvent,
    first: boolean,
    done: boolean,
    tap: boolean,
  ) => {
    if (first) {
      const target = event.target
      const onBox = target instanceof Element && !!target.closest('[data-text-block-layer]')
      const start = pointerToDocument(event)
      if (!tap && !onBox && !event.ctrlKey && !event.metaKey && start) {
        const base = event.shiftKey ? [...useSelectionStore.getState().nodeIds] : []
        areaStartRef.current = { point: start, base }
      }
    }
    const start = areaStartRef.current
    if (!start || !page) return
    const point = pointerToDocument(event)
    if (point) {
      const rect = rectBetween(start.point, point)
      setArea(rect)
      const hits = textNodesTouching(page, rect)
      useSelectionStore
        .getState()
        .selectMany([...new Set([...start.base, ...hits])], { quickEdit: false })
    }
    if (done) endArea()
  }

  const reset = () => {
    dragStartRef.current = null
    draftRef.current = null
    setDraft(null)
  }

  const finalize = () => {
    if (mode !== 'block') {
      reset()
      return
    }
    const d = draftRef.current
    reset()
    if (!d || !page) return
    const MIN = 4
    if (d.width < MIN || d.height < MIN) return
    onCreateBlock({
      x: Math.round(d.x),
      y: Math.round(d.y),
      width: Math.round(d.width),
      height: Math.round(d.height),
    })
  }

  const bind = useDrag(
    ({ first, last, event, active, tap }) => {
      if (!page) return
      if (mode === 'select' && areaSelect) {
        trackArea(event as PointerEvent, first, last || !active, tap)
        return
      }
      if (mode !== 'block') return
      const sourceEvent = event as MouseEvent
      const point = pointerToDocument(sourceEvent)
      if (!point) {
        if ((last || !active) && draftRef.current) finalize()
        return
      }

      if (first) {
        dragStartRef.current = point
        const next: BlockDraft = { x: point.x, y: point.y, width: 0, height: 0 }
        draftRef.current = next
        setDraft(next)
        clearSelection()
        return
      }

      const start = dragStartRef.current
      if (!start) return
      const x = Math.min(start.x, point.x)
      const y = Math.min(start.y, point.y)
      const width = Math.abs(point.x - start.x)
      const height = Math.abs(point.y - start.y)
      const next: BlockDraft = { x, y, width, height }
      draftRef.current = next
      setDraft(next)

      if (last || !active) finalize()
    },
    {
      // Pointer-only gesture: use-gesture's default arrow-key "keyboard drag"
      // would otherwise preventDefault arrow keydowns bubbling up from the
      // quick editor's textareas, freezing the caret there. The switch lives
      // under `pointer` (a top-level `keys` is silently ignored).
      pointer: { buttons: 1, touch: true, keys: false },
      preventDefault: true,
      filterTaps: true,
      eventOptions: { passive: false },
    },
  )

  return { draftBlock: draft, selectionArea: area, bind, resetDraft: reset }
}

function rectBetween(a: DocumentPointer, b: DocumentPointer): BlockDraft {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    width: Math.abs(b.x - a.x),
    height: Math.abs(b.y - a.y),
  }
}

/** Ids of the page's text boxes whose (rotated) bounds overlap `rect`. */
export function textNodesTouching(page: Page, rect: BlockDraft): string[] {
  const ids: string[] = []
  for (const [id, node] of Object.entries(page.nodes)) {
    if (!node?.transform || !('text' in node.kind)) continue
    const { x, y, width, height, rotationDeg } = displayBox(node.transform, node.kind.text)
    const rad = ((rotationDeg ?? 0) * Math.PI) / 180
    const cos = Math.abs(Math.cos(rad))
    const sin = Math.abs(Math.sin(rad))
    const halfW = (width * cos + height * sin) / 2
    const halfH = (width * sin + height * cos) / 2
    const cx = x + width / 2
    const cy = y + height / 2
    if (
      cx + halfW > rect.x &&
      cx - halfW < rect.x + rect.width &&
      cy + halfH > rect.y &&
      cy - halfH < rect.y + rect.height
    ) {
      ids.push(id)
    }
  }
  return ids
}
