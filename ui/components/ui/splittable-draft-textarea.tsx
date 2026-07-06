'use client'

import { useRef, useState } from 'react'

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu'
import { DraftTextarea, type DraftTextareaProps } from '@/components/ui/draft-textarea'

/**
 * DraftTextarea with a right-click "split block at cursor" action. The caret
 * position at the moment of the right-click (Chromium places the caret before
 * firing `contextmenu`) becomes the split offset. Splitting is disabled when
 * the caret sits at the very start/end — both halves need text.
 */
export function SplittableDraftTextarea({
  splitLabel,
  onSplit,
  ...props
}: DraftTextareaProps & { splitLabel: string; onSplit: (offset: number) => void }) {
  const caretRef = useRef(0)
  const [canSplit, setCanSplit] = useState(false)
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          className='contents'
          onContextMenu={(e) => {
            const el = e.target as HTMLTextAreaElement
            if (typeof el.selectionStart !== 'number') return
            const offset = el.selectionStart
            caretRef.current = offset
            setCanSplit(offset > 0 && offset < el.value.length)
          }}
        >
          <DraftTextarea {...props} />
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem disabled={!canSplit} onSelect={() => onSplit(caretRef.current)}>
          {splitLabel}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}
