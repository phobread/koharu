import type {
  TextAlign,
  TextFillGradient,
  TextShaderEffect,
  TextStrokeStyle,
  TextStyle,
} from '@/lib/api/schemas'

/**
 * Shared helpers for building explicit per-block `TextStyle` patches. The
 * scene only stores a style when the user overrides something. Since scene
 * format v4, `color` is genuinely optional: absent/null = automatic (the
 * renderer picks black/white from the page background), and any stored
 * colour — pure black and white included — is a manual pick honoured
 * verbatim.
 */

export const DEFAULT_TEXT_COLOR: number[] = [0, 0, 0, 255]

export const isManualTextColor = (color?: number[] | null): boolean =>
  color != null && color.length >= 3

/** Mirrors renderer intent: manual style colour wins; otherwise auto previews black. */
export const effectiveTextColor = (style?: TextStyle | null): number[] =>
  isManualTextColor(style?.color) ? style!.color! : DEFAULT_TEXT_COLOR

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

/** Merge updates over a block's existing style into a full style. */
export const mergeTextStyle = (
  current: TextStyle | null | undefined,
  updates: TextStyleUpdates,
): TextStyle => ({
  fontFamilies:
    'fontFamilies' in updates ? (updates.fontFamilies ?? []) : (current?.fontFamilies ?? []),
  fontSize: 'fontSize' in updates ? (updates.fontSize ?? null) : (current?.fontSize ?? null),
  color: 'color' in updates ? (updates.color ?? null) : (current?.color ?? null),
  effect: 'effect' in updates ? (updates.effect ?? null) : (current?.effect ?? null),
  stroke: 'stroke' in updates ? (updates.stroke ?? null) : (current?.stroke ?? null),
  textAlign: 'textAlign' in updates ? (updates.textAlign ?? null) : (current?.textAlign ?? null),
  gradient: 'gradient' in updates ? (updates.gradient ?? null) : (current?.gradient ?? null),
})
