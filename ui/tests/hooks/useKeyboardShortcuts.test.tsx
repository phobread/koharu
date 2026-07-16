import { fireEvent, renderHook, waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { useKeyboardShortcuts } from '@/hooks/useKeyboardShortcuts'
import { getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import type { Node, Page, SceneSnapshot } from '@/lib/api/schemas'
import { queryClient } from '@/lib/queryClient'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { server } from '../msw/server'

function textNode(id: string): Node {
  return {
    id,
    transform: { x: 0, y: 0, width: 10, height: 10, rotationDeg: 0 },
    visible: true,
    kind: { text: { raw: `t-${id}` } },
  } as unknown as Node
}

function seedScene(): SceneSnapshot {
  const page: Page = {
    id: 'p-1',
    name: 'P',
    width: 10,
    height: 10,
    nodes: { t1: textNode('t1'), t2: textNode('t2') },
  } as unknown as Page
  return {
    epoch: 1,
    scene: { pages: { 'p-1': page }, project: { name: 'P' } as never } as never,
  }
}

describe('useKeyboardShortcuts — Ctrl+A', () => {
  beforeEach(() => {
    useSelectionStore.getState().setPage(null)
    queryClient.clear()
  })

  it('Ctrl+A selects every text node on the active page', () => {
    queryClient.setQueryData(getGetSceneJsonQueryKey(), seedScene())
    useSelectionStore.getState().setPage('p-1')
    renderHook(() => useKeyboardShortcuts())

    fireEvent.keyDown(window, { key: 'a', ctrlKey: true })

    expect([...useSelectionStore.getState().nodeIds].sort()).toEqual(['t1', 't2'])
  })

  it('Ctrl+A is a no-op while typing inside a textarea', () => {
    queryClient.setQueryData(getGetSceneJsonQueryKey(), seedScene())
    useSelectionStore.getState().setPage('p-1')
    renderHook(() => useKeyboardShortcuts())

    const textarea = document.createElement('textarea')
    document.body.appendChild(textarea)
    textarea.focus()

    fireEvent.keyDown(textarea, { key: 'a', ctrlKey: true })

    expect(useSelectionStore.getState().nodeIds.size).toBe(0)

    document.body.removeChild(textarea)
  })
})

describe('useKeyboardShortcuts — Ctrl+W', () => {
  beforeEach(() => {
    queryClient.clear()
  })

  it('closes the current project when a scene is open', async () => {
    let deleted = 0
    server.use(
      http.delete('/api/v1/projects/current', () => {
        deleted += 1
        return new HttpResponse(null, { status: 204 })
      }),
    )
    queryClient.setQueryData(getGetSceneJsonQueryKey(), seedScene())
    renderHook(() => useKeyboardShortcuts())

    const event = new KeyboardEvent('keydown', {
      key: 'w',
      ctrlKey: true,
      cancelable: true,
    })
    fireEvent(window, event)

    expect(event.defaultPrevented).toBe(true)
    await waitFor(() => expect(deleted).toBe(1))
  })

  it('prevents the browser accelerator without a project open', () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch')
    renderHook(() => useKeyboardShortcuts())

    const event = new KeyboardEvent('keydown', {
      key: 'w',
      ctrlKey: true,
      cancelable: true,
    })
    fireEvent(window, event)

    expect(event.defaultPrevented).toBe(true)
    expect(fetchSpy).not.toHaveBeenCalled()
  })
})
