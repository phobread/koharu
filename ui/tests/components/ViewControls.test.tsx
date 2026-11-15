import { act, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it } from 'vitest'

import { ViewControls } from '@/components/canvas/ViewControls'
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

const imageNode = (id: string, role: string) => ({
  id,
  transform: { x: 0, y: 0, width: 100, height: 100, rotationDeg: 0 },
  visible: role !== 'official',
  kind: { image: { role, blob: `blob-${id}` } },
})

/** p1 has the official release, p2 (an extra page) has none. */
function sceneWithRelease() {
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
            src: imageNode('src', 'source'),
            off: imageNode('off', 'official'),
            inp: imageNode('inp', 'inpainted'),
          },
        },
        p2: {
          id: 'p2',
          name: 'P2',
          width: 100,
          height: 100,
          nodes: { src2: imageNode('src2', 'source'), inp2: imageNode('inp2', 'inpainted') },
        },
      },
      project: { name: 'Proj' },
    },
  }
}

describe('ViewControls', () => {
  beforeEach(() => {
    useSelectionStore.getState().setPage('p1')
    useEditorUiStore.setState({
      mode: 'select',
      showRenderedImage: false,
      showInpaintedImage: false,
      showOfficialImage: false,
      showTextBlocksOverlay: false,
      showSegmentationMask: false,
      showBrushLayer: false,
    })
    server.use(http.get('/api/v1/scene.json', () => HttpResponse.json(sceneWithLayers())))
  })

  it('maps the view choice onto the image toggles', async () => {
    renderWithQuery(<ViewControls />)

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
    renderWithQuery(<ViewControls />)

    const translated = await screen.findByTestId('view-translated')
    expect(translated).toHaveAttribute('aria-checked', 'true')
  })

  it('toggles the text-box overlay independently of the view', async () => {
    renderWithQuery(<ViewControls />)

    // The overlays sit behind the eye button next to the view switch.
    await userEvent.click(await screen.findByTestId('view-overlays'))
    const row = await screen.findByTestId('overlay-textBlocks')
    await userEvent.click(row)
    expect(useEditorUiStore.getState().showTextBlocksOverlay).toBe(true)
    // The view is untouched by overlay toggles.
    expect(useEditorUiStore.getState().showRenderedImage).toBe(false)
    expect(useEditorUiStore.getState().showInpaintedImage).toBe(false)
  })

  describe('Release view', () => {
    it('is not offered in a project without an official release', async () => {
      renderWithQuery(<ViewControls />)

      const cleaned = await screen.findByTestId('view-cleaned')
      await waitFor(() => expect(cleaned).not.toBeDisabled())
      // Exactly the three views of before: no greyed-out fourth button.
      expect(screen.queryByTestId('view-release')).toBeNull()
      expect(screen.getAllByRole('radio')).toHaveLength(3)
    })

    it('shows the release on its own, then hands back to the other views', async () => {
      server.use(http.get('/api/v1/scene.json', () => HttpResponse.json(sceneWithRelease())))
      renderWithQuery(<ViewControls />)

      const release = await screen.findByTestId('view-release')
      await waitFor(() => expect(release).not.toBeDisabled())
      expect(release).toHaveTextContent('layers.viewRelease')
      await userEvent.click(release)
      expect(release).toHaveAttribute('aria-checked', 'true')
      expect(screen.getByTestId('view-original')).toHaveAttribute('aria-checked', 'false')
      // Shown instead of the page: the cleaned and translated images are off.
      expect(useEditorUiStore.getState()).toMatchObject({
        showOfficialImage: true,
        showInpaintedImage: false,
        showRenderedImage: false,
      })

      await userEvent.click(screen.getByTestId('view-cleaned'))
      expect(useEditorUiStore.getState()).toMatchObject({
        showOfficialImage: false,
        showInpaintedImage: true,
      })
      await userEvent.click(release)
      await userEvent.click(screen.getByTestId('view-original'))
      expect(useEditorUiStore.getState()).toMatchObject({
        showOfficialImage: false,
        showInpaintedImage: false,
        showRenderedImage: false,
      })
    })

    it('falls back to Original on a page without a release', async () => {
      server.use(http.get('/api/v1/scene.json', () => HttpResponse.json(sceneWithRelease())))
      useEditorUiStore.setState({ showOfficialImage: true })
      useSelectionStore.getState().setPage('p2')
      renderWithQuery(<ViewControls />)

      // The project has a release, so the button stays put, but this page
      // has none: it is off and the page shows as Original.
      const release = await screen.findByTestId('view-release')
      expect(release).toBeDisabled()
      expect(release).toHaveAttribute('aria-checked', 'false')
      expect(release).toHaveAttribute('title', 'layers.viewReleaseNone')
      expect(screen.getByTestId('view-original')).toHaveAttribute('aria-checked', 'true')

      // Back on a page with a release, the Release view is still the choice.
      act(() => {
        useSelectionStore.getState().setPage('p1')
      })
      await waitFor(() => expect(release).not.toBeDisabled())
      expect(release).toHaveAttribute('aria-checked', 'true')
    })

    it('is left when a brush tool switches to the cleaned page', async () => {
      server.use(http.get('/api/v1/scene.json', () => HttpResponse.json(sceneWithRelease())))
      renderWithQuery(<ViewControls />)
      const release = await screen.findByTestId('view-release')
      await waitFor(() => expect(release).not.toBeDisabled())

      for (const mode of ['brush', 'eraser', 'repairBrush'] as const) {
        await userEvent.click(release)
        expect(release).toHaveAttribute('aria-checked', 'true')
        act(() => {
          useEditorUiStore.getState().setMode(mode)
        })
        expect(useEditorUiStore.getState()).toMatchObject({
          showOfficialImage: false,
          showInpaintedImage: true,
        })
        expect(screen.getByTestId('view-cleaned')).toHaveAttribute('aria-checked', 'true')
        act(() => {
          useEditorUiStore.getState().setMode('select')
        })
      }
    })

    it('is left whenever something else turns on the cleaned or translated page', () => {
      const ui = useEditorUiStore.getState()
      ui.setShowOfficialImage(true)
      ui.setShowRenderedImage(true) // e.g. image generation in the AI panel
      expect(useEditorUiStore.getState().showOfficialImage).toBe(false)

      ui.setShowOfficialImage(true)
      ui.setShowInpaintedImage(true) // e.g. after a repair stroke
      expect(useEditorUiStore.getState().showOfficialImage).toBe(false)

      // Turning a layer off leaves the Release view alone.
      ui.setShowOfficialImage(true)
      ui.setShowRenderedImage(false)
      ui.setShowInpaintedImage(false)
      expect(useEditorUiStore.getState().showOfficialImage).toBe(true)
    })
  })
})
