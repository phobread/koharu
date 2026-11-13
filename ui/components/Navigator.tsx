'use client'

import { useVirtualizer } from '@tanstack/react-virtual'
import { CheckIcon, LayoutGridIcon, PlayIcon, StarIcon, Trash2Icon } from 'lucide-react'
import { memo, useCallback, useMemo, useRef, useState } from 'react'
import type React from 'react'
import { useTranslation } from 'react-i18next'

import { PageManagerDialog } from '@/components/PageManagerDialog'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useScene } from '@/hooks/useScene'
import { getGetPageThumbnailUrl } from '@/lib/api/default/default'
import { usePageFavourites } from '@/lib/io/favourites'
import { orderedPageIds, processPagesWithFeedback } from '@/lib/io/processPages'
import { applyOp } from '@/lib/io/scene'
import { ops } from '@/lib/ops'
import { pageStatus, type PageStatus } from '@/lib/pageStatus'
import { type ProcessSteps, usePreferencesStore } from '@/lib/stores/preferencesStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'
import { cn } from '@/lib/utils'

const THUMBNAIL_DPR =
  typeof window !== 'undefined' ? Math.min(Math.ceil(window.devicePixelRatio || 1), 3) : 2

const ROW_HEIGHT = 82
const OVERSCAN = 5

export function Navigator() {
  const { scene } = useScene()
  const pagesMap = scene?.pages
  const pages = useMemo(() => (pagesMap ? Object.values(pagesMap) : []), [pagesMap])
  const totalPages = pages.length
  const pageId = useSelectionStore((s) => s.pageId)
  const setPage = useSelectionStore((s) => s.setPage)
  const selectedPageIds = useSelectionStore((s) => s.selectedPageIds)
  const setSelectedPageIds = useSelectionStore((s) => s.setSelectedPageIds)

  const viewportRef = useRef<HTMLDivElement | null>(null)
  const { t } = useTranslation()
  const [pageManagerOpen, setPageManagerOpen] = useState(false)
  const processStepsPref = usePreferencesStore((s) => s.processSteps)
  const tickedSteps = PROCESS_STEP_LABELS.filter(({ key }) => processStepsPref[key])
  const selectedInOrder = useMemo(
    () => orderedPageIds(pagesMap, selectedPageIds),
    [pagesMap, selectedPageIds],
  )
  const { starred, setStarred } = usePageFavourites()
  // Back to every page whenever the list is opened afresh.
  const [starredOnly, setStarredOnly] = useState(false)
  // The rows to show, each with its place in the whole project.
  const shown = useMemo(() => {
    const all = pages.map((page, index) => ({ page, index }))
    return starredOnly ? all.filter(({ page }) => starred.has(page.id)) : all
  }, [pages, starredOnly, starred])

  const handleSelect = useCallback(
    (id: string, event: React.MouseEvent | React.KeyboardEvent) => {
      const clickedIndex = pages.findIndex((p) => p.id === id)
      if (clickedIndex === -1) return

      if (event.ctrlKey || event.metaKey) {
        setSelectedPageIds((prev) => {
          const next = new Set(prev)
          if (next.has(id)) {
            next.delete(id)
            if (id === pageId) {
              const remaining = Array.from(next)
              if (remaining.length > 0) {
                setPage(remaining[0])
              } else {
                next.add(id) // Active page must always be selected
              }
            }
          } else {
            next.add(id)
            setPage(id)
          }
          return next
        })
      } else if (event.shiftKey && pageId) {
        const fromIndex = pages.findIndex((p) => p.id === pageId)
        if (fromIndex !== -1) {
          const start = Math.min(fromIndex, clickedIndex)
          const end = Math.max(fromIndex, clickedIndex)
          const rangeIds = pages
            .slice(start, end + 1)
            .filter((p) => !starredOnly || starred.has(p.id))
            .map((p) => p.id)
          setSelectedPageIds(new Set(rangeIds))
          setPage(id)
        }
      } else {
        setSelectedPageIds(new Set([id]))
        setPage(id)
      }
    },
    [pages, pageId, setPage, setSelectedPageIds, starredOnly, starred],
  )

  const handleDeletePages = useCallback(
    async (idsToDelete: Set<string>) => {
      // Intersect with existing pages to filter out any stale selection IDs
      const existingPageIds = new Set(pages.map((p) => p.id))
      const validIdsToDelete = new Set(
        Array.from(idsToDelete).filter((id) => existingPageIds.has(id)),
      )
      if (validIdsToDelete.size === 0) return
      if (totalPages - validIdsToDelete.size < 1) return

      const remainingPages = pages.filter((p) => !validIdsToDelete.has(p.id))
      if (remainingPages.length === 0) return

      // Transition selection if active page is being deleted
      let nextPageId = pageId
      if (pageId && validIdsToDelete.has(pageId)) {
        const firstDeletedIndex = pages.findIndex((p) => validIdsToDelete.has(p.id))
        const nextIndex = Math.min(firstDeletedIndex, remainingPages.length - 1)
        nextPageId = remainingPages[nextIndex]?.id ?? null
        setPage(nextPageId)
      }

      // Sort indices in descending order before generating ops to maintain correctness in sequence
      const sortedIdsToDelete = Array.from(validIdsToDelete)
        .map((id) => ({ id, index: pages.findIndex((p) => p.id === id) }))
        .sort((a, b) => b.index - a.index)

      const removeOps = sortedIdsToDelete.map(({ id, index }) => {
        const pageToDelete = pagesMap?.[id]
        return ops.removePage(id, pageToDelete!, index)
      })

      if (removeOps.length > 0) {
        setSelectedPageIds(new Set(nextPageId ? [nextPageId] : []))
        if (removeOps.length === 1) {
          await applyOp(removeOps[0])
        } else {
          await applyOp(ops.batch(t('navigator.batchDelete', 'Delete pages'), removeOps))
        }
      }
    },
    [pages, pagesMap, pageId, setPage, totalPages, t, setSelectedPageIds],
  )

  const handleDeletePage = useCallback(
    (id: string) => {
      void handleDeletePages(new Set([id]))
    },
    [handleDeletePages],
  )

  const handleBatchDelete = useCallback(() => {
    const { selectedPageIds } = useSelectionStore.getState()
    void handleDeletePages(selectedPageIds)
  }, [handleDeletePages])

  const virtualizer = useVirtualizer({
    count: shown.length,
    getScrollElement: () => viewportRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: OVERSCAN,
  })

  return (
    <div
      data-testid='navigator-panel'
      data-total-pages={totalPages}
      className='flex h-full min-h-0 w-full flex-col bg-muted/50'
    >
      <div className='flex items-center justify-between border-b border-border px-2 py-1.5'>
        <div>
          <p className='text-xs tracking-wide text-muted-foreground uppercase'>
            {t('navigator.title')}
          </p>
          <p className='text-xs font-semibold text-foreground'>
            {totalPages ? t('navigator.pages', { count: totalPages }) : t('navigator.empty')}
          </p>
        </div>
        <div className='flex items-center gap-0.5'>
          {totalPages > 0 && (
            <Button
              variant='ghost'
              size='icon'
              data-testid='navigator-favourites-only'
              aria-pressed={starredOnly}
              className={cn('h-6 w-6', starredOnly && 'bg-amber-400/15 hover:bg-amber-400/25')}
              onClick={() => setStarredOnly((on) => !on)}
              title={
                starredOnly
                  ? t('navigator.favourites.showAll', 'Show all pages')
                  : t('navigator.favourites.only', 'Show only favourite pages')
              }
            >
              <StarIcon className={cn('h-3.5 w-3.5', starredOnly && STAR_ON)} />
            </Button>
          )}
          {totalPages > 1 && (
            <Button
              variant='ghost'
              size='icon'
              data-testid='navigator-manage-pages'
              className='h-6 w-6'
              onClick={() => setPageManagerOpen(true)}
              title={t('navigator.pageManager.title')}
            >
              <LayoutGridIcon className='h-3.5 w-3.5' />
            </Button>
          )}
        </div>
      </div>

      {selectedInOrder.length > 1 && (
        <div className='border-b border-border px-2 py-1.5'>
          <Button
            variant='secondary'
            size='sm'
            data-testid='navigator-process-selected'
            className='h-7 w-full gap-1.5 text-xs'
            disabled={tickedSteps.length === 0}
            title={t('process.pagesButtonHint')}
            onClick={() =>
              void processPagesWithFeedback(
                { pages: selectedInOrder, onlyMissing: true },
                t(
                  'process.nothingToDo',
                  'Nothing to process: those pages already have every step.',
                ),
              )
            }
          >
            <PlayIcon className='size-3.5' />
            {t('process.pagesButton', {
              count: selectedInOrder.length,
              defaultValue: 'Process {{count}} pages',
            })}
          </Button>
          <div
            data-testid='navigator-process-steps'
            className='mt-1 truncate text-center text-[10px] text-muted-foreground'
          >
            {tickedSteps.length > 0
              ? tickedSteps.map(({ labelKey, fallback }) => t(labelKey, fallback)).join(' · ')
              : t('process.noSteps', 'Tick steps in the Process menu first')}
          </div>
        </div>
      )}

      {totalPages === 0 && (
        <div className='px-2 py-1.5 text-xs text-muted-foreground'>{t('navigator.prompt')}</div>
      )}
      {totalPages > 0 && shown.length === 0 && (
        <div
          data-testid='navigator-favourites-empty'
          className='px-2 py-1.5 text-xs text-muted-foreground'
        >
          {t(
            'navigator.favourites.empty',
            'No favourite pages in this project yet. Point at a page and click its star.',
          )}
        </div>
      )}

      <ScrollArea className='min-h-0 flex-1' viewportRef={viewportRef}>
        <div className='relative w-full' style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const row = shown[virtualRow.index]
            if (!row) return null
            const { page, index } = row
            return (
              <div
                key={page.id}
                className='absolute left-0 w-full px-1.5 pb-1'
                style={{
                  height: ROW_HEIGHT,
                  top: 0,
                  transform: `translateY(${virtualRow.start}px)`,
                }}
              >
                <PagePreview
                  index={index}
                  pageId={page.id}
                  label={page.name}
                  status={pageStatus(page)}
                  blockCount={Object.values(page.nodes).filter((n) => 'text' in n.kind).length}
                  selected={selectedPageIds.has(page.id)}
                  active={page.id === pageId}
                  onSelect={handleSelect}
                  canDelete={totalPages > 1}
                  onDelete={handleDeletePage}
                  onBatchDelete={handleBatchDelete}
                  starred={starred.has(page.id)}
                  onStar={setStarred}
                />
              </div>
            )
          })}
        </div>
      </ScrollArea>

      <PageManagerDialog open={pageManagerOpen} onOpenChange={setPageManagerOpen} />
    </div>
  )
}

type PagePreviewProps = {
  index: number
  pageId: string
  label: string
  status: PageStatus
  blockCount: number
  selected: boolean
  active: boolean
  onSelect: (id: string, e: React.MouseEvent | React.KeyboardEvent) => void
  canDelete: boolean
  onDelete: (id: string) => void
  onBatchDelete: () => void
  starred: boolean
  onStar: (id: string, starred: boolean) => void
}

const STAR_ON = 'fill-amber-400 text-amber-500'

const PagePreview = memo(function PagePreview({
  index,
  pageId,
  label,
  status,
  blockCount,
  selected,
  active,
  onSelect,
  canDelete,
  onDelete,
  onBatchDelete,
  starred,
  onStar,
}: PagePreviewProps) {
  const src = pageId ? `${getGetPageThumbnailUrl(pageId)}?size=${200 * THUMBNAIL_DPR}` : undefined
  const { t } = useTranslation()
  const starLabel = starred
    ? t('navigator.favourites.remove', 'Remove from favourites')
    : t('navigator.favourites.add', 'Add to favourites')

  return (
    <div
      role='button'
      tabIndex={0}
      onClick={(e) => onSelect(pageId, e)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onSelect(pageId, e)
        } else if (e.key === 'Delete') {
          e.preventDefault()
          onBatchDelete()
        }
      }}
      data-testid={`navigator-page-${index}`}
      data-page-index={index}
      data-selected={selected}
      data-active={active}
      aria-label={`Page ${index + 1}: ${label}`}
      className='group relative grid h-full w-full cursor-pointer grid-cols-[48px_minmax(0,1fr)] items-center gap-2.5 rounded-xl border border-transparent p-1.5 text-left transition select-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-primary focus-visible:outline-hidden data-[active=true]:border-primary/30 data-[selected=true]:bg-primary/10'
    >
      <div className='relative flex h-16 w-12 items-center justify-center overflow-hidden rounded-lg bg-[var(--surface-well)]'>
        {src ? (
          <img
            src={src}
            alt={`Page ${index + 1}`}
            loading='lazy'
            className='max-h-full max-w-full rounded object-contain'
          />
        ) : (
          <div className='h-full w-full rounded bg-muted' />
        )}
        {canDelete && (
          <Button
            variant='destructive'
            size='icon'
            data-testid={`navigator-page-delete-${index}`}
            className='absolute top-1.5 right-1.5 h-6 w-6 rounded-full opacity-0 shadow-md transition-opacity duration-200 group-hover:opacity-100 hover:scale-105'
            onClick={(e) => {
              e.stopPropagation()
              onDelete(pageId)
            }}
            onPointerDown={(e) => e.stopPropagation()}
            onMouseDown={(e) => e.stopPropagation()}
            title={t('common.delete', 'Delete')}
          >
            <Trash2Icon className='h-3.5 w-3.5' />
          </Button>
        )}
        {/* Bottom left, clear of the delete button. Shown while set, and on
            hover otherwise. Its own clicks and keys stay off the row. */}
        <button
          type='button'
          data-testid={`navigator-page-star-${index}`}
          aria-pressed={starred}
          aria-label={starLabel}
          title={starLabel}
          className={cn(
            'absolute bottom-0.5 left-0.5 flex h-5 w-5 cursor-pointer items-center justify-center rounded-full bg-background/85 shadow-sm transition-opacity duration-200 hover:scale-110 focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-primary focus-visible:outline-hidden',
            starred ? 'opacity-100' : 'text-muted-foreground opacity-0 group-hover:opacity-100',
          )}
          onClick={(e) => {
            e.stopPropagation()
            onStar(pageId, !starred)
          }}
          onKeyDown={(e) => e.stopPropagation()}
          onPointerDown={(e) => e.stopPropagation()}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <StarIcon className={cn('h-3 w-3', starred && STAR_ON)} />
        </button>
      </div>
      <div className='min-w-0 text-xs'>
        <div className='truncate font-medium'>{label}</div>
        <div className='mt-1 text-[10px] text-muted-foreground'>{blockCount} text blocks</div>
        <PageStatusLine status={status} />
      </div>
    </div>
  )
})

const PROCESS_STEP_LABELS: { key: keyof ProcessSteps; labelKey: string; fallback: string }[] = [
  { key: 'detect', labelKey: 'processing.detect', fallback: 'Detect' },
  { key: 'ocr', labelKey: 'processing.ocr', fallback: 'OCR' },
  { key: 'translate', labelKey: 'llm.translate', fallback: 'Translate' },
  { key: 'inpaint', labelKey: 'mask.inpaint', fallback: 'Inpaint' },
  { key: 'render', labelKey: 'llm.render', fallback: 'Render' },
]

const STEP_LABELS: Record<string, string> = {
  ocr: 'OCR',
  translate: 'translation',
  clean: 'cleanup',
  render: 'render',
}

/** What the page still needs: the same rules as "Process unfinished pages". */
function PageStatusLine({ status }: { status: PageStatus }) {
  const { t } = useTranslation()
  if (status.kind === 'done') {
    return (
      <div
        data-testid='page-status'
        data-status='done'
        className='mt-1 flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-400'
      >
        <CheckIcon className='size-3' />
        {t('navigator.status.done', 'Done')}
      </div>
    )
  }
  const text =
    status.kind === 'notStarted'
      ? t('navigator.status.notStarted', 'Not started')
      : t('navigator.status.needs', {
          steps: status.steps
            .map((step) => t(`navigator.status.${step}`, STEP_LABELS[step]))
            .join(', '),
          defaultValue: 'Needs {{steps}}',
        })
  return (
    <div
      data-testid='page-status'
      data-status={status.kind}
      className='mt-1 truncate text-[10px] text-amber-600 dark:text-amber-400'
      title={text}
    >
      {text}
    </div>
  )
}
