import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  mkdir: vi.fn(),
  open: vi.fn(),
  pictureDir: vi.fn(),
  save: vi.fn(),
  writeFile: vi.fn(),
  join: vi.fn(async (...parts: string[]) => parts.join('/')),
}))

vi.mock('@/lib/backend', () => ({ isTauri: () => true }))
vi.mock('@tauri-apps/api/path', () => ({
  join: mocks.join,
  pictureDir: mocks.pictureDir,
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: mocks.open,
  save: mocks.save,
}))
vi.mock('@tauri-apps/plugin-fs', () => ({
  mkdir: mocks.mkdir,
  writeFile: mocks.writeFile,
}))

import { defaultRenderedExportDirectory, pickSaveDirectory, saveBlob } from '@/lib/io/saveBlob'

beforeEach(() => {
  mocks.pictureDir.mockResolvedValue('C:/Users/Test/Pictures')
  mocks.mkdir.mockResolvedValue(undefined)
  mocks.open.mockResolvedValue(undefined)
  mocks.save.mockResolvedValue(undefined)
  mocks.writeFile.mockResolvedValue(undefined)
})

describe('Tauri rendered export defaults', () => {
  it('creates Pictures/Koharu/<project>/Rendered using the OS known folder', async () => {
    const folder = await defaultRenderedExportDirectory('Bad: End')

    expect(folder).toBe('C:/Users/Test/Pictures/Koharu/Bad_ End/Rendered')
    expect(mocks.mkdir).toHaveBeenCalledWith(folder, { recursive: true })
  })

  it('opens the folder picker at the supplied default directory', async () => {
    mocks.open.mockResolvedValue('D:/Manga')

    await expect(pickSaveDirectory('C:/Users/Test/Pictures/Koharu/P/Rendered')).resolves.toBe(
      'D:/Manga',
    )
    expect(mocks.open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      defaultPath: 'C:/Users/Test/Pictures/Koharu/P/Rendered',
    })
  })

  it('opens a single-image save dialog with the filename inside that directory', async () => {
    mocks.save.mockResolvedValue('C:/Users/Test/Pictures/Koharu/P/Rendered/page.png')
    const blob = new Blob([new Uint8Array([1, 2, 3])], { type: 'image/png' })

    await expect(
      saveBlob(blob, 'page.png', {
        defaultDirectory: 'C:/Users/Test/Pictures/Koharu/P/Rendered',
      }),
    ).resolves.toBe(true)

    expect(mocks.save).toHaveBeenCalledWith({
      defaultPath: 'C:/Users/Test/Pictures/Koharu/P/Rendered/page.png',
    })
    expect(mocks.writeFile).toHaveBeenCalledWith(
      'C:/Users/Test/Pictures/Koharu/P/Rendered/page.png',
      new Uint8Array([1, 2, 3]),
    )
  })
})
