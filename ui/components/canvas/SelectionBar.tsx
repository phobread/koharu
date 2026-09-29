'use client'

import { Trash2Icon, XIcon } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { useTextNodes } from '@/hooks/useCurrentPage'
import { deleteTextNodes } from '@/lib/io/scene'
import { useSelectionStore } from '@/lib/stores/selectionStore'

/**
 * Floating bar over the page while two or more boxes are selected: how many,
 * a one-click delete (one undo step) and a way to drop the selection.
 */
export function SelectionBar({ pageId }: { pageId: string }) {
  const { t } = useTranslation()
  const nodes = useTextNodes()
  const selectedIds = useSelectionStore((s) => s.nodeIds)
  const clear = useSelectionStore((s) => s.clear)
  const count = nodes.filter((n) => selectedIds.has(n.id)).length
  if (count < 2) return null

  return (
    <div
      data-testid='selection-bar'
      className='pointer-events-auto absolute bottom-4 left-1/2 z-50 flex -translate-x-1/2 items-center gap-1 rounded-full border border-border bg-popover py-1 pr-1 pl-3 text-xs text-popover-foreground shadow-lg'
    >
      <span className='pr-1 font-medium'>
        {t('selection.count', { count, defaultValue: '{{count}} boxes selected' })}
      </span>
      <Button
        variant='destructive'
        size='sm'
        data-testid='selection-delete'
        className='h-7 gap-1 rounded-full px-3 text-xs'
        title={t('selection.deleteHint', 'Delete the selected boxes (Delete)')}
        onClick={() => void deleteTextNodes(pageId, useSelectionStore.getState().nodeIds)}
      >
        <Trash2Icon className='size-3.5' />
        {t('selection.delete', 'Delete')}
      </Button>
      <Button
        variant='ghost'
        size='icon'
        data-testid='selection-clear'
        className='size-7 rounded-full'
        title={t('selection.clear', 'Clear selection')}
        aria-label={t('selection.clear', 'Clear selection')}
        onClick={clear}
      >
        <XIcon className='size-3.5' />
      </Button>
    </div>
  )
}
