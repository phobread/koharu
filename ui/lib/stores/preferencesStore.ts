'use client'

import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'

import { getPlatform } from '@/lib/shortcutUtils'
import { serverConfigStorage } from '@/lib/stores/serverConfigStorage'

export type ProcessSteps = {
  detect: boolean
  ocr: boolean
  translate: boolean
  inpaint: boolean
  render: boolean
}

type PreferencesState = {
  brushConfig: {
    size: number
    color: string
  }
  setBrushConfig: (config: Partial<PreferencesState['brushConfig']>) => void
  /** Repair-brush strokes use LaMa instead of the pipeline's inpainter. */
  repairWithLama: boolean
  setRepairWithLama: (enabled: boolean) => void
  /** Style panel: the "More" section (direction, border, gradient, padding) is open. */
  styleMoreOpen: boolean
  setStyleMoreOpen: (open: boolean) => void
  defaultFont?: string
  setDefaultFont: (font?: string) => void
  /** Global default text size (px). Caps render auto-fit; undefined = auto. */
  defaultFontSize?: number
  setDefaultFontSize: (size?: number) => void
  /** Pixels to inset text from each layout-box edge (stops edge clipping). */
  boxPadding: number
  setBoxPadding: (px: number) => void
  favoriteFonts: string[]
  toggleFavoriteFont: (font: string) => void
  customSystemPrompt?: string
  setCustomSystemPrompt: (prompt?: string) => void
  /** OCR hint: language of the source text (e.g. "Korean"). undefined = auto. */
  ocrLanguage?: string
  setOcrLanguage: (lang?: string) => void
  codexImagePrompt?: string
  setCodexImagePrompt: (prompt?: string) => void
  codexImageModel?: string
  setCodexImageModel: (model?: string) => void
  shortcuts: {
    select: string
    block: string
    brush: string
    eraser: string
    repairBrush: string
    increaseBrushSize: string
    decreaseBrushSize: string
    closeProject: string
    undo: string
    redo: string
  }
  setShortcuts: (shortcuts: Partial<PreferencesState['shortcuts']>) => void
  resetShortcuts: () => void
  /** Steps the Process actions run (each only where it's still missing). */
  processSteps: ProcessSteps
  setProcessSteps: (steps: Partial<ProcessSteps>) => void
  resetPreferences: () => void
}

const DEFAULT_CUSTOM_SYSTEM_PROMPT = 'Write in a descriptive, vivid, erotica style.'

const initialPreferences = {
  brushConfig: {
    size: 36,
    color: '#ffffff',
  },
  repairWithLama: false,
  styleMoreOpen: false,
  boxPadding: 0,
  favoriteFonts: [],
  shortcuts: {
    select: 'V',
    block: 'M',
    brush: 'B',
    eraser: 'E',
    repairBrush: 'R',
    increaseBrushSize: ']',
    decreaseBrushSize: '[',
    closeProject: getPlatform() === 'mac' ? 'Cmd+W' : 'Ctrl+W',
    undo: getPlatform() === 'mac' ? 'Cmd+Z' : 'Ctrl+Z',
    redo: getPlatform() === 'mac' ? 'Cmd+Shift+Z' : 'Ctrl+Shift+Z',
  },
  customSystemPrompt: DEFAULT_CUSTOM_SYSTEM_PROMPT,
  codexImagePrompt:
    'Translate all visible text to natural English, remove the original lettering, and redraw the page as a clean manga image while preserving the artwork, panel layout, speech bubbles, tone, and composition.',
  codexImageModel: 'gpt-5.5',
  processSteps: {
    detect: true,
    ocr: true,
    translate: true,
    inpaint: true,
    render: true,
  },
}

export const usePreferencesStore = create<PreferencesState>()(
  persist(
    (set) => ({
      ...initialPreferences,
      setBrushConfig: (config) =>
        set((state) => ({
          brushConfig: {
            ...state.brushConfig,
            ...config,
          },
        })),
      setRepairWithLama: (enabled) => set({ repairWithLama: enabled }),
      setStyleMoreOpen: (open) => set({ styleMoreOpen: open }),
      setDefaultFont: (font) => set({ defaultFont: font }),
      setDefaultFontSize: (size) =>
        set({
          defaultFontSize:
            size === undefined || !Number.isFinite(size)
              ? undefined
              : Math.max(1, Math.round(size)),
        }),
      setBoxPadding: (px) =>
        set({ boxPadding: Number.isFinite(px) ? Math.max(0, Math.round(px)) : 0 }),
      toggleFavoriteFont: (font) =>
        set((state) => ({
          favoriteFonts: state.favoriteFonts.includes(font)
            ? state.favoriteFonts.filter((f) => f !== font)
            : [...state.favoriteFonts, font],
        })),
      setCustomSystemPrompt: (prompt) => set({ customSystemPrompt: prompt }),
      setOcrLanguage: (lang) => set({ ocrLanguage: lang }),
      setCodexImagePrompt: (prompt) => set({ codexImagePrompt: prompt }),
      setCodexImageModel: (model) => set({ codexImageModel: model }),
      setShortcuts: (shortcuts) =>
        set((state) => ({
          shortcuts: {
            ...state.shortcuts,
            ...shortcuts,
          },
        })),
      resetShortcuts: () =>
        set(() => ({
          shortcuts: {
            ...initialPreferences.shortcuts,
          },
        })),
      setProcessSteps: (steps) =>
        set((state) => ({
          processSteps: {
            ...state.processSteps,
            ...steps,
          },
        })),
      resetPreferences: () => set({ ...initialPreferences }),
    }),
    {
      name: 'koharu-config',
      storage: createJSONStorage(() => serverConfigStorage),
      version: 10,
      migrate: (persisted: any, version: number) => {
        if (version < 2 && persisted) {
          delete persisted.localLlm
          delete persisted.openAiCompatibleConfigVersion
        }
        if (version < 3 && persisted) {
          delete persisted.apiKeys
          delete persisted.providerBaseUrls
          delete persisted.providerModelNames
        }
        if (version < 4 && persisted?.shortcuts) {
          for (const key in persisted.shortcuts) {
            const val = persisted.shortcuts[key]
            if (typeof val === 'string' && val.length === 1) {
              persisted.shortcuts[key] = val.toUpperCase()
            }
          }
        }
        if (version < 5 && persisted?.shortcuts) {
          const isMac = getPlatform() === 'mac'
          if (!persisted.shortcuts.undo) {
            persisted.shortcuts.undo = isMac ? 'Cmd+Z' : 'Ctrl+Z'
          }
          if (!persisted.shortcuts.redo) {
            persisted.shortcuts.redo = isMac ? 'Cmd+Shift+Z' : 'Ctrl+Shift+Z'
          }
        }
        if (version < 6 && persisted) {
          persisted.codexImagePrompt ??= initialPreferences.codexImagePrompt
          persisted.codexImageModel ??= initialPreferences.codexImageModel
        }
        if (version < 10 && persisted) {
          // The Custom pipeline submenu became the Process step ticks.
          delete persisted.customPipeline
          persisted.processSteps ??= initialPreferences.processSteps
        }
        if (version < 8 && persisted) {
          persisted.boxPadding ??= initialPreferences.boxPadding
        }
        if (version < 9 && persisted?.shortcuts) {
          persisted.shortcuts.closeProject ??= initialPreferences.shortcuts.closeProject
        }
        return persisted
      },
      partialize: (state) => ({
        brushConfig: state.brushConfig,
        repairWithLama: state.repairWithLama,
        styleMoreOpen: state.styleMoreOpen,
        defaultFont: state.defaultFont,
        defaultFontSize: state.defaultFontSize,
        boxPadding: state.boxPadding,
        favoriteFonts: state.favoriteFonts,
        customSystemPrompt: state.customSystemPrompt,
        ocrLanguage: state.ocrLanguage,
        codexImagePrompt: state.codexImagePrompt,
        codexImageModel: state.codexImageModel,
        shortcuts: state.shortcuts,
        processSteps: state.processSteps,
      }),
    },
  ),
)
