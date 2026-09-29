'use client'

import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { isTauri } from '@/lib/backend'
import { defaultExportFolder, pickSaveDirectory } from '@/lib/io/saveBlob'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'

/**
 * Settings: the folder exports go to. Each project gets its own folder
 * inside it (e.g. `<folder>/<project>/Rendered`), where the save dialog
 * opens. Desktop only: the web build saves through the browser.
 */
export function ExportFolderSetting() {
  const { t } = useTranslation()
  const exportFolder = usePreferencesStore((s) => s.exportFolder)
  const setExportFolder = usePreferencesStore((s) => s.setExportFolder)
  const [fallback, setFallback] = useState<string>()

  useEffect(() => {
    void defaultExportFolder().then(setFallback)
  }, [])

  if (!isTauri()) return null

  const change = async () => {
    const picked = await pickSaveDirectory(exportFolder ?? fallback)
    if (picked) setExportFolder(picked)
  }

  return (
    <div className='space-y-1.5'>
      <Label className='text-xs'>{t('settings.exportFolder', 'Export folder')}</Label>
      <div className='flex items-center gap-2'>
        <Input
          readOnly
          data-testid='export-folder-path'
          value={exportFolder ?? fallback ?? ''}
          title={exportFolder ?? fallback}
        />
        <Button variant='outline' data-testid='export-folder-change' onClick={() => void change()}>
          {t('settings.exportFolderChange', 'Change…')}
        </Button>
        {exportFolder && (
          <Button
            variant='ghost'
            data-testid='export-folder-reset'
            onClick={() => setExportFolder(undefined)}
          >
            {t('settings.exportFolderReset', 'Use Pictures\\Koharu')}
          </Button>
        )}
      </div>
      <p className='text-xs leading-relaxed text-muted-foreground'>
        {t(
          'settings.exportFolderDescription',
          'Each project gets its own folder in here, and exports open the save dialog there (translated pages in <project>\\Rendered).',
        )}
      </p>
    </div>
  )
}
