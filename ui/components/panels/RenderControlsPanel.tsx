'use client'

import {
  AlignCenterIcon,
  AlignLeftIcon,
  AlignRightIcon,
  BlendIcon,
  BoldIcon,
  ItalicIcon,
  MinusIcon,
  MoveDownIcon,
  MoveRightIcon,
  PlusIcon,
  RotateCcwIcon,
  SquareIcon,
} from 'lucide-react'
import { type ComponentType, useMemo, useRef, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { ColorPicker } from '@/components/ui/color-picker'
import { FontSelect, useGoogleFontPreview } from '@/components/ui/font-select'
import { FontUploadButton } from '@/components/ui/font-upload-button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { VariantItem } from '@/components/ui/variant-item'
import {
  useCurrentPage,
  useSelectedTextNode,
  useSelectedTextNodes,
  useTextNodes,
  type TextNodeEntry,
} from '@/hooks/useCurrentPage'
import { fetchGoogleFont, useGetGoogleFontsCatalog, useListFonts } from '@/lib/api/default/default'
import type {
  FontFaceInfo,
  GradientDirection,
  Op,
  TextAlign,
  TextDirection,
  TextFillGradient,
  TextShaderEffect,
  TextStrokeStyle,
} from '@/lib/api/schemas'
import {
  findFontFace,
  getLocalizedFontLabel,
  normalizeFamilyName,
  STYLE_KEYWORDS,
  uniqueFontFaces,
} from '@/lib/font-utils'
import { applyOp, invalidateScene, queueAutoRender } from '@/lib/io/scene'
import { ops } from '@/lib/ops'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import {
  effectiveTextColor,
  isManualTextColor,
  mergeTextStyle,
  type TextStyleUpdates,
} from '@/lib/textStyle'
import { cn } from '@/lib/utils'

const DEFAULT_STROKE_WIDTH = 1.6
const MIN_STROKE_WIDTH = 0.2
const MAX_STROKE_WIDTH = 24
const STROKE_WIDTH_STEP = 0.1

const DEFAULT_FONT_FACES: FontFaceInfo[] = [
  {
    familyName: 'Arial',
    postScriptName: 'ArialMT',
    source: 'system',
    cached: true,
  },
]

const clampByte = (v: number) => Math.max(0, Math.min(255, Math.round(v)))
const clampStrokeWidth = (v: number) =>
  Number(Math.max(MIN_STROKE_WIDTH, Math.min(MAX_STROKE_WIDTH, v)).toFixed(1))

const colorToHex = (color: number[]) =>
  `#${color
    .slice(0, 3)
    .map((v) => clampByte(v).toString(16).padStart(2, '0'))
    .join('')}`

const hexToColor = (value: string, alpha: number): number[] => {
  const normalized = value.replace('#', '')
  if (normalized.length !== 6) return [0, 0, 0, clampByte(alpha)]
  const r = Number.parseInt(normalized.slice(0, 2), 16)
  const g = Number.parseInt(normalized.slice(2, 4), 16)
  const b = Number.parseInt(normalized.slice(4, 6), 16)
  if ([r, g, b].some((c) => Number.isNaN(c))) return [0, 0, 0, clampByte(alpha)]
  return [r, g, b, clampByte(alpha)]
}

const fallbackFontFace = (value?: string): FontFaceInfo | undefined => {
  const normalized = value?.trim()
  if (!normalized) return undefined
  return {
    familyName: normalized,
    postScriptName: normalized,
    source: 'system',
    cached: true,
  }
}

const normalizeStroke = (stroke?: TextStrokeStyle | null): TextStrokeStyle => ({
  enabled: stroke?.enabled ?? true,
  // null = automatic: the renderer contrasts the outline with the text colour.
  color: stroke?.color ?? null,
  widthPx: stroke?.widthPx ?? null,
})

// Mirrors the renderer's auto outline: contrast against the text colour
// (luminance 0.299r + 0.587g + 0.114b, threshold 128 → black, else white).
const contrastingStrokeColor = (textColor: number[]): [number, number, number, number] => {
  const [r = 0, g = 0, b = 0] = textColor
  return 0.299 * r + 0.587 * g + 0.114 * b > 128 ? [0, 0, 0, 255] : [255, 255, 255, 255]
}

const normalizeEffect = (effect?: TextShaderEffect | null): TextShaderEffect => ({
  bold: effect?.bold ?? false,
  italic: effect?.italic ?? false,
})

const hasExplicitColor = (node: TextNodeEntry) => isManualTextColor(node.data.style?.color)

/** Small ↺ button that clears an override back to the renderer's automatic value. */
function ResetToAutoButton({
  label,
  disabled,
  onClick,
  testId,
  className,
}: {
  label: string
  disabled?: boolean
  onClick: () => void
  testId: string
  className?: string
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type='button'
          variant='ghost'
          size='icon-sm'
          aria-label={label}
          data-testid={testId}
          disabled={disabled}
          className={cn('shrink-0 text-muted-foreground hover:text-foreground', className)}
          onClick={onClick}
        >
          <RotateCcwIcon className='size-3' />
        </Button>
      </TooltipTrigger>
      <TooltipContent side='bottom' sideOffset={4}>
        {label}
      </TooltipContent>
    </Tooltip>
  )
}

export function RenderControlsPanel() {
  const { t } = useTranslation()
  const page = useCurrentPage()
  const textNodes = useTextNodes()
  const selectedNode = useSelectedTextNode()
  const selectedNodes = useSelectedTextNodes()
  const { data: availableFonts = [] } = useListFonts()
  useGetGoogleFontsCatalog() // prefetch catalog so picker can decorate Google entries
  const appDefaultFont = usePreferencesStore((s) => s.defaultFont)
  const appDefaultFontSize = usePreferencesStore((s) => s.defaultFontSize)
  const setAppDefaultFontSize = usePreferencesStore((s) => s.setDefaultFontSize)
  const boxPadding = usePreferencesStore((s) => s.boxPadding)
  const setBoxPadding = usePreferencesStore((s) => s.setBoxPadding)
  const favoriteFonts = usePreferencesStore((s) => s.favoriteFonts)
  const toggleFavoriteFont = usePreferencesStore((s) => s.toggleFavoriteFont)
  const renderEffect = useEditorUiStore((s) => s.renderEffect)
  const setRenderEffect = useEditorUiStore((s) => s.setRenderEffect)
  const renderStroke = useEditorUiStore((s) => s.renderStroke)
  const setRenderStroke = useEditorUiStore((s) => s.setRenderStroke)

  const sortedFonts = useMemo(() => {
    return [...(availableFonts ?? [])].sort((a, b) => a.familyName.localeCompare(b.familyName))
  }, [availableFonts])

  const sectionRef = useRef<HTMLDivElement>(null)
  const [sectionWidth, setSectionWidth] = useState<number>(0)

  useEffect(() => {
    if (!sectionRef.current) return
    const observer = new ResizeObserver((entries) => {
      setSectionWidth(entries[0].contentRect.width)
    })
    observer.observe(sectionRef.current)
    return () => observer.disconnect()
  }, [])

  const firstNode = textNodes[0]
  const hasNodes = textNodes.length > 0

  const fontCandidates = useMemo(
    () =>
      uniqueFontFaces(
        [
          ...sortedFonts,
          ...(appDefaultFont ? [fallbackFontFace(appDefaultFont)] : []),
          ...(selectedNode?.data.style?.fontFamilies?.slice(0, 1)?.map(fallbackFontFace) ?? []),
          ...(firstNode?.data.style?.fontFamilies?.slice(0, 1)?.map(fallbackFontFace) ?? []),
          ...DEFAULT_FONT_FACES,
        ].filter((v): v is FontFaceInfo => !!v),
      ),
    [sortedFonts, appDefaultFont, selectedNode?.id, selectedNode?.data.style?.fontFamilies],
  )

  const currentFontCandidate =
    selectedNode?.data.style?.fontFamilies?.[0] ??
    appDefaultFont ??
    firstNode?.data.style?.fontFamilies?.[0] ??
    (hasNodes ? fontCandidates[0]?.postScriptName : '')
  const currentFontFace = useMemo(() => {
    return (
      findFontFace(fontCandidates, currentFontCandidate) || fallbackFontFace(currentFontCandidate)
    )
  }, [fontCandidates, currentFontCandidate])

  const currentFont = currentFontFace?.postScriptName ?? ''
  const currentFontFamilyName = useMemo(() => {
    if (!currentFontFace) return undefined
    return normalizeFamilyName(currentFontFace.familyName)
  }, [currentFontFace])

  const familyOptions = useMemo(() => {
    const families = new Map<string, FontFaceInfo>()
    for (const f of fontCandidates) {
      const name = normalizeFamilyName(f.familyName)
      if (!families.has(name) || f.postScriptName === name) {
        families.set(name, { ...f, familyName: name }) // Use normalized name for the option
      }
    }
    return Array.from(families.values()).sort((a, b) => a.familyName.localeCompare(b.familyName))
  }, [fontCandidates])

  const currentVariants = useMemo(() => {
    const name = normalizeFamilyName(currentFontFamilyName ?? '').toLowerCase()
    if (!name) return []
    const nameNoSpace = name.replace(/\s+/g, '')
    return fontCandidates.filter((f) => {
      const fFamilyNorm = normalizeFamilyName(f.familyName).toLowerCase()
      if (fFamilyNorm === name) return true

      const fPsNorm = f.postScriptName.toLowerCase()
      if (fPsNorm.includes(nameNoSpace)) {
        // Ensure the family part of the PS name is an EXACT match
        const familyPart = f.postScriptName
          .split(/[:\-_]/)[0]
          .replace(/[\s\-_]+/g, '')
          .toLowerCase()
        if (familyPart !== nameNoSpace) return false

        const rest = fPsNorm.replace(nameNoSpace, '')
        const isStyleSuffix =
          !rest ||
          /^[-_\s]/.test(rest) ||
          STYLE_KEYWORDS.some((k) => rest.toLowerCase().includes(k.toLowerCase()))

        if (isStyleSuffix) return true
      }
      return false
    })
  }, [fontCandidates, currentFontFamilyName])

  const currentVariantsWithLabels = useMemo(() => {
    if (!currentVariants) return []

    // First pass: generate all labels
    const mapped = currentVariants.map((v) => ({
      variant: v,
      label: getLocalizedFontLabel(v, t),
    }))

    // Second pass: identify duplicates
    return mapped.map((item) => {
      const isDuplicate =
        mapped.filter(
          (other) =>
            other.variant.postScriptName !== item.variant.postScriptName &&
            other.label === item.label,
        ).length > 0

      return {
        ...item,
        isDuplicate,
      }
    })
  }, [currentVariants, t])

  const selectedStyle = selectedNode?.data.style ?? firstNode?.data.style
  const colorSource = selectedNode ?? firstNode
  // Auto blocks show the colour the renderer actually painted (write-back),
  // so the swatch no longer guesses black when contrast picked white.
  const currentColor = effectiveTextColor(
    colorSource?.data.style,
    colorSource?.data.renderedTextColor,
  )
  const currentColorHex = colorToHex(currentColor)
  const currentStroke = normalizeStroke(selectedStyle?.stroke)
  // Auto stroke shows the colour the renderer would actually pick.
  const currentStrokeColorHex = colorToHex(
    currentStroke.color ?? contrastingStrokeColor(currentColor),
  )
  const currentStrokeWidth = currentStroke.widthPx ?? DEFAULT_STROKE_WIDTH
  // Gradient fill: `style.color` is the start colour, `gradient.to` the end.
  // No gradient stored = flat fill.
  const currentGradient = selectedStyle?.gradient ?? null
  const currentGradientToHex = colorToHex(currentGradient?.to ?? currentColor)
  const currentEffect = normalizeEffect(selectedStyle?.effect ?? renderEffect)
  // The scene only persists manual overrides in `style.fontSize`. Font detector
  // metadata describes the source text, not the renderer's current auto-fit size.
  const currentFontSize: number | undefined = selectedNode?.data.style?.fontSize ?? undefined
  // Size the renderer last used for the selected block (auto-fit result) —
  // shown as the "auto" placeholder and used as the stepping base so +/-
  // nudges start from the real size instead of an arbitrary constant.
  const renderedFontSize: number | undefined = selectedNode?.data.renderedFontSizePx ?? undefined

  const writingDirectionTargets = selectedNodes.length > 0 ? selectedNodes : textNodes
  const writingDirectionValues = new Set(
    writingDirectionTargets.map((node) => node.data.writingDirection ?? 'auto'),
  )
  const currentWritingDirection: TextDirection | 'auto' | undefined =
    writingDirectionValues.size === 1 ? writingDirectionValues.values().next().value : undefined

  const effectiveAlign: TextAlign =
    selectedNode?.data.style?.textAlign ??
    firstNode?.data.style?.textAlign ??
    (selectedNode?.data.translation ? 'center' : 'left')

  const currentFontPreviewState = useGoogleFontPreview(
    currentFontFace?.source === 'google' ? currentFont : (currentFontFamilyName ?? ''),
    currentFontFace?.source ?? 'system',
    true,
  )

  // ---------------------------------------------------------------------------
  // Mutations
  // ---------------------------------------------------------------------------

  const buildStyleOp = (n: TextNodeEntry, updates: TextStyleUpdates): Op => {
    const nextStyle = mergeTextStyle(n.data.style, updates)
    return ops.updateNode(page!.id, n.id, {
      data: { text: { style: nextStyle } } as never,
    })
  }

  const applyStyleToNodes = (nodes: TextNodeEntry[], updates: TextStyleUpdates, label: string) => {
    if (!page || nodes.length === 0) return
    void (async () => {
      const op =
        nodes.length === 1
          ? buildStyleOp(nodes[0], updates)
          : ops.batch(
              label,
              nodes.map((n) => buildStyleOp(n, updates)),
            )
      await applyOp(op)
      queueAutoRender(page.id)
    })()
  }

  const applyStyleToSelected = (updates: TextStyleUpdates): boolean => {
    if (selectedNodes.length === 0) return false
    applyStyleToNodes(selectedNodes, updates, 'Multi-block style update')
    return true
  }

  const applyStyleToAll = (updates: TextStyleUpdates) => {
    applyStyleToNodes(textNodes, updates, 'Bulk style update')
  }

  // ── Reset to auto (model-predicted) ─────────────────────────────────────
  // With blocks selected, resets clear those blocks' overrides; with nothing
  // selected they clear the global default AND every block's override, so
  // the whole page genuinely returns to auto. Only blocks that actually hold
  // an override are patched — touching a clean block would materialise an
  // explicit style (freezing its predicted color) for no reason.

  const resetTargets = selectedNodes.length > 0 ? selectedNodes : textNodes

  const canResetFontSize =
    resetTargets.some((n) => n.data.style?.fontSize != null) ||
    (selectedNodes.length === 0 && appDefaultFontSize !== undefined)
  const resetFontSizeToAuto = () => {
    if (selectedNodes.length === 0) setAppDefaultFontSize(undefined)
    const targets = resetTargets.filter((n) => n.data.style?.fontSize != null)
    if (targets.length > 0) applyStyleToNodes(targets, { fontSize: null }, 'Reset font size')
    else if (page) queueAutoRender(page.id)
  }

  const canResetStroke =
    resetTargets.some((n) => n.data.style?.stroke != null) ||
    (selectedNodes.length === 0 && renderStroke !== undefined)
  const resetStrokeToAuto = () => {
    if (selectedNodes.length === 0) setRenderStroke(undefined)
    const targets = resetTargets.filter((n) => n.data.style?.stroke != null)
    if (targets.length > 0) applyStyleToNodes(targets, { stroke: null }, 'Reset outline')
    else if (page) queueAutoRender(page.id)
  }

  const applyGradient = (gradient: TextFillGradient | null) => {
    if (applyStyleToSelected({ gradient })) return
    applyStyleToAll({ gradient })
  }

  const canResetGradient = resetTargets.some((n) => n.data.style?.gradient != null)
  const resetGradientToAuto = () => {
    applyStyleToNodes(
      resetTargets.filter((n) => n.data.style?.gradient != null),
      { gradient: null },
      'Reset gradient',
    )
  }

  const canResetColor = resetTargets.some(hasExplicitColor)
  const resetColorToAuto = () => {
    applyStyleToNodes(resetTargets.filter(hasExplicitColor), { color: null }, 'Reset text color')
  }

  const commitCurrentFontColorIfImplicit = () => {
    const targets = selectedNodes.length > 0 ? selectedNodes : textNodes
    if (targets.every(hasExplicitColor)) return
    applyStyleToNodes(targets, { color: currentColor }, 'Explicit font color update')
  }

  const applyStrokeSetting = (nextStroke: TextStrokeStyle) => {
    if (applyStyleToSelected({ stroke: normalizeStroke(nextStroke) })) return
    setRenderStroke({
      enabled: nextStroke.enabled ?? true,
      // Absent = auto — don't materialise a colour the user never picked.
      color: (nextStroke.color ?? undefined) as [number, number, number, number] | undefined,
      widthPx: nextStroke.widthPx ?? undefined,
    })
    if (page) queueAutoRender(page.id)
  }

  const updateStrokeWidth = (value: number) => {
    applyStrokeSetting({ ...currentStroke, widthPx: clampStrokeWidth(value) })
  }

  // Font size: per-node when a block is selected; otherwise the global default
  // size (a cap applied to every block without an explicit size at render).
  const activeFontSize = selectedNode ? currentFontSize : appDefaultFontSize
  const applyFontSize = (size: number | undefined) => {
    if (selectedNode) {
      if (size === undefined) return
      applyStyleToSelected({ fontSize: size })
      return
    }
    setAppDefaultFontSize(size)
    if (page) queueAutoRender(page.id)
  }

  // Box padding is a global render default (insets text from each box edge to
  // stop glyphs/strokes clipping). Re-render so the change is visible at once.
  const updateBoxPadding = (px: number) => {
    setBoxPadding(px)
    if (page) queueAutoRender(page.id)
  }

  const applyWritingDirection = (direction: TextDirection | 'auto') => {
    if (!page || writingDirectionTargets.length === 0) return
    void (async () => {
      const inner = writingDirectionTargets.map((node) =>
        ops.updateText(page.id, node.id, {
          writingDirection: direction === 'auto' ? null : direction,
        }),
      )
      const op = inner.length === 1 ? inner[0] : ops.batch('Writing direction update', inner)
      await applyOp(op)
      queueAutoRender(page.id)
    })()
  }

  const effectItems: {
    key: 'italic' | 'bold'
    label: string
    Icon: ComponentType<{ className?: string }>
  }[] = [
    { key: 'italic', label: t('render.effectItalic'), Icon: ItalicIcon },
    { key: 'bold', label: t('render.effectBold'), Icon: BoldIcon },
  ]

  const textAlignItems: {
    value: TextAlign
    label: string
    Icon: ComponentType<{ className?: string }>
  }[] = [
    { value: 'left', label: t('render.alignLeft'), Icon: AlignLeftIcon },
    { value: 'center', label: t('render.alignCenter'), Icon: AlignCenterIcon },
    { value: 'right', label: t('render.alignRight'), Icon: AlignRightIcon },
  ]

  const scopeLabel =
    selectedNodes.length > 1
      ? t('render.fontScopeBlocksCount', { count: selectedNodes.length })
      : selectedNode
        ? t('render.fontScopeBlockIndex', {
            index: textNodes.findIndex((n) => n.id === selectedNode.id) + 1,
          })
        : t('render.fontScopeGlobal')
  const scopeToneClass = selectedNode
    ? 'border-primary/20 bg-primary/10 text-primary'
    : 'border-border/60 bg-muted text-muted-foreground'

  if (!page) {
    return (
      <div className='flex items-center justify-center py-6 text-xs text-muted-foreground'>
        {t('textBlocks.emptyPrompt')}
      </div>
    )
  }

  return (
    <div className='flex w-full min-w-0 flex-col gap-2'>
      {/* Scope */}
      <div className='flex items-center justify-end'>
        <span
          data-testid='render-scope-indicator'
          className={cn(
            'rounded-full border px-2 py-0.5 text-[10px] font-medium tracking-wide uppercase',
            scopeToneClass,
          )}
        >
          {scopeLabel}
        </span>
      </div>

      {/* Font + Color */}
      <div className='flex flex-col gap-0.5' ref={sectionRef}>
        <div className='flex items-baseline justify-between'>
          <span className='text-[10px] font-medium text-muted-foreground uppercase'>
            {t('render.fontLabel')}
          </span>
          <span className='text-[10px] font-medium text-muted-foreground uppercase'>
            {t('render.fontColorLabel')}
          </span>
        </div>
        <div className='flex min-w-0 items-center gap-1.5'>
          <div className='min-w-0 flex-[1.5]'>
            <FontSelect
              data-testid='render-font-select'
              value={currentFontFamilyName ?? ''}
              options={familyOptions}
              favoriteFonts={favoriteFonts}
              onToggleFavorite={toggleFavoriteFont}
              disabled={familyOptions.length === 0}
              placeholder={t('render.fontPlaceholder')}
              triggerStyle={
                currentFontFamilyName ? { fontFamily: currentFontFamilyName } : undefined
              }
              contentStyle={
                sectionWidth > 0 ? { width: sectionWidth, maxWidth: sectionWidth } : undefined
              }
              onChange={async (value) => {
                const familyVariants = fontCandidates.filter(
                  (f) => normalizeFamilyName(f.familyName) === value,
                )
                // Try to find Regular/400 first
                const regularFace =
                  familyVariants.find((f) => {
                    const ps = f.postScriptName.toLowerCase()
                    return ps.includes('regular') || ps.includes('400') || ps.includes(':400')
                  }) || familyVariants[0]

                const face = regularFace || findFontFace(fontCandidates, value)
                if (!face) return

                // Trigger fetch for Google Fonts if not cached
                if (face.source === 'google' && !face.cached) {
                  try {
                    await fetchGoogleFont(encodeURIComponent(face.postScriptName))
                    invalidateScene()
                  } catch (e) {
                    console.error('Failed to fetch font:', e)
                  }
                }

                if (selectedNode) {
                  applyStyleToSelected({ fontFamilies: [face.postScriptName] })
                  return
                }
                usePreferencesStore.getState().setDefaultFont(face.postScriptName)
                if (page) queueAutoRender(page.id)
              }}
            />
          </div>
          <FontUploadButton
            className='size-7'
            onUploaded={(postScriptName) => {
              if (selectedNode) {
                applyStyleToSelected({ fontFamilies: [postScriptName] })
                return
              }
              usePreferencesStore.getState().setDefaultFont(postScriptName)
              if (page) queueAutoRender(page.id)
            }}
          />
          {currentVariants && currentVariants.length > 1 && (
            <div className='min-w-0 flex-1'>
              <Select
                key={`${currentFontFamilyName}-${currentVariants.length}`}
                value={currentFont}
                onValueChange={async (value) => {
                  // Trigger fetch for Google Fonts if not cached
                  const variant = currentVariants.find((v) => v.postScriptName === value)
                  if (variant?.source === 'google' && !variant.cached) {
                    try {
                      await fetchGoogleFont(encodeURIComponent(value))
                      invalidateScene()
                    } catch (e) {
                      console.error('Failed to fetch font variant:', e)
                    }
                  }

                  if (selectedNode) {
                    applyStyleToSelected({ fontFamilies: [value] })
                    return
                  }
                  usePreferencesStore.getState().setDefaultFont(value)
                  if (page) queueAutoRender(page.id)
                }}
              >
                <SelectTrigger
                  className='h-7 w-full px-2 text-xs'
                  style={{
                    fontFamily:
                      currentFontPreviewState === 'ready'
                        ? `"${(currentFontFace?.source === 'google' ? currentFont : (currentFontFamilyName ?? '')).replace(':', '-')}"`
                        : undefined,
                  }}
                >
                  <SelectValue placeholder={t('render.fontStylePlaceholder')} />
                </SelectTrigger>
                <SelectContent
                  position='popper'
                  style={
                    sectionWidth > 0 ? { width: sectionWidth, maxWidth: sectionWidth } : undefined
                  }
                  className='overflow-hidden p-0'
                  align='start'
                  sideOffset={4}
                >
                  {currentVariantsWithLabels.map(({ variant, label, isDuplicate }) => (
                    <VariantItem
                      key={variant.postScriptName}
                      variant={variant}
                      label={
                        isDuplicate
                          ? `${label} (${variant.source === 'google' ? 'Google' : 'System'})`
                          : label
                      }
                    />
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          <ColorPicker
            value={currentColorHex}
            disabled={!hasNodes}
            triggerTestId='render-color-trigger'
            pickerTestId='render-color-picker'
            swatchTestId='render-color-swatch'
            inputTestId='render-color-input'
            pickButtonTestId='render-color-pick'
            pickButtonLabel={t('render.eyedropper')}
            onChange={(hex) => {
              const nextColor = hexToColor(hex, currentColor[3] ?? 255)
              if (applyStyleToSelected({ color: nextColor })) return
              applyStyleToAll({ color: nextColor })
            }}
            className='size-7'
          />
          <ResetToAutoButton
            label={t('render.resetToAuto')}
            disabled={!canResetColor}
            onClick={resetColorToAuto}
            testId='render-color-reset'
            className='size-7'
          />
        </div>
      </div>

      {/* Size / Effect / Align */}
      <div className='grid w-full grid-cols-[minmax(0,1fr)_auto_auto] items-end gap-x-1.5'>
        <span className='text-[10px] font-medium text-muted-foreground uppercase'>
          {t('render.fontSizeLabel')}
        </span>
        <span className='text-[10px] font-medium text-muted-foreground uppercase'>
          {t('render.effectLabel')}
        </span>
        <span className='text-[10px] font-medium text-muted-foreground uppercase'>
          {t('render.alignLabel')}
        </span>

        <div className='flex min-w-0 items-center gap-0.5'>
          <div className='flex min-w-0 flex-1 items-center rounded-md border border-input bg-background shadow-xs'>
            <Button
              type='button'
              variant='ghost'
              size='icon-sm'
              className='size-6 shrink-0 rounded-r-none border-r'
              onClick={() =>
                applyFontSize(
                  Math.max(6, Math.round((activeFontSize ?? renderedFontSize ?? 16) - 1)),
                )
              }
            >
              <MinusIcon className='size-3' />
            </Button>
            <Input
              type='number'
              step='1'
              min='6'
              max='300'
              inputMode='numeric'
              className='h-6 min-w-0 flex-1 [appearance:textfield] rounded-none border-0 px-0.5 text-center text-xs shadow-none focus-visible:ring-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'
              data-testid='render-font-size'
              value={activeFontSize !== undefined ? Math.round(activeFontSize) : ''}
              placeholder={
                selectedNode && renderedFontSize !== undefined
                  ? `auto (${Math.round(renderedFontSize)})`
                  : 'auto'
              }
              onChange={(event) => {
                const value = event.target.value.trim()
                if (value === '') {
                  // Clearing only makes sense for the global default (→ auto-fit).
                  if (!selectedNode) applyFontSize(undefined)
                  return
                }
                const parsed = Number.parseInt(value, 10)
                if (!Number.isFinite(parsed) || parsed < 1) return
                applyFontSize(Math.min(300, parsed))
              }}
            />
            <Button
              type='button'
              variant='ghost'
              size='icon-sm'
              className='size-6 shrink-0 rounded-l-none border-l'
              onClick={() =>
                applyFontSize(
                  Math.min(300, Math.round((activeFontSize ?? renderedFontSize ?? 16) + 1)),
                )
              }
            >
              <PlusIcon className='size-3' />
            </Button>
          </div>
          <ResetToAutoButton
            label={t('render.resetToAuto')}
            disabled={!canResetFontSize}
            onClick={resetFontSizeToAuto}
            testId='render-font-size-reset'
            className='size-6'
          />
        </div>

        <div className='flex items-center gap-0.5'>
          {effectItems.map((item) => {
            const active = currentEffect[item.key]
            const Icon = item.Icon
            return (
              <Tooltip key={item.key}>
                <TooltipTrigger asChild>
                  <Button
                    variant='outline'
                    size='icon-sm'
                    aria-label={item.label}
                    data-testid={`render-effect-toggle-${item.key}`}
                    className={cn(
                      'size-6 shrink-0',
                      active &&
                        'border-primary bg-primary text-primary-foreground hover:bg-primary/90',
                    )}
                    onClick={() => {
                      const nextEffect: TextShaderEffect = {
                        ...currentEffect,
                        [item.key]: !active,
                      }
                      if (applyStyleToSelected({ effect: nextEffect })) return
                      setRenderEffect({
                        bold: nextEffect.bold ?? false,
                        italic: nextEffect.italic ?? false,
                      })
                      if (page) queueAutoRender(page.id)
                    }}
                  >
                    <Icon className='size-3' />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side='bottom' sideOffset={4}>
                  {item.label}
                </TooltipContent>
              </Tooltip>
            )
          })}
        </div>

        <div className='flex items-center gap-0.5'>
          {textAlignItems.map((item) => {
            const active = effectiveAlign === item.value
            const Icon = item.Icon
            return (
              <Tooltip key={item.value}>
                <TooltipTrigger asChild>
                  <Button
                    variant='outline'
                    size='icon-sm'
                    aria-label={item.label}
                    data-testid={`render-align-${item.value}`}
                    disabled={!hasNodes}
                    className={cn(
                      'size-6 shrink-0',
                      active &&
                        'border-primary bg-primary text-primary-foreground hover:bg-primary/90',
                    )}
                    onClick={() => {
                      if (applyStyleToSelected({ textAlign: item.value })) return
                      applyStyleToAll({ textAlign: item.value })
                    }}
                  >
                    <Icon className='size-3' />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side='bottom' sideOffset={4}>
                  {item.label}
                </TooltipContent>
              </Tooltip>
            )
          })}
        </div>
      </div>

      {/* Writing direction */}
      <div className='flex flex-col gap-0.5'>
        <span className='text-[10px] font-medium text-muted-foreground uppercase'>
          {t('render.writingDirectionLabel')}
        </span>
        <div className='grid grid-cols-3 gap-1'>
          {(
            [
              {
                value: 'auto',
                label: t('render.writingDirectionAuto'),
                Icon: RotateCcwIcon,
              },
              {
                value: 'horizontal',
                label: t('render.writingDirectionHorizontal'),
                Icon: MoveRightIcon,
              },
              {
                value: 'vertical',
                label: t('render.writingDirectionVertical'),
                Icon: MoveDownIcon,
              },
            ] as const
          ).map(({ value, label, Icon }) => {
            const active = currentWritingDirection === value
            return (
              <Tooltip key={value}>
                <TooltipTrigger asChild>
                  <Button
                    type='button'
                    variant='outline'
                    size='sm'
                    aria-label={label}
                    aria-pressed={active}
                    data-testid={`render-writing-${value}`}
                    disabled={!hasNodes}
                    className={cn(
                      'h-7 min-w-0 gap-1 px-2 text-[11px]',
                      active &&
                        'border-primary bg-primary text-primary-foreground hover:bg-primary/90',
                    )}
                    onClick={() => applyWritingDirection(value)}
                  >
                    <Icon className='size-3 shrink-0' />
                    <span className='truncate'>{label}</span>
                  </Button>
                </TooltipTrigger>
                <TooltipContent side='bottom' sideOffset={4}>
                  {label}
                </TooltipContent>
              </Tooltip>
            )
          })}
        </div>
      </div>

      {/* Border / Stroke */}
      <div className='flex flex-col gap-0.5'>
        <span className='text-[10px] font-medium text-muted-foreground uppercase'>
          {t('render.effectBorder')}
        </span>
        <div className='flex min-w-0 items-center gap-1'>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant='outline'
                size='icon-sm'
                data-testid='render-stroke-enable'
                className={cn(
                  'size-7 shrink-0',
                  currentStroke.enabled &&
                    'border-primary bg-primary text-primary-foreground hover:bg-primary/90',
                )}
                onClick={() =>
                  applyStrokeSetting({ ...currentStroke, enabled: !currentStroke.enabled })
                }
              >
                <SquareIcon className='size-3.5' />
              </Button>
            </TooltipTrigger>
            <TooltipContent side='bottom' sideOffset={4}>
              {t('render.effectBorder')}
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <div>
                <ColorPicker
                  value={currentStrokeColorHex}
                  disabled={!hasNodes}
                  triggerTestId='render-stroke-color-trigger'
                  pickerTestId='render-stroke-color-picker'
                  swatchTestId='render-stroke-color-swatch'
                  inputTestId='render-stroke-color-input'
                  pickButtonTestId='render-stroke-color-pick'
                  pickButtonLabel={t('render.eyedropper')}
                  onChange={(hex) => {
                    applyStrokeSetting({
                      ...currentStroke,
                      color: hexToColor(hex, currentStroke.color?.[3] ?? 255),
                    })
                  }}
                  className='size-7'
                />
              </div>
            </TooltipTrigger>
            <TooltipContent side='bottom' sideOffset={4}>
              {t('render.strokeColorLabel')}
            </TooltipContent>
          </Tooltip>

          <div className='flex min-w-0 flex-1 items-center rounded-md border border-input bg-background shadow-xs'>
            <Button
              type='button'
              variant='ghost'
              size='icon-sm'
              className='size-7 shrink-0 rounded-r-none border-r'
              onClick={() => updateStrokeWidth(currentStrokeWidth - STROKE_WIDTH_STEP)}
            >
              <MinusIcon className='size-3' />
            </Button>
            <Input
              type='number'
              step={String(STROKE_WIDTH_STEP)}
              min={String(MIN_STROKE_WIDTH)}
              max={String(MAX_STROKE_WIDTH)}
              inputMode='decimal'
              className='h-7 min-w-0 flex-1 [appearance:textfield] rounded-none border-0 px-1 text-center text-xs shadow-none focus-visible:ring-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'
              data-testid='render-stroke-width'
              value={
                Number.isFinite(currentStrokeWidth) ? currentStrokeWidth : DEFAULT_STROKE_WIDTH
              }
              onChange={(event) => {
                const parsed = Number.parseFloat(event.target.value)
                if (!Number.isFinite(parsed)) return
                updateStrokeWidth(parsed)
              }}
            />
            <Button
              type='button'
              variant='ghost'
              size='icon-sm'
              className='size-7 shrink-0 rounded-l-none border-l'
              onClick={() => updateStrokeWidth(currentStrokeWidth + STROKE_WIDTH_STEP)}
            >
              <PlusIcon className='size-3' />
            </Button>
          </div>
          <ResetToAutoButton
            label={t('render.resetToAuto')}
            disabled={!canResetStroke}
            onClick={resetStrokeToAuto}
            testId='render-stroke-reset'
            className='size-7'
          />
        </div>
      </div>

      {/* Gradient fill — the text fades from the font colour into a second
          colour, left→right or top→bottom. The outline keeps its own colour. */}
      <div className='flex flex-col gap-0.5'>
        <span className='text-[10px] font-medium text-muted-foreground uppercase'>
          {t('render.gradientLabel')}
        </span>
        <div className='flex min-w-0 items-center gap-1'>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant='outline'
                size='icon-sm'
                aria-label={t('render.gradientLabel')}
                data-testid='render-gradient-enable'
                disabled={!hasNodes}
                className={cn(
                  'size-7 shrink-0',
                  currentGradient &&
                    'border-primary bg-primary text-primary-foreground hover:bg-primary/90',
                )}
                onClick={() => {
                  if (currentGradient) {
                    applyGradient(null)
                    return
                  }
                  commitCurrentFontColorIfImplicit()
                  applyGradient({ to: currentColor, direction: 'horizontal' })
                }}
              >
                <BlendIcon className='size-3.5' />
              </Button>
            </TooltipTrigger>
            <TooltipContent side='bottom' sideOffset={4}>
              {t('render.gradientLabel')}
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <div>
                <ColorPicker
                  value={currentGradientToHex}
                  disabled={!hasNodes || !currentGradient}
                  triggerTestId='render-gradient-color-trigger'
                  pickerTestId='render-gradient-color-picker'
                  swatchTestId='render-gradient-color-swatch'
                  inputTestId='render-gradient-color-input'
                  pickButtonTestId='render-gradient-color-pick'
                  pickButtonLabel={t('render.eyedropper')}
                  onChange={(hex) => {
                    applyGradient({
                      to: hexToColor(hex, (currentGradient?.to ?? currentColor)[3] ?? 255),
                      direction: currentGradient?.direction ?? 'horizontal',
                    })
                  }}
                  className='size-7'
                />
              </div>
            </TooltipTrigger>
            <TooltipContent side='bottom' sideOffset={4}>
              {t('render.gradientEndColorLabel')}
            </TooltipContent>
          </Tooltip>

          <div className='flex flex-1 items-center gap-0.5'>
            {(
              [
                {
                  value: 'horizontal',
                  label: t('render.gradientHorizontal'),
                  Icon: MoveRightIcon,
                },
                { value: 'vertical', label: t('render.gradientVertical'), Icon: MoveDownIcon },
              ] as {
                value: GradientDirection
                label: string
                Icon: ComponentType<{ className?: string }>
              }[]
            ).map(({ value, label, Icon }) => (
              <Tooltip key={value}>
                <TooltipTrigger asChild>
                  <Button
                    variant='outline'
                    size='icon-sm'
                    aria-label={label}
                    data-testid={`render-gradient-direction-${value}`}
                    disabled={!hasNodes || !currentGradient}
                    className={cn(
                      'size-7 shrink-0',
                      currentGradient?.direction === value &&
                        'border-primary bg-primary text-primary-foreground hover:bg-primary/90',
                    )}
                    onClick={() => {
                      if (!currentGradient) return
                      applyGradient({ to: currentGradient.to, direction: value })
                    }}
                  >
                    <Icon className='size-3.5' />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side='bottom' sideOffset={4}>
                  {label}
                </TooltipContent>
              </Tooltip>
            ))}
          </div>
          <ResetToAutoButton
            label={t('render.resetToAuto')}
            disabled={!canResetGradient}
            onClick={resetGradientToAuto}
            testId='render-gradient-reset'
            className='size-7'
          />
        </div>
      </div>

      {/* Box padding (global render default) — insets text from each box edge
          so glyphs/strokes don't clip at the border. */}
      <div className='flex flex-col gap-0.5'>
        <span className='text-[10px] font-medium text-muted-foreground uppercase'>
          {t('render.boxPadding', { defaultValue: 'Box padding' })}
        </span>
        <div className='flex min-w-0 items-center rounded-md border border-input bg-background shadow-xs'>
          <Button
            type='button'
            variant='ghost'
            size='icon-sm'
            className='size-7 shrink-0 rounded-r-none border-r'
            onClick={() => updateBoxPadding(Math.max(0, boxPadding - 1))}
          >
            <MinusIcon className='size-3' />
          </Button>
          <Input
            type='number'
            step='1'
            min='0'
            max='64'
            inputMode='numeric'
            className='h-7 min-w-0 flex-1 [appearance:textfield] rounded-none border-0 px-1 text-center text-xs shadow-none focus-visible:ring-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'
            data-testid='render-box-padding'
            value={Number.isFinite(boxPadding) ? boxPadding : 0}
            onChange={(event) => {
              const parsed = Number.parseInt(event.target.value, 10)
              if (!Number.isFinite(parsed)) return
              updateBoxPadding(Math.max(0, Math.min(64, parsed)))
            }}
          />
          <Button
            type='button'
            variant='ghost'
            size='icon-sm'
            className='size-7 shrink-0 rounded-l-none border-l'
            onClick={() => updateBoxPadding(Math.min(64, boxPadding + 1))}
          >
            <PlusIcon className='size-3' />
          </Button>
        </div>
      </div>
    </div>
  )
}
