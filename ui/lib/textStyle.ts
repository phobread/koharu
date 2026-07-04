import type { FontPrediction, TextStyle } from '@/lib/api/schemas'

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

/** Merge partial updates over a block's existing style into a full style. */
export const mergeTextStyle = (
  current: TextStyle | null | undefined,
  prediction: FontPrediction | null | undefined,
  updates: Partial<TextStyle>,
): TextStyle => ({
  fontFamilies: updates.fontFamilies ?? current?.fontFamilies ?? [],
  fontSize: updates.fontSize ?? current?.fontSize ?? null,
  color: updates.color ?? effectiveTextColor(current, prediction),
  effect: updates.effect ?? current?.effect ?? null,
  stroke: updates.stroke ?? current?.stroke ?? null,
  textAlign: updates.textAlign ?? current?.textAlign ?? null,
})
