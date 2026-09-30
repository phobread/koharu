'use client'

import type { TFunction } from 'i18next'
import { CopyIcon, MinusIcon, SquareIcon, XIcon } from 'lucide-react'
import Image from 'next/image'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { fitCanvasToViewport, resetCanvasScale } from '@/components/Canvas'
import { ProjectTitle } from '@/components/ProjectTitle'
import { SettingsDialog, type TabId } from '@/components/SettingsDialog'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import {
  MenubarCheckboxItem,
  Menubar,
  MenubarContent,
  MenubarItem,
  MenubarLabel,
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
import { getConfig, startPipeline } from '@/lib/api/default/default'
import { isTauri, openExternalUrl } from '@/lib/backend'
import {
  addOfficialRelease,
  exportCurrentProjectAs,
  exportPageImages,
  importPages,
  pickOfficialRelease,
} from '@/lib/io/pagesIo'
import { orderedPageIds, processPagesWithFeedback } from '@/lib/io/processPages'
import { renderDefaultsForPipeline } from '@/lib/io/renderDefaults'
import {
  awaitPendingSceneEdits,
  closeProject,
  redoOp,
  selectAllTextNodesOnCurrentPage,
  undoOp,
} from '@/lib/io/scene'
import type { ExportSummary } from '@/lib/pageStatus'
import { formatShortcutForDisplay, getPlatform } from '@/lib/shortcutUtils'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useJobsStore } from '@/lib/stores/jobsStore'
import { type ProcessSteps, usePreferencesStore } from '@/lib/stores/preferencesStore'
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

type MenuItem = {
  label: string
  onSelect?: () => void | Promise<void>
  disabled?: boolean
  testId?: string
}

const PROCESS_STEP_ITEMS: {
  key: keyof ProcessSteps
  labelKey: string
  fallback: string
}[] = [
  { key: 'detect', labelKey: 'processing.detect', fallback: 'Detect' },
  { key: 'ocr', labelKey: 'processing.ocr', fallback: 'OCR' },
  { key: 'translate', labelKey: 'llm.translate', fallback: 'Translate' },
  { key: 'inpaint', labelKey: 'mask.inpaint', fallback: 'Inpaint' },
  { key: 'render', labelKey: 'llm.render', fallback: 'Render' },
]

export function MenuBar() {
  const { t } = useTranslation()
  const settingsTab = useEditorUiStore((s) => s.settingsTab)
  const openSettings = useEditorUiStore((s) => s.openSettings)
  const closeSettings = useEditorUiStore((s) => s.closeSettings)
  const hasPage = useSelectionStore((s) => s.pageId !== null)
  const { scene } = useScene()
  const hasScene = scene !== null
  const shortcuts = usePreferencesStore((state) => state.shortcuts)
  const processStepsPref = usePreferencesStore((state) => state.processSteps)
  const isProcessing = useJobsStore((state) =>
    Object.values(state.jobs).some((job) => job.status === 'running'),
  )
  const ocrLanguage = usePreferencesStore((state) => state.ocrLanguage)
  const setOcrLanguage = usePreferencesStore((state) => state.setOcrLanguage)
  const setProcessSteps = usePreferencesStore((state) => state.setProcessSteps)
  const anyStepTicked = Object.values(processStepsPref).some(Boolean)
  const isMac = useMemo(() => getPlatform() === 'mac', [])
  const selectedPageIds = useSelectionStore((s) => s.selectedPageIds)
  const selectedPages = useMemo(
    () => orderedPageIds(scene?.pages, selectedPageIds),
    [scene?.pages, selectedPageIds],
  )
  const [redoAllConfirmOpen, setRedoAllConfirmOpen] = useState(false)
  // Controlled so "Export all pages", a submenu trigger that also acts on
  // click, can close the menu.
  const [openMenu, setOpenMenu] = useState('')

  const requirePageId = () => {
    const id = useSelectionStore.getState().pageId
    if (!id) throw new Error('No current page selected')
    return id
  }

  const startProcess = (opts: { pages?: string[]; onlyMissing: boolean }) =>
    processPagesWithFeedback(
      opts,
      t('process.nothingToDo', 'Nothing to process: those pages already have every step.'),
    )

  const rebuildMasksAndInpaint = async (pageId?: string) => {
    try {
      await awaitPendingSceneEdits()
      const cfg = await getConfig()
      const p = cfg.pipeline
      if (!p?.segmenter || !p.inpainter) return
      await startPipeline({
        steps: [p.segmenter, p.bubble_segmenter, p.inpainter, p.renderer].filter(
          (s): s is string => !!s,
        ),
        pages: pageId ? [pageId] : undefined,
        ...renderDefaultsForPipeline(),
      })
    } catch (err) {
      useEditorUiStore.getState().showError(String(err))
    }
  }

  const addOfficialPages = async () => {
    try {
      const paths = await pickOfficialRelease()
      if (paths.length > 0) await addOfficialRelease(paths, t)
    } catch (err) {
      useEditorUiStore.getState().showError(err instanceof Error ? err.message : String(err))
    }
  }

  const exportImages = async (pages?: string[]) => {
    const summary = await exportPageImages(pages)
    if (summary) useEditorUiStore.getState().showNotice(exportNotice(summary, t))
  }

  const exportAllPages = () => {
    if (!hasScene) return
    setOpenMenu('')
    void exportImages()
  }

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
      {hasScene && (
        <div className='flex h-full max-w-56 min-w-0 items-center border-r border-border pr-2 pl-1'>
          <ProjectTitle />
        </div>
      )}
      <Menubar
        value={openMenu}
        onValueChange={setOpenMenu}
        className='h-auto gap-1 border-none bg-transparent p-0 px-1.5 shadow-none'
      >
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
            <MenubarItem
              data-testid='menu-file-add-official'
              className='text-[13px]'
              disabled={!hasScene || !isTauri()}
              title={t(
                'menu.addOfficialHint',
                "Pick the folder of this chapter's official release. Each page gets its matching release page, and cleanup keeps the release's onomatopoeia wherever you have no text.",
              )}
              onSelect={() => void addOfficialPages()}
            >
              {t('menu.addOfficial', 'Add Official Release...')}
            </MenubarItem>
            <MenubarSeparator />
            {/* Click exports every page as far as it got; hover (or the right
                arrow key) opens the other kinds of export. */}
            <MenubarSub>
              <MenubarSubTrigger
                data-testid='menu-file-export'
                className='text-[13px] data-[disabled]:pointer-events-none data-[disabled]:opacity-50'
                disabled={!hasScene}
                onClick={(e) => {
                  e.preventDefault()
                  exportAllPages()
                }}
                onKeyDown={(e) => {
                  if (e.key !== 'Enter' && e.key !== ' ') return
                  e.preventDefault()
                  exportAllPages()
                }}
              >
                {t('menu.exportAllPages', 'Export all pages')}
              </MenubarSubTrigger>
              <MenubarSubContent className='min-w-52'>
                <p className='max-w-64 px-2 py-1.5 text-xs text-muted-foreground'>
                  {t(
                    'menu.exportAllPagesHint',
                    'Each page as far as it got: translated, else cleaned, else the original.',
                  )}
                </p>
                <MenubarItem
                  data-testid='menu-export-page'
                  className='text-[13px]'
                  disabled={!hasPage}
                  onSelect={() => void exportImages([requirePageId()])}
                >
                  {t('menu.exportThisPage', 'This page')}
                </MenubarItem>
                <MenubarItem
                  data-testid='menu-export-psd'
                  className='text-[13px]'
                  disabled={!hasPage}
                  onSelect={() => void exportCurrentProjectAs('psd', [requirePageId()])}
                >
                  {t('menu.exportThisPagePsd', 'This page as PSD')}
                </MenubarItem>
                <MenubarItem
                  data-testid='menu-export-cleaned'
                  className='text-[13px]'
                  onSelect={() => void exportCurrentProjectAs('inpainted')}
                >
                  {t('menu.exportAllCleaned', 'All pages, cleaned (no text)')}
                </MenubarItem>
                <MenubarSeparator />
                <MenubarItem
                  data-testid='menu-export-khr'
                  className='text-[13px]'
                  title={t(
                    'menu.exportKhrHint',
                    'The whole project in one file, to back it up or open it on another computer.',
                  )}
                  onSelect={() => void exportCurrentProjectAs('khr')}
                >
                  {t('menu.exportKhr', 'Project archive (.khr)')}
                </MenubarItem>
              </MenubarSubContent>
            </MenubarSub>
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
                openSettings('appearance')
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
            {/* Ticked steps apply to every Process action; each runs only where
                it's still missing, so re-ticking Translate + Render later
                finishes pages without redoing detection, OCR or cleanup. */}
            <MenubarLabel className='py-1 text-[11px] font-normal text-muted-foreground'>
              {t('menu.processStepsLabel', 'Steps to run · only what is missing')}
            </MenubarLabel>
            {PROCESS_STEP_ITEMS.map(({ key, labelKey, fallback }) => (
              <MenubarCheckboxItem
                key={key}
                data-testid={`menu-process-step-${key}`}
                className='text-[13px]'
                checked={processStepsPref[key]}
                onCheckedChange={(checked) => setProcessSteps({ [key]: checked })}
                onSelect={(e) => e.preventDefault()}
              >
                {t(labelKey, fallback)}
              </MenubarCheckboxItem>
            ))}
            <MenubarSeparator />
            <MenubarItem
              data-testid='menu-process-current'
              className='text-[13px]'
              disabled={!hasPage || !anyStepTicked}
              title={t('menu.processMissingHint')}
              onSelect={() => void startProcess({ pages: [requirePageId()], onlyMissing: true })}
            >
              {t('menu.processCurrent')}
            </MenubarItem>
            <MenubarItem
              data-testid='menu-process-selected'
              className='text-[13px]'
              disabled={selectedPages.length < 2 || !anyStepTicked}
              title={t('menu.processMissingHint')}
              onSelect={() => void startProcess({ pages: selectedPages, onlyMissing: true })}
            >
              {selectedPages.length < 2
                ? t('menu.processSelectedNone', 'Process selected pages')
                : t('menu.processSelected', {
                    count: selectedPages.length,
                    defaultValue: 'Process {{count}} selected pages',
                  })}
            </MenubarItem>
            <MenubarItem
              data-testid='menu-process-all'
              className='text-[13px]'
              disabled={!hasScene || !anyStepTicked}
              title={t('menu.processMissingHint')}
              onSelect={() => void startProcess({ onlyMissing: true })}
            >
              {t('menu.processUnfinished', 'Process unfinished pages')}
            </MenubarItem>
            <MenubarSeparator />
            <MenubarItem
              data-testid='menu-process-redo-all'
              className='text-[13px]'
              disabled={!hasScene || !anyStepTicked}
              onSelect={() => setRedoAllConfirmOpen(true)}
            >
              {t('menu.redoTicked', 'Redo ticked steps on all pages…')}
            </MenubarItem>
            <MenubarSeparator />
            {/* The one mask job the ticks can't do: re-make the text masks
                from the kept boxes (after a mask-detection improvement) and
                clean again. Detect would replace the boxes instead. */}
            <MenubarSub>
              <MenubarSubTrigger
                data-testid='menu-rebuild-masks'
                disabled={!hasScene || isProcessing}
                title={t('menu.rebuildMasksHint')}
                className='text-[13px]'
              >
                {t('menu.rebuildMasks')}
              </MenubarSubTrigger>
              <MenubarSubContent>
                <MenubarItem
                  data-testid='menu-rebuild-mask-current'
                  disabled={!hasPage || isProcessing}
                  onSelect={() => void rebuildMasksAndInpaint(requirePageId())}
                >
                  {t('menu.currentImage')}
                </MenubarItem>
                <MenubarItem
                  data-testid='menu-rebuild-mask-all'
                  disabled={!hasScene || isProcessing}
                  onSelect={() => void rebuildMasksAndInpaint()}
                >
                  {t('menu.allImages')}
                </MenubarItem>
                <p className='max-w-64 px-2 py-1.5 text-xs text-muted-foreground'>
                  {t('menu.rebuildMasksHint')}
                </p>
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
                openSettings('about')
              }}
            >
              {t('settings.about')}
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>
      </Menubar>
      <div data-tauri-drag-region className='flex h-full flex-1 items-center justify-center' />
      {isWindowsTauri && <WindowControls />}
      <SettingsDialog
        open={settingsTab !== null}
        onOpenChange={(open) => {
          if (!open) closeSettings()
        }}
        defaultTab={(settingsTab ?? 'appearance') as TabId}
      />
      <AlertDialog open={redoAllConfirmOpen} onOpenChange={setRedoAllConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogTitle>
            {t('menu.redoTickedTitle', 'Redo the ticked steps on all pages?')}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t('menu.redoTickedDescription', {
              steps: PROCESS_STEP_ITEMS.filter(({ key }) => processStepsPref[key])
                .map(({ labelKey, fallback }) => t(labelKey, fallback))
                .join(', '),
              defaultValue:
                'These steps run again on every page and replace what they made before: {{steps}}.',
            })}{' '}
            {processStepsPref.detect ? t('menu.redoDetectWarning') : null}
          </AlertDialogDescription>
          <div className='flex justify-end gap-2'>
            <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              data-testid='redo-all-confirm'
              onClick={() => void startProcess({ onlyMissing: false })}
            >
              {t('menu.redoTickedConfirm', 'Redo')}
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

/** The notice after an image export: how many pages, and which went out
 * without their translation. */
export function exportNotice(summary: ExportSummary, t: TFunction): string {
  const pages = (numbers: number[]) =>
    numbers.length > 10
      ? t('menu.exportPagesMore', {
          pages: numbers.slice(0, 10).join(', '),
          more: numbers.length - 10,
          defaultValue: '{{pages}} and {{more}} more',
        })
      : numbers.join(', ')
  const parts = [
    t('menu.exportDone', { count: summary.count, defaultValue: 'Exported {{count}} pages.' }),
  ]
  if (summary.unrendered.length > 0)
    parts.push(
      t('menu.exportUnrendered', {
        count: summary.unrendered.length,
        pages: pages(summary.unrendered),
        defaultValue: 'Not rendered yet, so without the translation: pages {{pages}}.',
      }),
    )
  if (summary.cleaned.length > 0)
    parts.push(
      t('menu.exportCleaned', {
        count: summary.cleaned.length,
        pages: pages(summary.cleaned),
        defaultValue: 'No translation yet, exported cleaned: pages {{pages}}.',
      }),
    )
  if (summary.original.length > 0)
    parts.push(
      t('menu.exportOriginal', {
        count: summary.original.length,
        pages: pages(summary.original),
        defaultValue: 'Not cleaned yet, exported as the original: pages {{pages}}.',
      }),
    )
  return parts.join(' ')
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
