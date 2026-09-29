import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { usePreferencesStore } from '@/lib/stores/preferencesStore'

import { renderWithQuery } from '../helpers'

const mocks = vi.hoisted(() => ({ tauri: true, pick: vi.fn() }))

vi.mock('@/lib/backend', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/backend')>()),
  isTauri: () => mocks.tauri,
}))
vi.mock('@/lib/io/saveBlob', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/io/saveBlob')>()),
  defaultExportFolder: vi.fn().mockResolvedValue('C:\\Users\\Me\\Pictures\\Koharu'),
  pickSaveDirectory: mocks.pick,
}))

import { ExportFolderSetting } from '@/components/ExportFolderSetting'

describe('ExportFolderSetting', () => {
  beforeEach(() => {
    mocks.tauri = true
    mocks.pick.mockReset()
    usePreferencesStore.setState({ exportFolder: undefined })
  })

  it('shows Pictures\\Koharu until a folder is picked, then the picked folder', async () => {
    mocks.pick.mockResolvedValue('D:\\Manga')
    renderWithQuery(<ExportFolderSetting />)
    await waitFor(() =>
      expect(screen.getByTestId('export-folder-path')).toHaveValue(
        'C:\\Users\\Me\\Pictures\\Koharu',
      ),
    )
    expect(screen.queryByTestId('export-folder-reset')).not.toBeInTheDocument()

    await userEvent.click(screen.getByTestId('export-folder-change'))

    expect(mocks.pick).toHaveBeenCalledWith('C:\\Users\\Me\\Pictures\\Koharu')
    expect(usePreferencesStore.getState().exportFolder).toBe('D:\\Manga')
    expect(screen.getByTestId('export-folder-path')).toHaveValue('D:\\Manga')
  })

  it('keeps the folder when the picker is cancelled, and can go back to the default', async () => {
    usePreferencesStore.setState({ exportFolder: 'D:\\Manga' })
    mocks.pick.mockResolvedValue(undefined)
    renderWithQuery(<ExportFolderSetting />)

    await userEvent.click(screen.getByTestId('export-folder-change'))
    expect(usePreferencesStore.getState().exportFolder).toBe('D:\\Manga')

    await userEvent.click(screen.getByTestId('export-folder-reset'))
    expect(usePreferencesStore.getState().exportFolder).toBeUndefined()
  })

  it('is hidden in the browser build', () => {
    mocks.tauri = false
    const { container } = renderWithQuery(<ExportFolderSetting />)
    expect(container.firstChild).toBeNull()
  })
})
