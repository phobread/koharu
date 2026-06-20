import type { StartPipelineRequest } from '@/lib/api/schemas'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'

/**
 * Global render defaults (font, size, box padding, border/stroke, bold/italic)
 * read from the persisted stores and shaped for `StartPipelineRequest`.
 *
 * These apply only to text blocks that have no explicit per-node override —
 * per-node styles saved in the scene still win on the backend. Reading them
 * here (rather than inlining at each call site) keeps every pipeline trigger
 * — manual Render, custom pipeline, and debounced auto-render — consistent.
 */
export function renderDefaultsForPipeline(): Pick<
  StartPipelineRequest,
  'defaultFont' | 'defaultFontSize' | 'boxPadding' | 'shaderEffect' | 'shaderStroke'
> {
  const prefs = usePreferencesStore.getState()
  const { renderEffect, renderStroke } = useEditorUiStore.getState()
  return {
    defaultFont: prefs.defaultFont,
    defaultFontSize: prefs.defaultFontSize,
    boxPadding: prefs.boxPadding,
    shaderEffect: { bold: renderEffect.bold, italic: renderEffect.italic },
    shaderStroke: renderStroke
      ? {
          enabled: renderStroke.enabled,
          color: renderStroke.color,
          widthPx: renderStroke.widthPx ?? null,
        }
      : undefined,
  }
}
