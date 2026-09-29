'use client'

import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { cleanUpStorage } from '@/lib/api/default/default'
import type { CleanUpStorageResponse } from '@/lib/api/schemas'
import { cn } from '@/lib/utils'

/** Bytes as "5.25 GB" / "312 MB" / "48 KB". */
export function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(2)} GB`
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`
  return `${Math.max(0, Math.round(bytes / 1e3))} KB`
}

/** Below this, there is nothing worth offering to free. */
const WORTH_FREEING = 1e6

const freeable = (r: CleanUpStorageResponse) => r.unusedImageBytes + r.thumbnailBytes

/**
 * "Free up space": how much the projects take and how much of it is old
 * images no project uses any more (plus thumbnails), with a confirmed
 * button that deletes them. `compact` is the one-line home-page version.
 */
export function FreeUpSpace({ compact = false }: { compact?: boolean }) {
  const { t } = useTranslation()
  const [usage, setUsage] = useState<CleanUpStorageResponse>()
  const [confirmOpen, setConfirmOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<string>()
  const [failed, setFailed] = useState(false)

  const measure = useCallback(async () => {
    try {
      setUsage(await cleanUpStorage({ apply: false }))
    } catch {
      setUsage(undefined)
    }
  }, [])

  useEffect(() => {
    void measure()
  }, [measure])

  const skippedNote = (r: CleanUpStorageResponse) => {
    if (r.skipped.length === 0) return null
    const open = r.skipped.filter((s) => s.reason === 'open').map((s) => s.id)
    const other = r.skipped.filter((s) => s.reason !== 'open').map((s) => s.id)
    return [
      open.length > 0
        ? t('storage.skippedOpen', {
            names: open.join(', '),
            defaultValue: 'Left alone while open: {{names}}.',
          })
        : null,
      other.length > 0
        ? t('storage.skippedUnreadable', {
            names: other.join(', '),
            defaultValue: 'Left alone, could not be read safely: {{names}}.',
          })
        : null,
    ]
      .filter(Boolean)
      .join(' ')
  }

  const apply = async () => {
    setBusy(true)
    setMessage(undefined)
    setFailed(false)
    try {
      const done = await cleanUpStorage({ apply: true })
      setMessage(
        [
          t('storage.freed', {
            size: formatBytes(freeable(done)),
            defaultValue: 'Freed {{size}}.',
          }),
          done.failed > 0
            ? t('storage.someFailed', {
                count: done.failed,
                defaultValue: '{{count}} files could not be removed; try again later.',
              })
            : null,
          skippedNote(done),
        ]
          .filter(Boolean)
          .join(' '),
      )
      setFailed(done.failed > 0)
    } catch {
      setMessage(t('storage.failed', 'Could not free up space. Try again.'))
      setFailed(true)
    } finally {
      setBusy(false)
      setConfirmOpen(false)
      void measure()
    }
  }

  const canFree = usage !== undefined && freeable(usage) >= WORTH_FREEING

  const button = (
    <Button
      data-testid='free-up-space'
      variant={compact ? 'ghost' : 'outline'}
      size={compact ? 'xs' : 'default'}
      disabled={!canFree || busy}
      onClick={() => setConfirmOpen(true)}
      className={cn(compact && 'h-6 px-2 text-[11px] text-primary')}
    >
      {canFree
        ? t('storage.freeUp', {
            size: formatBytes(freeable(usage)),
            defaultValue: 'Free up {{size}}',
          })
        : t('storage.nothingToFree', 'Nothing to free up')}
    </Button>
  )

  return (
    <>
      {compact ? (
        <div className='flex items-center gap-2 text-[10px] text-muted-foreground tabular-nums'>
          {usage && (
            <span
              data-testid='projects-size'
              title={t('storage.projectsUse', {
                size: formatBytes(usage.projectsBytes),
                defaultValue: 'Projects use {{size}}',
              })}
            >
              · {formatBytes(usage.projectsBytes)}
            </span>
          )}
          {canFree && button}
          {message && (
            <span role='status' className={cn(failed && 'text-destructive')}>
              {message}
            </span>
          )}
        </div>
      ) : (
        <div className='space-y-2'>
          {usage && (
            <p data-testid='projects-size' className='text-xs text-muted-foreground'>
              {t('storage.projectsUse', {
                size: formatBytes(usage.projectsBytes),
                defaultValue: 'Projects use {{size}}',
              })}
              {canFree
                ? ` · ${t('storage.canFree', {
                    size: formatBytes(freeable(usage)),
                    defaultValue: '{{size}} can be freed',
                  })}`
                : null}
            </p>
          )}
          {button}
          {usage && !message && skippedNote(usage) && (
            <p className='text-xs text-muted-foreground'>{skippedNote(usage)}</p>
          )}
          {message && (
            <p
              role='status'
              className={cn('text-xs', failed ? 'text-destructive' : 'text-muted-foreground')}
            >
              {message}
            </p>
          )}
        </div>
      )}

      <AlertDialog open={confirmOpen} onOpenChange={(open) => !busy && setConfirmOpen(open)}>
        <AlertDialogContent>
          <AlertDialogTitle>
            {t('storage.confirmTitle', {
              size: usage ? formatBytes(freeable(usage)) : '',
              defaultValue: 'Free up {{size}}?',
            })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t('storage.confirmDescription', {
              count: usage?.unusedImages ?? 0,
              number: (usage?.unusedImages ?? 0).toLocaleString(),
              size: formatBytes(usage?.unusedImageBytes ?? 0),
              thumbs: formatBytes(usage?.thumbnailBytes ?? 0),
              defaultValue:
                'Permanently deletes {{number}} old images ({{size}}) that no project uses any more: earlier versions of cleaned and translated pages, left behind each time a page was processed again. Undo cannot bring them back, and your pages, text and current images stay as they are. Thumbnails ({{thumbs}}) go too; they are made again when shown.',
            })}{' '}
            {usage ? skippedNote(usage) : null}
          </AlertDialogDescription>
          <div className='flex justify-end gap-2'>
            <AlertDialogCancel disabled={busy}>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              data-testid='free-up-space-confirm'
              disabled={busy}
              onClick={(e) => {
                e.preventDefault()
                void apply()
              }}
            >
              {busy
                ? t('storage.freeing', 'Deleting…')
                : t('storage.confirm', {
                    size: usage ? formatBytes(freeable(usage)) : '',
                    defaultValue: 'Delete {{size}}',
                  })}
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
