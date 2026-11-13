import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it } from 'vitest'

import { InpaintOptions, OcrOptions } from '@/components/canvas/StepOptions'
import { queryClient } from '@/lib/queryClient'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

const catalog = {
  detectors: [],
  fontDetectors: [],
  segmenters: [],
  bubbleSegmenters: [],
  ocr: [{ id: 'paddle-ocr-vl-1.6', name: 'PaddleOCR-VL 1.6', produces: [] }],
  translators: [],
  inpainters: [
    { id: 'flux2-klein', name: 'Flux.2 Klein', produces: [] },
    { id: 'lama-manga', name: 'LaMa', produces: [] },
  ],
  renderers: [],
}

describe('step settings on the toolbar', () => {
  let pipeline: Record<string, unknown>
  let patches: Array<Record<string, any>>

  beforeEach(() => {
    queryClient.clear()
    patches = []
    pipeline = {
      ocr: 'paddle-ocr-vl-1.6',
      inpainter: 'flux2-klein',
      flux2_steps: 2,
      flux2_flat_fill: true,
      flux2_fast_large_crops: false,
    }
    server.use(
      http.get('/api/v1/config', () => HttpResponse.json({ pipeline })),
      http.get('/api/v1/engines', () => HttpResponse.json(catalog)),
      http.patch('/api/v1/config', async ({ request }) => {
        const patch = (await request.json()) as Record<string, any>
        patches.push(patch)
        const p = patch.pipeline ?? {}
        if (p.flux2Steps != null) pipeline = { ...pipeline, flux2_steps: p.flux2Steps }
        if (p.flux2FastLargeCrops != null) {
          pipeline = { ...pipeline, flux2_fast_large_crops: p.flux2FastLargeCrops }
        }
        return HttpResponse.json({ pipeline })
      }),
    )
  })

  it('opens the clean-up settings from the arrow and saves a change at once', async () => {
    renderWithQuery(<InpaintOptions />)
    await userEvent.click(screen.getByTestId('toolbar-inpaint-options'))

    const quality = await screen.findByTestId('step-flux2-steps-4')
    expect(screen.getByTestId('step-flux2-steps-2')).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(quality)

    await waitFor(() => expect(patches).toEqual([{ pipeline: { flux2Steps: 4 } }]))
    // The panel shows what the server now holds.
    await waitFor(() =>
      expect(screen.getByTestId('step-flux2-steps-4')).toHaveAttribute('aria-pressed', 'true'),
    )
  })

  it('sends only the switch that was flipped', async () => {
    renderWithQuery(<InpaintOptions />)
    await userEvent.click(screen.getByTestId('toolbar-inpaint-options'))
    await screen.findByTestId('step-flux2-steps-2')
    const fast = document.getElementById('step-flux2-fast-large-crops') as HTMLElement
    expect(fast).not.toBeChecked()
    await userEvent.click(fast)
    await waitFor(() => expect(patches).toEqual([{ pipeline: { flux2FastLargeCrops: true } }]))
  })

  it('hides the Flux options for another engine', async () => {
    pipeline = { ...pipeline, inpainter: 'lama-manga' }
    renderWithQuery(<InpaintOptions />)
    await userEvent.click(screen.getByTestId('toolbar-inpaint-options'))
    expect(await screen.findByTestId('step-engine-inpainter')).toHaveTextContent('LaMa')
    expect(screen.queryByTestId('step-flux2-steps-2')).toBeNull()
  })

  it('offers the OCR engine from the OCR arrow', async () => {
    renderWithQuery(<OcrOptions />)
    await userEvent.click(screen.getByTestId('toolbar-ocr-options'))
    expect(await screen.findByTestId('step-engine-ocr')).toHaveTextContent('PaddleOCR-VL 1.6')
  })
})
