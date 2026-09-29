'use client'

import { create } from 'zustand'

/**
 * Page + node selection. Multi-select via `nodeIds: Set<string>`; the Navigator
 * and hotkeys read `pageId`; component interactions read `nodeIds`.
 */
type SelectionState = {
  pageId: string | null
  nodeIds: Set<string>
  /**
   * Whether the floating quick editor may open for a single selected box.
   * Ctrl/Shift-click and drag-select build a selection to act on, so they
   * keep it closed (it would cover the next box to click).
   */
  quickEdit: boolean
  selectedPageIds: Set<string>

  setPage: (id: string | null) => void
  select: (id: string, additive?: boolean) => void
  selectMany: (ids: string[], options?: { quickEdit?: boolean }) => void
  deselect: (id: string) => void
  clear: () => void
  isSelected: (id: string) => boolean
  setSelectedPageIds: (ids: Set<string> | ((prev: Set<string>) => Set<string>)) => void
}

export const useSelectionStore = create<SelectionState>((set, get) => ({
  pageId: null,
  nodeIds: new Set(),
  quickEdit: true,
  selectedPageIds: new Set(),

  setPage: (id) =>
    set((state) => {
      const nextSelected = new Set(state.selectedPageIds)
      if (id) {
        if (!nextSelected.has(id)) {
          nextSelected.clear()
          nextSelected.add(id)
        }
      } else {
        nextSelected.clear()
      }
      return {
        pageId: id,
        // Clear selection when the page changes — node ids are page-scoped.
        nodeIds: new Set(),
        quickEdit: true,
        selectedPageIds: nextSelected,
      }
    }),

  select: (id, additive) =>
    set((state) => {
      if (additive) {
        const next = new Set(state.nodeIds)
        if (next.has(id)) next.delete(id)
        else next.add(id)
        return { nodeIds: next, quickEdit: false }
      }
      return { nodeIds: new Set([id]), quickEdit: true }
    }),

  selectMany: (ids, options) =>
    set(() => ({ nodeIds: new Set(ids), quickEdit: options?.quickEdit ?? true })),

  deselect: (id) =>
    set((state) => {
      if (!state.nodeIds.has(id)) return state
      const next = new Set(state.nodeIds)
      next.delete(id)
      return { nodeIds: next }
    }),

  clear: () => set({ nodeIds: new Set(), quickEdit: true }),

  isSelected: (id) => get().nodeIds.has(id),

  setSelectedPageIds: (ids) =>
    set((state) => {
      const next = typeof ids === 'function' ? ids(state.selectedPageIds) : ids
      return { selectedPageIds: new Set(next) }
    }),
}))
