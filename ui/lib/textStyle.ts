import type {
  FontPrediction,
  TextAlign,
  TextShaderEffect,
  TextStrokeStyle,
  TextStyle,
} from '@/lib/api/schemas'

/**
 * Shared helpers for building explicit per-block `TextStyle` patches. The
 * scene only stores a style when the user overrides something; these helpers
 * merge an update over the existing style while materialising the effective
 * color, so an implicit predicted color isn't silently replaced by black when
 * the style becomes explicit.
 */

const clampByte = (v: number) => Math.max(0, Math.min(255, Math.round(v)))

export const DEFAULT_TEXT_COLOR: number[] = [0, 0, 0, 255]

export const predictionColor = (prediction?: FontPrediction | null): number[] | undefined => {
  const tc = prediction?.textColor
  if (!tc || tc.length < 3) return undefined
  return [clampByte(tc[0]), clampByte(tc[1]), clampByte(tc[2]), 255]
}

/** Mirrors renderer precedence: explicit style color → predicted color → black. */
export const effectiveTextColor = (
  style?: TextStyle | null,
  prediction?: FontPrediction | null,
): number[] => style?.color ?? predictionColor(prediction) ?? DEFAULT_TEXT_COLOR

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
}

/**
 * Merge updates over a block's existing style into a full style. `color` has
 * no "absent" representation in a stored style (the renderer only consults
 * the prediction when the block has no style at all), so resetting it writes
 * the predicted color explicitly.
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
      ? (updates.color ?? predictionColor(prediction) ?? DEFAULT_TEXT_COLOR)
      : effectiveTextColor(current, prediction),
  effect: 'effect' in updates ? (updates.effect ?? null) : (current?.effect ?? null),
  stroke: 'stroke' in updates ? (updates.stroke ?? null) : (current?.stroke ?? null),
  textAlign: 'textAlign' in updates ? (updates.textAlign ?? null) : (current?.textAlign ?? null),
})
