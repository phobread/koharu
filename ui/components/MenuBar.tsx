'use client'

import { CopyIcon, MinusIcon, SquareIcon, XIcon } from 'lucide-react'
import Image from 'next/image'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { fitCanvasToViewport, resetCanvasScale } from '@/components/Canvas'
import { SettingsDialog, type TabId } from '@/components/SettingsDialog'
import {
  MenubarCheckboxItem,
  Menubar,
  MenubarContent,
  MenubarItem,
  MenubarMenu,
  MenubarRadioGroup,
  MenubarRadioItem,
  MenubarSeparator,
  MenubarShortcut,
  MenubarTrigger,
  MenubarSub,
  MenubarSubContent,
  MenubarSubTrigger,
} from '@/components/ui/menubar'
import { useScene } from '@/hooks/useScene'
import { getConfig, listOperations, startPipeline } from '@/lib/api/default/default'
import type { JobSummary } from '@/lib/api/schemas'
import { isTauri, openExternalUrl } from '@/lib/backend'
import { exportCurrentProjectAs, importPages } from '@/lib/io/pagesIo'
import { renderDefaultsForPipeline } from '@/lib/io/renderDefaults'
import { pickSaveDirectory } from '@/lib/io/saveBlob'
import { closeProject, redoOp, selectAllTextNodesOnCurrentPage, undoOp } from '@/lib/io/scene'
import { formatShortcutForDisplay, getPlatform } from '@/lib/shortcutUtils'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useJobsStore } from '@/lib/stores/jobsStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

const windowControls = {
  async close() {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    return getCurrentWindow().close()
  },
  async minimize() {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    return getCurrentWindow().minimize()
  },
  async toggleMaximize() {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    return getCurrentWindow().toggleMaximize()
  },
  async isMaximized() {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    return getCurrentWindow().isMaximized()
  },
}

const FINAL_JOB_STATUSES = new Set<JobSummary['status']>([
  'completed',
  'completed_with_errors',
  'cancelled',
  'failed',
])

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

function isFinalJob(job: JobSummary | undefined): job is JobSummary {
  return !!job && FINAL_JOB_STATUSES.has(job.status)
}

function assertJobFinishedSuccessfully(job: JobSummary): void {
  if (job.status === 'failed') {
    throw new Error(job.error ?? 'Processing failed.')
  }
  if (job.status === 'cancelled') {
    throw new Error('Processing was cancelled before export.')
  }
}

async function waitForOperation(operationId: string): Promise<JobSummary> {
  let done = false
  let unsubscribe: (() => void) | undefined

  const storePromise = new Promise<JobSummary>((resolve) => {
    const finish = (job: JobSummary | undefined) => {
      if (!isFinalJob(job)) return false
      done = true
      unsubscribe?.()
      resolve(job)
      return true
    }

    if (finish(useJobsStore.getState().jobs[operationId])) return
    unsubscribe = useJobsStore.subscribe((state) => {
      finish(state.jobs[operationId])
    })
  })

  const pollPromise = (async () => {
    while (!done) {
      try {
        const job = (await listOperations()).operations.find((op) => op.id === operationId)
        if (isFinalJob(job)) return job
      } catch (err) {
        console.warn('Operation status poll failed:', err)
      }
      await sleep(2000)
    }
    return useJobsStore.getState().jobs[operationId]
  })()

  const job = await Promise.race([storePromise, pollPromise])
  done = true
  unsubscribe?.()
  if (!job) throw new Error('Processing finished, but its final status was unavailable.')
  return job
}

type MenuItem = {
  label: string
  onSelect?: () => void | Promise<void>
  disabled?: boolean
  testId?: string
}

export function MenuBar() {
  const { t } = useTranslation()
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [settingsTab, setSettingsTab] = useState<TabId>('appearance')
  const [processExporting, setProcessExporting] = useState(false)
  const hasPage = useSelectionStore((s) => s.pageId !== null)
  const hasScene = useScene().scene !== null
  const shortcuts = usePreferencesStore((state) => state.shortcuts)
  const customPipeline = usePreferencesStore((state) => state.customPipeline)
  const ocrLanguage = usePreferencesStore((state) => state.ocrLanguage)
  const setOcrLanguage = usePreferencesStore((state) => state.setOcrLanguage)
  const setCustomPipeline = usePreferencesStore((state) => state.setCustomPipeline)
  const hasSelectedSteps = useMemo(
    () => Object.values(customPipeline).some(Boolean),
    [customPipeline],
  )
  const isMac = useMemo(() => getPlatform() === 'mac', [])

  const requirePageId = () => {
    const id = useSelectionStore.getState().pageId
    if (!id) throw new Error('No current page selected')
    return id
  }

  const runPipeline = async (opts: { pageId?: string }) => {
    const cfg = await getConfig()
    if (!cfg.pipeline) return
    const p = cfg.pipeline
    const steps = [
      p.detector,
      p.segmenter,
      p.bubble_segmenter,
      p.font_detector,
      p.ocr,
      p.translator,
      p.inpainter,
      p.renderer,
    ].filter((s): s is string => !!s)
    const editor = useEditorUiStore.getState()
    const prefs = usePreferencesStore.getState()
    return startPipeline({
      steps,
      pages: opts.pageId ? [opts.pageId] : undefined,
      targetLanguage: editor.selectedLanguage,
      sourceLanguage: prefs.ocrLanguage,
      systemPrompt: prefs.customSystemPrompt,
      readingOrder: editor.readingOrder === 'custom' ? undefined : editor.readingOrder,
      ...renderDefaultsForPipeline(),
    })
  }

  const runPipelineAndExportRendered = async () => {
    if (processExporting) return
    setProcessExporting(true)
    try {
      const desktop = isTauri()
      const outputDirectory = desktop ? await pickSaveDirectory() : undefined
      if (desktop && !outputDirectory) return

      const started = await runPipeline({})
      if (!started?.operationId) {
        throw new Error('Could not start processing because no pipeline is configured.')
      }

      const finished = await waitForOperation(started.operationId)
      assertJobFinishedSuccessfully(finished)
      await exportCurrentProjectAs('rendered', undefined, { outputDirectory })
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err)
      useEditorUiStore.getState().showError(`Process/export failed: ${raw}`)
    } finally {
      setProcessExporting(false)
    }
  }

  const runInpaint = async (pageId: string) => {
    const cfg = await getConfig()
    if (!cfg.pipeline?.inpainter) return
    await startPipeline({ steps: [cfg.pipeline.inpainter], pages: [pageId] })
  }

  const runCustomPipeline = async (opts: { pageId?: string }) => {
    const cfg = await getConfig()
    if (!cfg.pipeline) return
    const p = cfg.pipeline
    const prefs = usePreferencesStore.getState()
    const steps = [
      ...(prefs.customPipeline.detect
        ? [p.detector, p.segmenter, p.bubble_segmenter, p.font_detector]
        : []),
      prefs.customPipeline.ocr ? p.ocr : null,
      prefs.customPipeline.translator ? p.translator : null,
      prefs.customPipeline.inpainter ? p.inpainter : null,
      prefs.customPipeline.renderer ? p.renderer : null,
    ].filter((s): s is string => !!s)
    const editor = useEditorUiStore.getState()
    await startPipeline({
      steps,
      pages: opts.pageId ? [opts.pageId] : undefined,
      targetLanguage: editor.selectedLanguage,
      sourceLanguage: prefs.ocrLanguage,
      systemPrompt: prefs.customSystemPrompt,
      readingOrder: editor.readingOrder === 'custom' ? undefined : editor.readingOrder,
      ...renderDefaultsForPipeline(),
    })
  }

  const exportItems: MenuItem[] = [
    {
      label: t('menu.export'),
      onSelect: () => void exportCurrentProjectAs('rendered', [requirePageId()]),
      disabled: !hasPage,
      testId: 'menu-file-export',
    },
    {
      label: t('menu.exportPsd'),
      onSelect: () => void exportCurrentProjectAs('psd', [requirePageId()]),
      disabled: !hasPage,
      testId: 'menu-file-export-psd',
    },
    {
      label: t('menu.exportAllInpainted'),
      onSelect: () => void exportCurrentProjectAs('inpainted'),
      disabled: !hasScene,
      testId: 'menu-file-export-all-inpainted',
    },
    {
      label: t('menu.exportAllRendered'),
      onSelect: () => void exportCurrentProjectAs('rendered'),
      disabled: !hasScene,
      testId: 'menu-file-export-all-rendered',
    },
  ]

  const helpMenuItems: MenuItem[] = [
    { label: t('menu.discord'), onSelect: () => openExternalUrl('https://discord.gg/mHvHkxGnUY') },
    {
      label: t('menu.github'),
      onSelect: () => openExternalUrl('https://github.com/mayocream/koharu'),
    },
  ]

  const isNativeMacOS = isTauri() && isMac
  const isWindowsTauri = isTauri() && !isMac

  return (
    <div className='flex h-8 items-center border-b border-border bg-background text-[13px] text-foreground'>
      {isNativeMacOS && <MacOSControls />}
      <div className='flex h-full items-center pl-2 select-none'>
        <Image src='/icon.png' alt='Koharu' width={18} height={18} draggable={false} />
      </div>
      <Menubar className='h-auto gap-1 border-none bg-transparent p-0 px-1.5 shadow-none'>
        <MenubarMenu>
          <MenubarTrigger
            data-testid='menu-file-trigger'
            className='rounded px-3 py-1.5 font-medium hover:bg-accent data-[state=open]:bg-accent'
          >
            {t('menu.file')}
          </MenubarTrigger>
          <MenubarContent className='min-w-48' align='start' sideOffset={5} alignOffset={-3}>
            <MenubarItem
              data-testid='menu-file-open-files'
              className='text-[13px]'
              disabled={!hasScene}
              onSelect={() => void importPages('replace', 'files')}
            >
              {t('menu.openFiles')}
            </MenubarItem>
            <MenubarItem
              data-testid='menu-file-open-folder'
              className='text-[13px]'
              disabled={!hasScene}
              onSelect={() => void importPages('replace', 'folder')}
            >
              {t('menu.openFolder')}
            </MenubarItem>
            <MenubarSeparator />
            <MenubarItem
              data-testid='menu-file-save-as'
              className='text-[13px]'
              disabled={!hasScene}
              onSelect={() => void exportCurrentProjectAs('khr')}
            >
              {t('menu.saveAs')}
            </MenubarItem>
            <MenubarSeparator />
            {exportItems.map((item) => (
              <MenubarItem
                key={item.label}
                data-testid={item.testId}
                className='text-[13px]'
                disabled={item.disabled}
                onSelect={item.onSelect ? () => void item.onSelect?.() : undefined}
              >
                {item.label}
              </MenubarItem>
            ))}
            <MenubarSeparator />
            <MenubarItem
              data-testid='menu-file-close-project'
              className='text-[13px]'
              disabled={!hasScene}
              onSelect={() => void closeProject()}
            >
              {t('menu.closeProject')}
              <MenubarShortcut>
                {formatShortcutForDisplay(shortcuts.closeProject, isMac)}
              </MenubarShortcut>
            </MenubarItem>
            <MenubarSeparator />
            <MenubarItem
              className='text-[13px]'
              onSelect={() => {
                setSettingsTab('appearance')
                setSettingsOpen(true)
              }}
            >
              {t('menu.settings')}
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>
        <MenubarMenu>
          <MenubarTrigger
            data-testid='menu-edit-trigger'
            className='rounded px-3 py-1.5 font-medium hover:bg-accent data-[state=open]:bg-accent'
          >
            {t('menu.edit')}
          </MenubarTrigger>
          <MenubarContent className='min-w-40' align='start' sideOffset={5} alignOffset={-3}>
            <MenubarItem
              data-testid='menu-edit-undo'
              className='text-[13px]'
              disabled={!hasScene}
              onSelect={() => void undoOp()}
            >
              {t('menu.undo')}
              <MenubarShortcut>{formatShortcutForDisplay(shortcuts.undo, isMac)}</MenubarShortcut>
            </MenubarItem>
            <MenubarItem
              data-testid='menu-edit-redo'
              className='text-[13px]'
              disabled={!hasScene}
              onSelect={() => void redoOp()}
            >
              {t('menu.redo')}
              <MenubarShortcut>{formatShortcutForDisplay(shortcuts.redo, isMac)}</MenubarShortcut>
            </MenubarItem>
            <MenubarSeparator />
            <MenubarItem
              data-testid='menu-edit-select-all'
              className='text-[13px]'
              disabled={!hasPage}
              onSelect={() => selectAllTextNodesOnCurrentPage()}
            >
              {t('menu.selectAll')}
              <MenubarShortcut>{isMac ? '⌘A' : 'Ctrl+A'}</MenubarShortcut>
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>
        <MenubarMenu>
          <MenubarTrigger className='rounded px-3 py-1.5 font-medium hover:bg-accent data-[state=open]:bg-accent'>
            {t('menu.view')}
          </MenubarTrigger>
          <MenubarContent className='min-w-36' align='start' sideOffset={5} alignOffset={-3}>
            <MenubarItem className='text-[13px]' onSelect={fitCanvasToViewport}>
              {t('menu.fitWindow')}
            </MenubarItem>
            <MenubarItem className='text-[13px]' onSelect={resetCanvasScale}>
              {t('menu.originalSize')}
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>

        <MenubarMenu>
          <MenubarTrigger
            data-testid='menu-process-trigger'
            className='rounded px-3 py-1.5 font-medium hover:bg-accent data-[state=open]:bg-accent'
          >
            {t('menu.process')}
          </MenubarTrigger>
          <MenubarContent className='min-w-48' align='start' sideOffset={5} alignOffset={-3}>
            <MenubarItem
              data-testid='menu-process-current'
              className='text-[13px]'
              disabled={!hasPage}
              onSelect={() => void runPipeline({ pageId: requirePageId() })}
            >
              {t('menu.processCurrent')}
            </MenubarItem>
            <MenubarItem
              data-testid='menu-process-rerender'
              className='text-[13px]'
              disabled={!hasPage}
              onSelect={() => void runInpaint(requirePageId())}
            >
              {t('menu.redoInpaintRender')}
            </MenubarItem>
            <MenubarItem
              data-testid='menu-process-all'
              className='text-[13px]'
              disabled={!hasScene}
              onSelect={() => void runPipeline({})}
            >
              {t('menu.processAll')}
            </MenubarItem>
            <MenubarItem
              data-testid='menu-process-all-export-rendered'
              className='text-[13px]'
              disabled={!hasScene || processExporting}
              onSelect={() => void runPipelineAndExportRendered()}
            >
              {processExporting
                ? t('menu.processingAllAndExporting', 'Processing and exporting...')
                : t('menu.processAllAndExportRendered', 'Process all images + export rendered...')}
            </MenubarItem>
            <MenubarSeparator />
            <MenubarItem
              className='text-[13px]'
              disabled={!hasPage || !hasSelectedSteps}
              onSelect={() => void runCustomPipeline({ pageId: requirePageId() })}
            >
              {t('menu.runCustomCurrent')}
            </MenubarItem>
            <MenubarItem
              className='text-[13px]'
              disabled={!hasScene || !hasSelectedSteps}
              onSelect={() => void runCustomPipeline({})}
            >
              {t('menu.runCustomAll')}
            </MenubarItem>
            <MenubarSub>
              <MenubarSubTrigger className='text-[13px]'>
                {t('menu.customPipeline')}
              </MenubarSubTrigger>
              <MenubarSubContent className='min-w-48'>
                <MenubarCheckboxItem
                  className='text-[13px]'
                  checked={customPipeline.detect}
                  onCheckedChange={(checked) => setCustomPipeline({ detect: checked })}
                  onSelect={(e) => e.preventDefault()}
                >
                  {t('processing.detect')}
                </MenubarCheckboxItem>
                <MenubarCheckboxItem
                  className='text-[13px]'
                  checked={customPipeline.ocr}
                  onCheckedChange={(checked) => setCustomPipeline({ ocr: checked })}
                  onSelect={(e) => e.preventDefault()}
                >
                  {t('processing.ocr')}
                </MenubarCheckboxItem>
                <MenubarCheckboxItem
                  className='text-[13px]'
                  checked={customPipeline.translator}
                  onCheckedChange={(checked) => setCustomPipeline({ translator: checked })}
                  onSelect={(e) => e.preventDefault()}
                >
                  {t('llm.generate')}
                </MenubarCheckboxItem>
                <MenubarCheckboxItem
                  className='text-[13px]'
                  checked={customPipeline.inpainter}
                  onCheckedChange={(checked) => setCustomPipeline({ inpainter: checked })}
                  onSelect={(e) => e.preventDefault()}
                >
                  {t('mask.inpaint')}
                </MenubarCheckboxItem>
                <MenubarCheckboxItem
                  className='text-[13px]'
                  checked={customPipeline.renderer}
                  onCheckedChange={(checked) => setCustomPipeline({ renderer: checked })}
                  onSelect={(e) => e.preventDefault()}
                >
                  {t('llm.render')}
                </MenubarCheckboxItem>
              </MenubarSubContent>
            </MenubarSub>
            <MenubarSub>
              <MenubarSubTrigger className='text-[13px]' data-testid='menu-ocr-language'>
                {t('menu.ocrLanguage')}
              </MenubarSubTrigger>
              <MenubarSubContent className='min-w-40'>
                {/* The value is the English language name — it goes verbatim
                    into the OCR prompt as a hint ("The text in the image is
                    Korean."). Auto keeps the model's own script detection. */}
                <MenubarRadioGroup
                  value={ocrLanguage ?? 'auto'}
                  onValueChange={(value) => setOcrLanguage(value === 'auto' ? undefined : value)}
                >
                  <MenubarRadioItem
                    value='auto'
                    className='text-[13px]'
                    onSelect={(e) => e.preventDefault()}
                  >
                    {t('menu.ocrLanguageAuto')}
                  </MenubarRadioItem>
                  {(
                    [
                      ['Korean', 'ko-KR'],
                      ['Japanese', 'ja-JP'],
                      ['Chinese', 'zh-CN'],
                      ['English', 'en-US'],
                    ] as const
                  ).map(([value, localeKey]) => (
                    <MenubarRadioItem
                      key={value}
                      value={value}
                      className='text-[13px]'
                      onSelect={(e) => e.preventDefault()}
                    >
                      {t(`llm.languages.${localeKey}`, { defaultValue: value })}
                    </MenubarRadioItem>
                  ))}
                </MenubarRadioGroup>
              </MenubarSubContent>
            </MenubarSub>
          </MenubarContent>
        </MenubarMenu>
        <MenubarMenu>
          <MenubarTrigger className='rounded px-3 py-1.5 font-medium hover:bg-accent data-[state=open]:bg-accent'>
            {t('menu.help')}
          </MenubarTrigger>
          <MenubarContent className='min-w-36' align='start' sideOffset={5} alignOffset={-3}>
            {helpMenuItems.map((item) => (
              <MenubarItem
                key={item.label}
                className='text-[13px]'
                disabled={item.disabled}
                onSelect={item.onSelect ? () => void item.onSelect?.() : undefined}
              >
                {item.label}
              </MenubarItem>
            ))}
            <MenubarSeparator />
            <MenubarItem
              className='text-[13px]'
              onSelect={() => {
                setSettingsTab('about')
                setSettingsOpen(true)
              }}
            >
              {t('settings.about')}
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>
      </Menubar>
      <div data-tauri-drag-region className='flex h-full flex-1 items-center justify-center' />
      {isWindowsTauri && <WindowControls />}
      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} defaultTab={settingsTab} />
    </div>
  )
}

function MacOSControls() {
  return (
    <div className='flex h-full items-center gap-2 pr-2 pl-4'>
      <button
        onClick={() => void windowControls.close()}
        className='group flex size-3 items-center justify-center rounded-full bg-[#FF5F57] active:bg-[#bf4942]'
      >
        <XIcon
          className='size-2 text-[#4a0002] opacity-0 group-hover:opacity-100'
          strokeWidth={3}
        />
      </button>
      <button
        onClick={() => void windowControls.minimize()}
        className='group flex size-3 items-center justify-center rounded-full bg-[#FEBC2E] active:bg-[#bf8d22]'
      >
        <MinusIcon
          className='size-2 text-[#5f4a00] opacity-0 group-hover:opacity-100'
          strokeWidth={3}
        />
      </button>
      <button
        onClick={() => void windowControls.toggleMaximize()}
        className='group flex size-3 items-center justify-center rounded-full bg-[#28C840] active:bg-[#1e9630]'
      >
        <SquareIcon
          className='size-1.5 text-[#006500] opacity-0 group-hover:opacity-100'
          strokeWidth={3}
        />
      </button>
    </div>
  )
}

function WindowControls() {
  const [maximized, setMaximized] = useState(false)

  const updateMaximized = useCallback(async () => {
    setMaximized(await windowControls.isMaximized())
  }, [])

  useEffect(() => {
    void updateMaximized()
    const onResize = () => void updateMaximized()
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [updateMaximized])

  return (
    <div className='flex h-full'>
      <button
        onClick={() => void windowControls.minimize()}
        className='flex h-full w-11 items-center justify-center hover:bg-accent'
      >
        <MinusIcon className='size-4' />
      </button>
      <button
        onClick={() => {
          void windowControls.toggleMaximize().then(updateMaximized)
        }}
        className='flex h-full w-11 items-center justify-center hover:bg-accent'
      >
        {maximized ? <CopyIcon className='size-3.5' /> : <SquareIcon className='size-3.5' />}
      </button>
      <button
        onClick={() => void windowControls.close()}
        className='flex h-full w-11 items-center justify-center hover:bg-red-500 hover:text-white'
      >
        <XIcon className='size-4' />
      </button>
    </div>
  )
}
