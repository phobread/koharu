'use client'

import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { useGetBootstrap, useRetryBootstrap } from '@/lib/api/default/default'
import { BootstrapState, type DownloadProgress } from '@/lib/api/schemas'
import { useDownloadsStore } from '@/lib/stores/downloadsStore'

const summarize = (downloads: Record<string, DownloadProgress>) => {
  const values = Object.values(downloads)
  if (values.length === 0) return null
  let total = 0
  let downloaded = 0
  let active: string | null = null
  for (const d of values) {
    total += d.total ?? 0
    downloaded += d.downloaded
    const s = d.status.status
    if (s === 'started' || s === 'downloading') active = d.filename
  }
  return {
    filename: active,
    percent: total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : undefined,
  }
}

export function AppInitializationSkeleton() {
  const { t } = useTranslation()
  const downloads = useDownloadsStore((s) => s.downloads)
  const progress = useMemo(() => summarize(downloads), [downloads])

  // Startup failures (e.g. a runtime download error) are reported here
  // instead of the app exiting, so the user can retry.
  const { data: bootstrap, refetch } = useGetBootstrap({
    query: { refetchInterval: 1500, retry: false },
  })
  const retry = useRetryBootstrap({ mutation: { onSettled: () => refetch() } })
  const failed = bootstrap?.state === BootstrapState.failed

  return (
    <div className='flex min-h-0 flex-1 items-center justify-center bg-background'>
      <div className='flex flex-col items-center gap-6'>
        <img
          src='/icon-large.png'
          alt='Koharu'
          className='h-20 w-20 opacity-80'
          draggable={false}
        />
        <div className='flex flex-col items-center gap-1'>
          <h1 className='text-lg font-semibold tracking-widest text-foreground uppercase'>
            Koharu
          </h1>
          <p className='text-xs text-muted-foreground'>
            {failed ? t('startup.failed') : t('common.initializing')}
          </p>
        </div>
        {failed ? (
          <div className='flex w-80 flex-col items-center gap-3'>
            <p className='max-h-32 overflow-auto text-center text-xs break-words text-destructive'>
              {bootstrap?.error}
            </p>
            <Button size='sm' onClick={() => retry.mutate()} disabled={retry.isPending}>
              {t('startup.retry')}
            </Button>
          </div>
        ) : (
          <div className='w-56'>
            <p className='mb-1.5 h-4 truncate text-center text-[11px] text-muted-foreground'>
              {progress?.filename ?? ' '}
            </p>
            <Progress
              value={progress?.percent ?? 0}
              className={`h-1.5 ${progress ? 'visible' : 'invisible'}`}
            />
          </div>
        )}
      </div>
    </div>
  )
}
