import { waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { FontUploadButton } from '@/components/ui/font-upload-button'
import { queryClient } from '@/lib/queryClient'

import { renderWithQuery } from '../helpers'
import { server } from '../msw/server'

describe('FontUploadButton', () => {
  beforeEach(() => {
    queryClient.clear()
  })

  it('uploads the picked file and reports the new face', async () => {
    let receivedFilename: string | null = null
    server.use(
      http.post('/api/v1/fonts/upload', ({ request }) => {
        receivedFilename = new URL(request.url).searchParams.get('filename')
        return HttpResponse.json([
          {
            familyName: 'My Font',
            postScriptName: 'MyFont-Regular',
            source: 'custom',
            category: null,
            cached: true,
          },
        ])
      }),
    )

    const onUploaded = vi.fn()
    const { getByTestId } = renderWithQuery(<FontUploadButton onUploaded={onUploaded} />)

    const input = getByTestId('font-upload-input') as HTMLInputElement
    const file = new File([new Uint8Array([1, 2, 3, 4])], 'My Font.otf', { type: 'font/otf' })
    await userEvent.upload(input, file)

    await waitFor(() => expect(onUploaded).toHaveBeenCalledWith('MyFont-Regular'))
    expect(receivedFilename).toBe('My Font.otf')
  })

  it('surfaces an error and does not report a face when the upload fails', async () => {
    server.use(
      http.post('/api/v1/fonts/upload', () =>
        HttpResponse.json({ message: 'not a font' }, { status: 400 }),
      ),
    )

    const onUploaded = vi.fn()
    const { getByTestId } = renderWithQuery(<FontUploadButton onUploaded={onUploaded} />)

    const input = getByTestId('font-upload-input') as HTMLInputElement
    const file = new File([new Uint8Array([0])], 'bad.txt', { type: 'text/plain' })
    await userEvent.upload(input, file)

    await waitFor(() => expect(getByTestId('font-upload-button')).not.toBeDisabled())
    expect(onUploaded).not.toHaveBeenCalled()
  })
})
