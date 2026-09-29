'use client'

import { ArrowLeftIcon } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { useScene } from '@/hooks/useScene'
import { closeProject } from '@/lib/io/scene'
import { formatShortcutForDisplay, getPlatform } from '@/lib/shortcutUtils'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import { cn } from '@/lib/utils'

/** Back arrow (closes the project) followed by the open project's name. */
export function ProjectTitle({ className }: { className?: string }) {
  const { t } = useTranslation()
  const { scene } = useScene()
  const closeShortcut = usePreferencesStore((s) => s.shortcuts.closeProject)
  if (!scene) return null

  const name = scene.project?.name ?? ''
  const label = t('project.back', 'Back to projects')
  const shortcut = formatShortcutForDisplay(closeShortcut, getPlatform() === 'mac')

  return (
    <div className={cn('flex min-w-0 items-center gap-1', className)}>
      <Button
        variant='ghost'
        size='icon'
        data-testid='project-back'
        className='h-6 w-6 shrink-0'
        onClick={() => void closeProject()}
        title={shortcut ? `${label} (${shortcut})` : label}
        aria-label={label}
      >
        <ArrowLeftIcon className='size-4' />
      </Button>
      <span
        data-testid='project-name'
        className='truncate text-xs font-semibold text-foreground'
        title={name}
      >
        {name}
      </span>
    </div>
  )
}
