import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it } from 'vitest'

import { LayersPanel } from '@/components/panels/LayersPanel'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

function sceneWithLayers() {
  return {
    epoch: 1,
    scene: {
      pages: {
        p1: {
          id: 'p1',
          name: 'P1',
          width: 100,
          height: 100,
          nodes: {
            src: {
              id: 'src',
              transform: { x: 0, y: 0, width: 100, height: 100, rotationDeg: 0 },
              visible: true,
              kind: { image: { role: 'source', blob: 'blob-src' } },
            },
            inp: {
              id: 'inp',
              transform: { x: 0, y: 0, width: 100, height: 100, rotationDeg: 0 },
              visible: true,
              kind: { image: { role: 'inpainted', blob: 'blob-inp' } },
            },
            t1: {
              id: 't1',
              transform: { x: 0, y: 0, width: 10, height: 10, rotationDeg: 0 },
              visible: true,
              kind: { text: { text: 'first' } },
            },
          },
        },
      },
      project: { name: 'Proj' },
    },
  }
}

describe('LayersPanel', () => {
  beforeEach(() => {
    useSelectionStore.getState().setPage('p1')
    useEditorUiStore.setState({
      showRenderedImage: false,
      showInpaintedImage: false,
      showTextBlocksOverlay: false,
      showSegmentationMask: false,
      showBrushLayer: false,
    })
    server.use(http.get('/api/v1/scene.json', () => HttpResponse.json(sceneWithLayers())))
  })

  it('maps the view choice onto the image toggles', async () => {
    renderWithQuery(<LayersPanel />)

    const original = await screen.findByTestId('view-original')
    expect(original).toHaveAttribute('aria-checked', 'true')

    const cleaned = screen.getByTestId('view-cleaned')
    await waitFor(() => expect(cleaned).not.toBeDisabled())
    await userEvent.click(cleaned)
    expect(useEditorUiStore.getState().showInpaintedImage).toBe(true)
    expect(useEditorUiStore.getState().showRenderedImage).toBe(false)

    // No rendered image on the page → the Translated view is not offered.
    expect(screen.getByTestId('view-translated')).toBeDisabled()

    await userEvent.click(screen.getByTestId('view-original'))
    expect(useEditorUiStore.getState().showInpaintedImage).toBe(false)
    expect(useEditorUiStore.getState().showRenderedImage).toBe(false)
  })

  it('reflects an externally set translated view', async () => {
    useEditorUiStore.setState({ showRenderedImage: true, showInpaintedImage: true })
    renderWithQuery(<LayersPanel />)

    const translated = await screen.findByTestId('view-translated')
    expect(translated).toHaveAttribute('aria-checked', 'true')
  })

  it('toggles the text-box overlay independently of the view', async () => {
    renderWithQuery(<LayersPanel />)

    const row = await screen.findByTestId('overlay-textBlocks')
    await userEvent.click(row)
    expect(useEditorUiStore.getState().showTextBlocksOverlay).toBe(true)
    // The view is untouched by overlay toggles.
    expect(useEditorUiStore.getState().showRenderedImage).toBe(false)
    expect(useEditorUiStore.getState().showInpaintedImage).toBe(false)
  })
})
