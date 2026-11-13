import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  open: vi.fn(),
  readDir: vi.fn(),
  readFile: vi.fn(),
}))

vi.mock('@/lib/backend', () => ({ isTauri: () => true }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: mocks.open }))
vi.mock('@tauri-apps/plugin-fs', () => ({
  readDir: mocks.readDir,
  readFile: mocks.readFile,
}))

import { openImageFiles, openImageFolder, openKhrFile, parentFolder } from '@/lib/io/openFiles'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'

beforeEach(() => {
  mocks.open.mockReset()
  mocks.readDir.mockReset().mockResolvedValue([])
  mocks.readFile.mockReset().mockResolvedValue(new Uint8Array())
  usePreferencesStore.setState({ lastOpenFolder: undefined, exportFolder: 'D:\\Exports' })
})

describe('parentFolder', () => {
  it('takes the folder of a Windows or POSIX path', () => {
    expect(parentFolder('D:\\Raws\\Series\\ch01\\001.png')).toBe('D:\\Raws\\Series\\ch01')
    expect(parentFolder('/home/u/raws/ch01/')).toBe('/home/u/raws')
  })

  it('has nothing to give for a bare name', () => {
    expect(parentFolder('001.png')).toBeUndefined()
    expect(parentFolder(undefined)).toBeUndefined()
  })
})

describe('open dialogs remember their own folder', () => {
  it('opens files where the last pages came from, not in the export folder', async () => {
    mocks.open.mockResolvedValueOnce(['D:\\Raws\\Series\\ch01\\001.png'])
    await openImageFiles()
    expect(mocks.open.mock.calls[0][0].defaultPath).toBeUndefined()
    expect(usePreferencesStore.getState().lastOpenFolder).toBe('D:\\Raws\\Series\\ch01')

    mocks.open.mockResolvedValueOnce(['D:\\Raws\\Series\\ch01\\002.png'])
    await openImageFiles()
    expect(mocks.open.mock.calls[1][0].defaultPath).toBe('D:\\Raws\\Series\\ch01')
    expect(usePreferencesStore.getState().exportFolder).toBe('D:\\Exports')
  })

  it('starts the next folder pick one level up, beside the last chapter', async () => {
    mocks.open.mockResolvedValueOnce('D:\\Raws\\Series\\ch01')
    await openImageFolder()
    expect(usePreferencesStore.getState().lastOpenFolder).toBe('D:\\Raws\\Series')

    mocks.open.mockResolvedValueOnce('D:\\Raws\\Series\\ch02')
    await openImageFolder()
    expect(mocks.open.mock.calls[1][0]).toMatchObject({
      directory: true,
      defaultPath: 'D:\\Raws\\Series',
    })
  })

  it('keeps the remembered folder when a dialog is cancelled', async () => {
    usePreferencesStore.setState({ lastOpenFolder: 'D:\\Raws' })
    mocks.open.mockResolvedValue(null)
    await openImageFiles()
    await openImageFolder()
    await openKhrFile()
    expect(usePreferencesStore.getState().lastOpenFolder).toBe('D:\\Raws')
    expect(mocks.open.mock.calls.every(([options]) => options.defaultPath === 'D:\\Raws')).toBe(
      true,
    )
  })
})
