import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { BlockQuickEditor } from '@/components/canvas/BlockQuickEditor'
import { CanvasToolbar } from '@/components/canvas/CanvasToolbar'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

const textNode = {
  id: 't1',
  transform: { x: 10, y: 10, width: 40, height: 20, rotationDeg: 0 },
  visible: true,
  kind: { text: { text: '원문', translation: 'Hi', renderedFontSizePx: 40 } },
}
const page = { id: 'p1', name: 'p1', width: 200, height: 200, nodes: { t1: textNode } }
const scene = { epoch: 1, scene: { project: { name: 'P' }, pages: { p1: page } } }

describe('box editor size', () => {
  let applied: Array<Record<string, any>>
  beforeEach(() => {
    applied = []
    server.use(
      http.get('/api/v1/scene.json', () => HttpResponse.json(scene)),
      http.post('/api/v1/history/apply', async ({ request }) => {
        applied.push((await request.json()) as Record<string, any>)
        return HttpResponse.json({ epoch: 2 })
      }),
    )
  })

  const renderEditor = () =>
    renderWithQuery(
      <BlockQuickEditor
        page={page as never}
        node={{ id: 't1', transform: textNode.transform, data: textNode.kind.text } as never}
        index={0}
        scale={1}
        showOriginal={false}
        onToggleOriginal={() => {}}
        onClose={() => {}}
      />,
    )

  it('shows the fitted size as auto and steps from it', async () => {
    renderEditor()
    expect(screen.getByTestId('quick-editor-size')).toHaveAttribute('placeholder', 'auto (40)')
    await userEvent.click(screen.getByTestId('quick-editor-size-up'))
    await waitFor(() => expect(applied).toHaveLength(1))
    expect(applied[0].updateNode.patch.data.text.style.fontSize).toBe(41)
    expect(screen.getByTestId('quick-editor-size-auto')).toBeDisabled()
  })

  it('switches off "original under the box" so a size change is visible', async () => {
    const toggle = vi.fn()
    renderWithQuery(
      <BlockQuickEditor
        page={page as never}
        node={{ id: 't1', transform: textNode.transform, data: textNode.kind.text } as never}
        index={0}
        scale={1}
        showOriginal={true}
        onToggleOriginal={toggle}
        onClose={() => {}}
      />,
    )
    await userEvent.click(screen.getByTestId('quick-editor-size-down'))
    expect(toggle).toHaveBeenCalledTimes(1)
  })

  it('takes a typed size on Enter', async () => {
    renderEditor()
    const input = screen.getByTestId('quick-editor-size')
    await userEvent.type(input, '52{Enter}')
    await waitFor(() => expect(applied).toHaveLength(1))
    expect(applied[0].updateNode.patch.data.text.style.fontSize).toBe(52)
  })
})

describe('Translate button without a model', () => {
  beforeEach(() => {
    useSelectionStore.getState().setPage('p1')
    useEditorUiStore.getState().closeSettings()
    server.use(
      http.get('/api/v1/scene.json', () => HttpResponse.json(scene)),
      http.get('/api/v1/llm/current', () => HttpResponse.json({ status: 'empty' })),
    )
  })

  it('opens Settings at Translation instead of doing nothing', async () => {
    const start = vi.fn()
    server.use(
      http.post('/api/v1/pipelines', () => {
        start()
        return HttpResponse.json({ operationId: 'x', pageCount: 1 })
      }),
    )
    renderWithQuery(<CanvasToolbar />)
    const button = await screen.findByTestId('toolbar-translate')
    await waitFor(() => expect(button).toHaveAttribute('data-llm-ready', 'false'))
    expect(button).not.toBeDisabled()
    await userEvent.click(button)
    expect(useEditorUiStore.getState().settingsTab).toBe('translation')
    expect(start).not.toHaveBeenCalled()
  })
})
