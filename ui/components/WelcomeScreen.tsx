'use client'

import {
  AlertCircleIcon,
  ArrowRightIcon,
  ClockIcon,
  FileArchiveIcon,
  ImageIcon,
  PlusIcon,
  TrashIcon,
  XIcon,
} from 'lucide-react'
import Image from 'next/image'
import { useCallback, useMemo, useState } from 'react'
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  getGetProjectThumbnailUrl,
  useDeleteProject,
  useListProjects,
} from '@/lib/api/default/default'
import type { ProjectSummary } from '@/lib/api/schemas'
import { importKhrFile } from '@/lib/io/pagesIo'
import { createAndOpenProject, switchProject } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { cn } from '@/lib/utils'

type Busy = false | 'new' | 'open' | 'import'

/**
 * Project-management / welcome screen. Rendered when no project is open.
 * Server manages all project paths under `{data.path}/projects/` — clients
 * only pass `id`. Same UX in Tauri and headless browser deployments.
 */
export function WelcomeScreen() {
  const { t } = useTranslation()
  const { data: projectsData, refetch: refetchProjects } = useListProjects()
  const projects = useMemo(() => {
    const all = projectsData?.projects ?? []
    return [...all].sort((a, b) => (b.updatedAtMs ?? 0) - (a.updatedAtMs ?? 0))
  }, [projectsData])
  const lastProjectId = useEditorUiStore((s) => s.lastProjectId)

  const [busy, setBusy] = useState<Busy | 'delete'>(false)
  const [error, setError] = useState<string | null>(null)
  const [newDialogOpen, setNewDialogOpen] = useState(false)
  const [projectToDelete, setProjectToDelete] = useState<ProjectSummary | null>(null)

  const deleteProjectMutation = useDeleteProject()

  const openById = useCallback(async (id: string) => {
    setError(null)
    setBusy('open')
    try {
      await switchProject({ id })
    } catch (e) {
      setError(`Open failed: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(false)
    }
  }, [])

  const onDeleteConfirm = useCallback(async () => {
    if (!projectToDelete) return
    setError(null)
    setBusy('delete')
    try {
      await deleteProjectMutation.mutateAsync({ id: projectToDelete.id })
      await refetchProjects()
      setProjectToDelete(null)
    } catch (e) {
      setError(`Delete failed: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(false)
    }
  }, [projectToDelete, deleteProjectMutation, refetchProjects])

  const onCreate = useCallback(async (name: string) => {
    setError(null)
    setBusy('new')
    try {
      await createAndOpenProject({ name })
    } catch (e) {
      setError(`New failed: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(false)
      setNewDialogOpen(false)
    }
  }, [])

  const importKhr = useCallback(async () => {
    setError(null)
    setBusy('import')
    try {
      await importKhrFile()
      await refetchProjects()
    } catch (e) {
      setError(`Import failed: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(false)
    }
  }, [refetchProjects])

  return (
    <div className='relative flex min-h-0 flex-1 overflow-hidden bg-background'>
      <div
        aria-hidden
        className='pointer-events-none absolute -top-40 left-1/2 h-80 w-[720px] -translate-x-1/2 rounded-full bg-primary/10 blur-3xl'
      />

      <ScrollArea className='relative z-10 min-h-0 flex-1'>
        <div className='mx-auto flex w-full max-w-5xl flex-col gap-8 px-8 pt-14 pb-12'>
          <header className='flex flex-wrap items-center gap-x-4 gap-y-3'>
            <Image src='/icon.png' alt='Koharu' width={44} height={44} priority />
            <div className='flex min-w-0 flex-1 flex-col gap-0.5'>
              <h1 className='text-2xl font-semibold tracking-tight text-foreground'>
                {t('welcome.title')}
              </h1>
              <p className='text-xs text-muted-foreground'>{t('welcome.subtitle')}</p>
            </div>
            <Button
              variant='ghost'
              onClick={() => void importKhr()}
              disabled={!!busy}
              className='text-muted-foreground'
            >
              <FileArchiveIcon className='size-4' />
              {t('welcome.importKhr')}
            </Button>
            <Button onClick={() => setNewDialogOpen(true)} disabled={!!busy}>
              <PlusIcon className='size-4' />
              {t('welcome.new')}
            </Button>
          </header>

          {error && (
            <div className='flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive'>
              <AlertCircleIcon className='mt-0.5 h-3.5 w-3.5 shrink-0' />
              <div className='flex-1'>{error}</div>
              <button
                type='button'
                onClick={() => setError(null)}
                className='cursor-pointer text-destructive/70 hover:text-destructive'
                aria-label='dismiss'
              >
                <XIcon className='h-3.5 w-3.5' />
              </button>
            </div>
          )}

          <section className='flex flex-col gap-3'>
            <div className='flex items-baseline justify-between px-0.5'>
              <h2 className='text-[10px] font-semibold tracking-[0.14em] text-muted-foreground uppercase'>
                {t('welcome.projects')}
              </h2>
              {projects.length > 0 && (
                <span className='text-[10px] text-muted-foreground tabular-nums'>
                  {projects.length}
                </span>
              )}
            </div>
            {projects.length > 0 ? (
              <ul className='grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-x-4 gap-y-5'>
                {projects.map((p) => (
                  <ProjectCard
                    key={p.id}
                    project={p}
                    last={p.id === lastProjectId}
                    onOpen={openById}
                    onDeleteRequest={setProjectToDelete}
                    disabled={!!busy}
                  />
                ))}
              </ul>
            ) : (
              <EmptyProjects />
            )}
          </section>
        </div>
      </ScrollArea>

      <NewProjectDialog
        open={newDialogOpen}
        onOpenChange={setNewDialogOpen}
        onSubmit={onCreate}
        busy={busy === 'new'}
      />

      <AlertDialog
        open={!!projectToDelete}
        onOpenChange={(open) => !open && setProjectToDelete(null)}
      >
        <AlertDialogContent>
          <div className='flex flex-col gap-1.5 text-center sm:text-left'>
            <AlertDialogTitle>{t('welcome.deleteConfirmTitle')}</AlertDialogTitle>
            <AlertDialogDescription>
              {t('welcome.deleteConfirmDescription', { name: projectToDelete?.name })}
            </AlertDialogDescription>
          </div>
          <div className='flex flex-col-reverse gap-2 sm:flex-row sm:justify-end'>
            <AlertDialogCancel disabled={busy === 'delete'}>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault()
                void onDeleteConfirm()
              }}
              disabled={busy === 'delete'}
              className='text-destructive-foreground bg-destructive hover:bg-destructive/90'
            >
              {busy === 'delete' ? t('welcome.deleting') : t('welcome.delete')}
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

// ---------------------------------------------------------------------------

function EmptyProjects() {
  const { t } = useTranslation()
  return (
    <div className='flex h-48 items-center justify-center rounded-lg border border-dashed border-border/60 bg-card/20'>
      <p className='text-center text-[11px] text-muted-foreground'>{t('welcome.emptyHint')}</p>
    </div>
  )
}

/** A project in the grid: its first page as the cover, name and last edit. */
function ProjectCard({
  project,
  last,
  onOpen,
  onDeleteRequest,
  disabled,
}: {
  project: ProjectSummary
  /** Opened last: the mouse forward button reopens it. */
  last: boolean
  onOpen: (id: string) => void
  onDeleteRequest: (project: ProjectSummary) => void
  disabled?: boolean
}) {
  const { t } = useTranslation()
  const [coverFailed, setCoverFailed] = useState(false)
  const when = project.updatedAtMs && project.updatedAtMs > 0 ? new Date(project.updatedAtMs) : null
  // The folder's modified time changes when the project is saved, so a new
  // first page gets a fresh request.
  const cover = `${getGetProjectThumbnailUrl(project.id)}?v=${project.updatedAtMs ?? 0}`
  const deleteLabel = t('welcome.deleteProject', {
    defaultValue: 'Delete {{name}}',
    name: project.name,
  })

  return (
    <li className='group relative'>
      <button
        type='button'
        data-testid={`welcome-project-${project.id}`}
        onClick={() => onOpen(project.id)}
        disabled={disabled}
        title={project.name}
        className='flex w-full cursor-pointer flex-col gap-2 rounded-lg text-left outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-60'
      >
        <div
          className={cn(
            'relative aspect-[3/4] w-full overflow-hidden rounded-lg border border-border/60 bg-muted/40 shadow-sm transition',
            'group-hover:-translate-y-0.5 group-hover:border-primary/50 group-hover:shadow-md',
            last && 'border-primary/40',
          )}
        >
          {coverFailed ? (
            <div className='flex h-full w-full flex-col items-center justify-center gap-2 text-muted-foreground'>
              <ImageIcon className='size-6' />
              <span className='text-[11px]'>{t('welcome.noPages', 'No pages yet')}</span>
            </div>
          ) : (
            <img
              src={cover}
              alt=''
              loading='lazy'
              draggable={false}
              onError={() => setCoverFailed(true)}
              className='h-full w-full object-cover object-top'
            />
          )}
          {last && (
            <span
              data-testid='welcome-last-opened'
              title={t('welcome.lastOpenedHint', 'The mouse forward button reopens it')}
              className='absolute bottom-2 left-2 flex items-center gap-1 rounded-full bg-background/85 px-2 py-0.5 text-[10px] font-medium text-foreground shadow-sm backdrop-blur'
            >
              <ArrowRightIcon className='size-3' />
              {t('welcome.lastOpened', 'Last opened')}
            </span>
          )}
        </div>
        <div className='flex min-w-0 flex-col gap-0.5 px-0.5'>
          <div className='truncate text-sm font-medium text-foreground'>{project.name}</div>
          {when && (
            <div className='flex items-center gap-1 text-[11px] text-muted-foreground'>
              <ClockIcon className='h-3 w-3' />
              {formatRelative(when)}
            </div>
          )}
        </div>
      </button>

      <Button
        data-testid={`welcome-delete-project-${project.id}`}
        variant='ghost'
        size='icon-xs'
        className='absolute top-2 right-2 h-7 w-7 bg-background/80 text-muted-foreground opacity-0 shadow-sm backdrop-blur group-hover:opacity-100 hover:bg-destructive/15 hover:text-destructive focus-visible:opacity-100'
        disabled={disabled}
        aria-label={deleteLabel}
        title={deleteLabel}
        onClick={() => onDeleteRequest(project)}
      >
        <TrashIcon className='h-3.5 w-3.5' />
      </Button>
    </li>
  )
}

function formatRelative(d: Date): string {
  const diff = Date.now() - d.getTime()
  const m = 60_000
  const h = 3_600_000
  const day = 86_400_000
  if (diff < m) return 'just now'
  if (diff < h) return `${Math.floor(diff / m)}m ago`
  if (diff < day) return `${Math.floor(diff / h)}h ago`
  if (diff < day * 30) return `${Math.floor(diff / day)}d ago`
  return d.toLocaleDateString()
}

// ---------------------------------------------------------------------------

function NewProjectDialog({
  open,
  onOpenChange,
  onSubmit,
  busy,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSubmit: (name: string) => void
  busy: boolean
}) {
  const { t } = useTranslation()
  const [name, setName] = useState('')

  const trimmed = name.trim()
  const canSubmit = trimmed.length > 0 && !busy

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        onOpenChange(o)
        if (!o) setName('')
      }}
    >
      <DialogContent className='sm:max-w-md'>
        <DialogHeader>
          <DialogTitle>{t('welcome.newDialogTitle')}</DialogTitle>
          <DialogDescription>{t('welcome.newDialogDescription')}</DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault()
            if (canSubmit) onSubmit(trimmed)
          }}
          className='flex flex-col gap-4'
        >
          <Input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t('welcome.newDialogPlaceholder')}
          />
          <DialogFooter>
            <Button type='button' variant='outline' onClick={() => onOpenChange(false)}>
              {t('common.cancel')}
            </Button>
            <Button type='submit' disabled={!canSubmit}>
              <PlusIcon className='h-3.5 w-3.5' />
              {t('welcome.newDialogSubmit')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
