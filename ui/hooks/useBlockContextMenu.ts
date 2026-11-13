'use client'

import { useState } from 'react'
import type React from 'react'

import { isTextNode } from '@/hooks/useCurrentPage'
import type { DocumentPointer, PointerToDocumentFn } from '@/hooks/usePointerToDocument'
import type { Page } from '@/lib/api/schemas'
import { displayBox } from '@/lib/displayBox'

type BlockContextMenuOptions = {
  page: Page | null
  pointerToDocument: PointerToDocumentFn
  onSelect: (nodeId: string | null) => void
  onRemove: (nodeId: string) => void
  onSplit: (nodeId: string) => void
  /** Open the menu on the picture itself too: the page has actions that
   * work on a point of it rather than on a box. */
  menuOnPicture?: boolean
}

/**
 * Right-click on a text node pops the context menu. `onSelect(null)` / clear
 * on empty-space right-click. `onRemove(id)` triggered by menu's delete item.
 * With `menuOnPicture`, a right-click that hits no box opens the menu as
 * well and `contextMenuPoint` holds where on the page it was.
 */
export function useBlockContextMenu({
  page,
  pointerToDocument,
  onSelect,
  onRemove,
  onSplit,
  menuOnPicture = false,
}: BlockContextMenuOptions) {
  const [contextMenuNodeId, setContextMenuNodeId] = useState<string | null>(null)
  const [contextMenuPoint, setContextMenuPoint] = useState<DocumentPointer | null>(null)

  const handleContextMenu = (event: React.MouseEvent<HTMLElement>) => {
    if (!page) return
    const point = pointerToDocument(event)
    if (!point) {
      event.preventDefault()
      setContextMenuNodeId(null)
      setContextMenuPoint(null)
      onSelect(null)
      return
    }
    const hitId = Object.entries(page.nodes).find(([, n]) => {
      if (!isTextNode(n)) return false
      if (!n.transform) return false
      const t = displayBox(n.transform, n.kind.text)
      return (
        point.x >= t.x && point.x <= t.x + t.width && point.y >= t.y && point.y <= t.y + t.height
      )
    })?.[0]
    if (hitId) {
      onSelect(hitId)
      setContextMenuNodeId(hitId)
      setContextMenuPoint(null)
    } else {
      // Without an action for the picture the menu has nothing to offer.
      if (!menuOnPicture) event.preventDefault()
      setContextMenuNodeId(null)
      setContextMenuPoint(menuOnPicture ? point : null)
      onSelect(null)
    }
  }

  const handleDeleteBlock = () => {
    if (!contextMenuNodeId) return
    onRemove(contextMenuNodeId)
    setContextMenuNodeId(null)
  }

  const handleSplitBlock = () => {
    if (!contextMenuNodeId) return
    onSplit(contextMenuNodeId)
    setContextMenuNodeId(null)
  }

  const clearContextMenu = () => {
    setContextMenuNodeId(null)
    setContextMenuPoint(null)
  }

  return {
    contextMenuNodeId,
    contextMenuPoint,
    handleContextMenu,
    handleDeleteBlock,
    handleSplitBlock,
    clearContextMenu,
  }
}
