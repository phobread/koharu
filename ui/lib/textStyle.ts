import type {
  FontPrediction,
  TextAlign,
  TextFillGradient,
  TextShaderEffect,
  TextStrokeStyle,
  TextStyle,
} from '@/lib/api/schemas'

/**
 * Shared helpers for building explicit per-block `TextStyle` patches. The
 * scene only stores a style when the user overrides something; these helpers
 * merge an update over the existing style. Automatic render colour is now
 * chosen by the renderer from the page background, so stale model-predicted
 * colours are treated as auto placeholders rather than user intent.
 */

const clampByte = (v: number) => Math.max(0, Math.min(255, Math.round(v)))

export const DEFAULT_TEXT_COLOR: number[] = [0, 0, 0, 255]

export const predictionColor = (prediction?: FontPrediction | null): number[] | undefined => {
  const tc = prediction?.textColor
  if (!tc || tc.length < 3) return undefined
  return [clampByte(tc[0]), clampByte(tc[1]), clampByte(tc[2]), 255]
}

const sameRgb = (a: number[], b: number[]) =>
  a.length >= 3 &&
  b.length >= 3 &&
  clampByte(a[0]) === clampByte(b[0]) &&
  clampByte(a[1]) === clampByte(b[1]) &&
  clampByte(a[2]) === clampByte(b[2])

export const isManualTextColor = (
  color?: number[] | null,
  prediction?: FontPrediction | null,
): boolean => {
  if (!color || color.length < 3) return false
  if ((color[3] ?? 255) !== 255) return true
  if (sameRgb(color, DEFAULT_TEXT_COLOR)) return false
  const predicted = predictionColor(prediction)
  if (predicted && sameRgb(color, predicted)) return false
  return true
}

/** Mirrors renderer intent: manual style colour wins; otherwise auto previews black. */
export const effectiveTextColor = (
  style?: TextStyle | null,
  prediction?: FontPrediction | null,
): number[] => (isManualTextColor(style?.color, prediction) ? style!.color : DEFAULT_TEXT_COLOR)

/**
 * Partial style update where each field distinguishes three states: key
 * absent = keep the block's current value, explicit `null` = reset the field
 * to auto (renderer falls back to prediction/global default), value = set it.
 */
export type TextStyleUpdates = {
  fontFamilies?: string[] | null
  fontSize?: number | null
  color?: number[] | null
  effect?: TextShaderEffect | null
  stroke?: TextStrokeStyle | null
  textAlign?: TextAlign | null
  gradient?: TextFillGradient | null
}

/**
 * Merge updates over a block's existing style into a full style. `color` has
 * no "absent" representation in a stored style, so resetting it writes the
 * black auto placeholder; the renderer treats that placeholder as background
 * contrast mode instead of a manual black override.
 */
export const mergeTextStyle = (
  current: TextStyle | null | undefined,
  prediction: FontPrediction | null | undefined,
  updates: TextStyleUpdates,
): TextStyle => ({
  fontFamilies:
    'fontFamilies' in updates ? (updates.fontFamilies ?? []) : (current?.fontFamilies ?? []),
  fontSize: 'fontSize' in updates ? (updates.fontSize ?? null) : (current?.fontSize ?? null),
  color:
    'color' in updates
      ? (updates.color ?? DEFAULT_TEXT_COLOR)
      : effectiveTextColor(current, prediction),
  effect: 'effect' in updates ? (updates.effect ?? null) : (current?.effect ?? null),
  stroke: 'stroke' in updates ? (updates.stroke ?? null) : (current?.stroke ?? null),
  textAlign: 'textAlign' in updates ? (updates.textAlign ?? null) : (current?.textAlign ?? null),
  gradient: 'gradient' in updates ? (updates.gradient ?? null) : (current?.gradient ?? null),
})
