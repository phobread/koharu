import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'

import { SettingsDialog } from '@/components/SettingsDialog'
import type { AppConfig, ConfigPatch, EngineCatalog } from '@/lib/api/schemas'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

const pipeline = {
  detector: 'detector',
  font_detector: 'font-detector',
  segmenter: 'segmenter',
  bubble_segmenter: 'bubble-segmenter',
  ocr: 'ocr',
  translator: 'translator',
  inpainter: 'lama-manga',
  renderer: 'renderer',
  flux2_strength: 1,
  flux2_steps: 2,
}

const engine = (id: string) => ({ id, name: id, produces: [] })

const engineCatalog: EngineCatalog = {
  detectors: [engine('detector')],
  fontDetectors: [engine('font-detector')],
  segmenters: [engine('segmenter')],
  bubbleSegmenters: [engine('bubble-segmenter')],
  ocr: [engine('ocr')],
  translators: [engine('translator')],
  inpainters: [engine('lama-manga'), engine('flux2-klein')],
  renderers: [engine('renderer')],
}

function installSettingsHandlers(config: AppConfig, patches: ConfigPatch[] = []): void {
  let current = config
  server.use(
    http.get('/api/v1/config', () => HttpResponse.json(current)),
    http.patch('/api/v1/config', async ({ request }) => {
      const patch = (await request.json()) as ConfigPatch
      patches.push(patch)
      current = {
        ...current,
        pipeline: {
          ...current.pipeline,
          flux2_steps: patch.pipeline?.flux2Steps ?? current.pipeline?.flux2_steps,
        },
      }
      return HttpResponse.json(current)
    }),
    http.get('/api/v1/engines', () => HttpResponse.json(engineCatalog)),
    http.get('/api/v1/llm/catalog', () => HttpResponse.json({ localModels: [], providers: [] })),
    http.get('/api/v1/meta', () => HttpResponse.json({ version: 'test' })),
  )
}

function renderSettings(inpainter: string, flux2Steps: number) {
  installSettingsHandlers({
    pipeline: { ...pipeline, inpainter, flux2_steps: flux2Steps },
    providers: [],
  })
  return renderWithQuery(
    <SettingsDialog open={true} onOpenChange={() => {}} defaultTab='engines' />,
  )
}

describe('SettingsDialog Flux.2 Klein quality', () => {
  it('shows the quality row only for the selected Flux.2 Klein inpainter', async () => {
    const lama = renderSettings('lama-manga', 2)
    await screen.findByText('settings.enginesDescription')
    expect(
      screen.queryByRole('group', { name: 'settings.flux2KleinQuality' }),
    ).not.toBeInTheDocument()
    lama.unmount()

    renderSettings('flux2-klein', 2)
    expect(
      await screen.findByRole('group', { name: 'settings.flux2KleinQuality' }),
    ).toBeInTheDocument()
  })

  it('PATCHes the full pipeline with four steps when Quality is clicked', async () => {
    const patches: ConfigPatch[] = []
    installSettingsHandlers(
      {
        pipeline: { ...pipeline, inpainter: 'flux2-klein', flux2_steps: 2 },
        providers: [],
      },
      patches,
    )
    renderWithQuery(<SettingsDialog open={true} onOpenChange={() => {}} defaultTab='engines' />)

    await userEvent.click(await screen.findByRole('button', { name: 'settings.flux2Quality' }))

    await waitFor(() => expect(patches).toHaveLength(1))
    expect(patches[0].pipeline).toMatchObject({
      detector: 'detector',
      inpainter: 'flux2-klein',
      renderer: 'renderer',
      flux2Steps: 4,
    })
  })

  it('selects only a supported committed step value', async () => {
    const fast = renderSettings('flux2-klein', 2)
    expect(await screen.findByRole('button', { name: 'settings.flux2Fast' })).toHaveAttribute(
      'aria-pressed',
      'true',
    )
    expect(screen.getByRole('button', { name: 'settings.flux2Quality' })).toHaveAttribute(
      'aria-pressed',
      'false',
    )
    fast.unmount()

    renderSettings('flux2-klein', 8)
    expect(await screen.findByRole('button', { name: 'settings.flux2Fast' })).toHaveAttribute(
      'aria-pressed',
      'false',
    )
    expect(screen.getByRole('button', { name: 'settings.flux2Quality' })).toHaveAttribute(
      'aria-pressed',
      'false',
    )
  })
})
