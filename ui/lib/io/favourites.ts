'use client'

import { useCallback, useMemo } from 'react'

import {
  getGetCurrentFavouritesQueryKey,
  getListProjectsQueryKey,
  putPageFavourite,
  putProjectFavourite,
  useGetCurrentFavourites,
} from '@/lib/api/default/default'
import type { ListProjectsResponse, ProjectFavourites } from '@/lib/api/schemas'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

/**
 * Favourites: a star on pages and on projects. The backend keeps them in a
 * side file in the project folder, so they are not scene edits: no undo, no
 * scene refetch. A click shows at once and is put back if saving fails.
 */

const failed = (e: unknown) =>
  useEditorUiStore.getState().showError(e instanceof Error ? e.message : String(e))

/** The open project's starred pages, and the star toggle for a page. */
export function usePageFavourites(): {
  starred: ReadonlySet<string>
  setStarred: (pageId: string, starred: boolean) => void
} {
  // Keyed by project, so one project's stars are never shown for another.
  const projectId = useEditorUiStore((s) => s.lastProjectId) ?? null
  const queryKey = useMemo(
    () => [...getGetCurrentFavouritesQueryKey(), projectId] as const,
    [projectId],
  )
  const { data } = useGetCurrentFavourites({ query: { queryKey, retry: false } })
  const starred = useMemo(() => new Set(data?.pages ?? []), [data])

  const setStarred = useCallback(
    (pageId: string, on: boolean) => {
      queryClient.setQueryData<ProjectFavourites>(queryKey, (old) => {
        const pages = (old?.pages ?? []).filter((id) => id !== pageId)
        return { project: old?.project ?? false, pages: on ? [...pages, pageId] : pages }
      })
      putPageFavourite(pageId, { favourite: on })
        .then((saved) => queryClient.setQueryData<ProjectFavourites>(queryKey, saved))
        .catch((e) => {
          failed(e)
          void queryClient.invalidateQueries({ queryKey })
        })
    },
    [queryKey],
  )

  return { starred, setStarred }
}

/** Star or unstar a project in the project list. */
export function setProjectStarred(projectId: string, on: boolean): void {
  const queryKey = getListProjectsQueryKey()
  queryClient.setQueryData<ListProjectsResponse>(queryKey, (old) => {
    if (!old) return old
    const others = (old.favourites ?? []).filter((id) => id !== projectId)
    return { ...old, favourites: on ? [...others, projectId] : others }
  })
  putProjectFavourite(projectId, { favourite: on }).catch((e) => {
    failed(e)
    void queryClient.invalidateQueries({ queryKey })
  })
}
