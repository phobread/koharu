import { act, screen } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { createElement } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { getGetSceneJsonQueryKey } from '@/lib/api/default/default'
import { queryClient } from '@/lib/queryClient'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

// The canvas only: which page picture it shows in the Release view. jsdom
// paints nothing, so each picture renders as a marker naming its blob.
const blobs = vi.hoisted(() => ({ cache: new Map<string, Uint8Array>(), asked: new Set<string>() }))
vi.mock('@/hooks/useBlobData', () => ({
  useBlobData: (hash?: string) => {
    if (!hash) return undefined
    blobs.asked.add(hash)
    if (!blobs.cache.has(hash)) blobs.cache.set(hash, new TextEncoder().encode(hash))
    return blobs.cache.get(hash)
  },
}))
vi.mock('@/components/Image', () => ({
  Image: ({
    data,
    visible = true,
    'data-testid': testId,
  }: {
    data?: Uint8Array
    visible?: boolean
    'data-testid'?: string
  }) =>
    data && visible
      ? createElement('img', { 'data-testid': testId, 'data-blob': new TextDecoder().decode(data) })
      : null,
}))
vi.mock('@/components/canvas/TextBlockLayer', () => ({
  TextBlockLayer: ({ showSprites }: { showSprites: boolean }) =>
    createElement('div', { 'data-testid': 'text-layer', 'data-sprites': String(showSprites) }),
}))
vi.mock('@/components/canvas/ToolRail', () => ({ ToolRail: () => null }))
vi.mock('@/components/canvas/SubToolRail', () => ({ SubToolRail: () => null }))
vi.mock('@/components/canvas/CanvasToolbar', () => ({ CanvasToolbar: () => null }))
vi.mock('@/components/canvas/SelectionBar', () => ({ SelectionBar: () => null }))
vi.mock('@/hooks/useKeyboardShortcuts', () => ({ useKeyboardShortcuts: () => {} }))

import { Workspace } from '@/components/canvas/Workspace'

const image = (id: string, role: string) => ({
  id,
  visible: role !== 'official',
  transform: { x: 0, y: 0, width: 0, height: 0, rotationDeg: 0 },
  kind: { image: { role, blob: `blob-${id}`, opacity: 1, naturalWidth: 400, naturalHeight: 300 } },
})

const page = (id: string, nodes: Record<string, unknown>) => ({
  id,
  name: `${id}.jpg`,
  width: 400,
  height: 300,
  nodes,
})

/** p1 has the official release, p2 (an extra page) has none. */
const scene = {
  epoch: 1,
  scene: {
    project: { name: 'P' },
    pages: {
      p1: page('p1', {
        src: image('src', 'source'),
        off: image('off', 'official'),
        cleaned: image('cleaned', 'inpainted'),
      }),
      p2: page('p2', { src2: image('src2', 'source'), cleaned2: image('cleaned2', 'inpainted') }),
    },
  },
}

function open(pageId: string) {
  server.use(http.get('/api/v1/scene.json', () => HttpResponse.json(scene)))
  queryClient.setQueryData(getGetSceneJsonQueryKey(), scene)
  useSelectionStore.getState().setPage(pageId)
  return renderWithQuery(<Workspace />)
}

const shownPictures = (container: HTMLElement) =>
  Array.from(container.querySelectorAll('img')).map((img) => img.dataset.blob)

describe('Release view on the canvas', () => {
  beforeEach(() => {
    // jsdom has no 2D canvas; the drawing hooks take a missing context in stride.
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null)
    blobs.asked.clear()
    useEditorUiStore.setState({
      scale: 100,
      mode: 'select',
      showInpaintedImage: false,
      showRenderedImage: false,
      showOfficialImage: false,
      showTextBlocksOverlay: true,
    })
  })

  it('shows the release over the raw page, with the boxes as on Original', () => {
    useEditorUiStore.getState().setShowOfficialImage(true)
    const { container } = open('p1')

    expect(screen.getByTestId('workspace-official-image')).toHaveAttribute('data-blob', 'blob-off')
    // The raw page stays underneath; the cleaned page is not shown.
    expect(shownPictures(container)).toEqual(['blob-src', 'blob-off'])
    // The boxes are there without the translation previews, as on Original.
    expect(screen.getByTestId('text-layer')).toHaveAttribute('data-sprites', 'false')
  })

  it('leaves the release hidden, and unfetched, in the other views', () => {
    const { container } = open('p1')
    expect(screen.queryByTestId('workspace-official-image')).toBeNull()
    expect(shownPictures(container)).toEqual(['blob-src'])
    expect(blobs.asked.has('blob-off')).toBe(false)

    act(() => {
      useEditorUiStore.getState().setShowInpaintedImage(true)
    })
    expect(screen.queryByTestId('workspace-official-image')).toBeNull()
    expect(shownPictures(container)).toEqual(['blob-src', 'blob-cleaned'])
  })

  it('shows the raw page on a page without a release', () => {
    useEditorUiStore.getState().setShowOfficialImage(true)
    const { container } = open('p2')
    expect(screen.queryByTestId('workspace-official-image')).toBeNull()
    expect(shownPictures(container)).toEqual(['blob-src2'])

    // The next page with a release shows it again.
    act(() => {
      useSelectionStore.getState().setPage('p1')
    })
    expect(screen.getByTestId('workspace-official-image')).toHaveAttribute('data-blob', 'blob-off')
  })

  it('goes back to the cleaned page when a brush tool is picked', () => {
    useEditorUiStore.getState().setShowOfficialImage(true)
    const { container } = open('p1')
    expect(screen.getByTestId('workspace-official-image')).toBeInTheDocument()

    act(() => {
      useEditorUiStore.getState().setMode('brush')
    })
    expect(screen.queryByTestId('workspace-official-image')).toBeNull()
    expect(shownPictures(container)).toEqual(['blob-src', 'blob-cleaned'])
  })
})
