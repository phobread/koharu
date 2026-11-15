import { afterEach, describe, expect, it, vi } from 'vitest'

describe('preferencesStore persistence', () => {
  afterEach(() => {
    vi.doUnmock('@/lib/stores/serverConfigStorage')
    vi.resetModules()
  })

  it('adds the close-project default when rehydrating shortcuts saved before version 9', async () => {
    const persisted = JSON.stringify({
      state: {
        shortcuts: {
          select: 'V',
          block: 'M',
          brush: 'B',
          eraser: 'E',
          repairBrush: 'R',
          increaseBrushSize: ']',
          decreaseBrushSize: '[',
          undo: 'Ctrl+U',
          redo: 'Ctrl+Shift+U',
        },
      },
      version: 8,
    })
    const storage = {
      getItem: vi.fn(async () => persisted),
      setItem: vi.fn(async () => undefined),
      removeItem: vi.fn(async () => undefined),
    }
    vi.resetModules()
    vi.doMock('@/lib/stores/serverConfigStorage', () => ({ serverConfigStorage: storage }))

    const { usePreferencesStore } = await import('@/lib/stores/preferencesStore')
    await usePreferencesStore.persist.rehydrate()

    expect(usePreferencesStore.getState().shortcuts).toMatchObject({
      closeProject: 'Ctrl+W',
      undo: 'Ctrl+U',
      redo: 'Ctrl+Shift+U',
    })
  })

  it.each([
    ['adds T for showing / hiding text boxes', { undo: 'Ctrl+Z' }, 'T'],
    ['leaves it unbound when T is already taken', { brush: 'T' }, ''],
  ])('%s when rehydrating shortcuts saved before version 11', async (_, extra, expected) => {
    const persisted = JSON.stringify({
      state: {
        shortcuts: {
          select: 'V',
          block: 'M',
          brush: 'B',
          eraser: 'E',
          repairBrush: 'R',
          increaseBrushSize: ']',
          decreaseBrushSize: '[',
          closeProject: 'Ctrl+W',
          undo: 'Ctrl+Z',
          redo: 'Ctrl+Shift+Z',
          ...extra,
        },
      },
      version: 10,
    })
    const storage = {
      getItem: vi.fn(async () => persisted),
      setItem: vi.fn(async () => undefined),
      removeItem: vi.fn(async () => undefined),
    }
    vi.resetModules()
    vi.doMock('@/lib/stores/serverConfigStorage', () => ({ serverConfigStorage: storage }))

    const { usePreferencesStore } = await import('@/lib/stores/preferencesStore')
    await usePreferencesStore.persist.rehydrate()

    expect(usePreferencesStore.getState().shortcuts.toggleTextBoxes).toBe(expected)
    expect(usePreferencesStore.getState().shortcuts.repairBrush).toBe('R')
  })

  it('replaces the old Custom pipeline ticks with all Process steps ticked', async () => {
    const persisted = JSON.stringify({
      state: {
        customPipeline: {
          detect: false,
          ocr: false,
          translator: false,
          inpainter: true,
          renderer: false,
        },
      },
      version: 9,
    })
    const storage = {
      getItem: vi.fn(async () => persisted),
      setItem: vi.fn(async () => undefined),
      removeItem: vi.fn(async () => undefined),
    }
    vi.resetModules()
    vi.doMock('@/lib/stores/serverConfigStorage', () => ({ serverConfigStorage: storage }))

    const { usePreferencesStore } = await import('@/lib/stores/preferencesStore')
    await usePreferencesStore.persist.rehydrate()

    const state = usePreferencesStore.getState() as unknown as Record<string, unknown>
    expect(state.processSteps).toEqual({
      detect: true,
      ocr: true,
      translate: true,
      inpaint: true,
      render: true,
    })
    expect(state).not.toHaveProperty('customPipeline')
  })
})
