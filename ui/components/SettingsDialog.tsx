'use client'

import { useQueryClient } from '@tanstack/react-query'
import {
  SunIcon,
  MoonIcon,
  MonitorIcon,
  CheckCircleIcon,
  AlertCircleIcon,
  LoaderIcon,
  PaletteIcon,
  KeyIcon,
  HardDriveIcon,
  InfoIcon,
  CpuIcon,
  KeyboardIcon,
  SaveIcon,
  RotateCcwIcon,
  AlertTriangleIcon,
  CopyIcon,
  ExternalLinkIcon,
  LogInIcon,
  LogOutIcon,
  SparklesIcon,
  TypeIcon,
  BoldIcon,
  ItalicIcon,
  MinusIcon,
  PlusIcon,
} from 'lucide-react'
import { useTheme } from 'next-themes'
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from '@/components/ui/accordion'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { ColorPicker } from '@/components/ui/color-picker'
import { Dialog, DialogContent, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { FontSelect } from '@/components/ui/font-select'
import { Input } from '@/components/ui/input'
import { Kbd } from '@/components/ui/kbd'
import { Label } from '@/components/ui/label'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useUpdater, type UpdaterStatus } from '@/components/Updater'
import {
  getCatalog as getLlmCatalog,
  getConfig,
  getEngineCatalog,
  getGetCatalogQueryKey as getGetLlmCatalogQueryKey,
  getMeta,
  patchConfig,
  setProviderSecret,
  clearProviderSecret,
  deleteCodexSession,
  getGetCodexAuthStatusQueryKey,
  startCodexDeviceLogin,
  useGetCodexAuthStatus,
  useGetGoogleFontsCatalog,
  useListFonts,
} from '@/lib/api/default/default'
import type {
  AppConfig,
  ConfigPatch,
  CodexDeviceLogin,
  EngineCatalog as GetEngineCatalog200,
  FontFaceInfo,
  LlmProviderCatalog,
  ProviderConfig,
} from '@/lib/api/schemas'
import { isTauri, openExternalUrl } from '@/lib/backend'
import { normalizeFamilyName } from '@/lib/font-utils'
import { supportedLanguages } from '@/lib/i18n'
import { queueAutoRender } from '@/lib/io/scene'
import {
  areShortcutsEqual,
  formatShortcut,
  formatModifierCombination,
  getPlatform,
  isKeyBlocked,
  isModifierKey,
} from '@/lib/shortcutUtils'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'
import type { RenderStroke } from '@/lib/types'
import { cn } from '@/lib/utils'

// Dialog state models `AppConfig` (what `GET /config` returns — snake_case).
// But `PATCH /config` expects a `ConfigPatch` with camelCase fields because
// the patch schema derives `rename_all = "camelCase"` serde attrs. Translate
// at the boundary so the dialog internals stay unified.
type UpdateConfigBody = AppConfig

function withProviderBaseUrl(
  providers: ProviderConfig[] | undefined,
  id: string,
  baseUrl: string | null,
): ProviderConfig[] {
  const next = [...(providers ?? [])]
  const idx = next.findIndex((p) => p.id === id)
  if (idx >= 0) next[idx] = { ...next[idx], base_url: baseUrl }
  else next.push({ id, base_url: baseUrl })
  return next
}

function appConfigToPatch(cfg: AppConfig): ConfigPatch {
  const patch: ConfigPatch = {}
  if (cfg.data?.path) {
    patch.data = { path: cfg.data.path }
  }
  if (cfg.http) {
    patch.http = {
      connectTimeout: cfg.http.connect_timeout,
      readTimeout: cfg.http.read_timeout,
      maxRetries: cfg.http.max_retries,
    }
  }
  if (cfg.pipeline) {
    patch.pipeline = {
      detector: cfg.pipeline.detector,
      fontDetector: cfg.pipeline.font_detector,
      segmenter: cfg.pipeline.segmenter,
      bubbleSegmenter: cfg.pipeline.bubble_segmenter,
      ocr: cfg.pipeline.ocr,
      translator: cfg.pipeline.translator,
      inpainter: cfg.pipeline.inpainter,
      renderer: cfg.pipeline.renderer,
    }
  }
  if (cfg.providers) {
    patch.providers = cfg.providers.map((p) => ({
      id: p.id,
      baseUrl: p.base_url ?? null,
      apiKey: p.api_key ?? null,
    }))
  }
  return patch
}

async function updateConfig(next: UpdateConfigBody): Promise<AppConfig> {
  return (await patchConfig(appConfigToPatch(next))) as AppConfig
}

const GITHUB_REPO = 'mayocream/koharu'

const TABS = [
  { id: 'appearance', icon: PaletteIcon, labelKey: 'settings.appearance' },
  { id: 'rendering', icon: TypeIcon, labelKey: 'settings.textDefaults' },
  { id: 'engines', icon: CpuIcon, labelKey: 'settings.engines' },
  { id: 'providers', icon: KeyIcon, labelKey: 'settings.apiKeys' },
  { id: 'ai', icon: SparklesIcon, labelKey: 'settings.ai' },
  { id: 'keybinds', icon: KeyboardIcon, labelKey: 'settings.keybinds' },
  { id: 'runtime', icon: HardDriveIcon, labelKey: 'settings.runtime' },
  { id: 'about', icon: InfoIcon, labelKey: 'settings.about' },
] as const

export type TabId = (typeof TABS)[number]['id']

type SettingsDialogProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  defaultTab?: TabId
}

const DEFAULT_HTTP_CONNECT_TIMEOUT = 20
const DEFAULT_HTTP_READ_TIMEOUT = 300
const DEFAULT_HTTP_MAX_RETRIES = 3

export function SettingsDialog({
  open,
  onOpenChange,
  defaultTab = 'appearance',
}: SettingsDialogProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [tab, setTab] = useState<TabId>(defaultTab)
  useEffect(() => {
    if (open) setTab(defaultTab)
  }, [defaultTab, open])

  const [appConfig, setAppConfig] = useState<UpdateConfigBody | null>(null)
  const committedConfigRef = useRef<AppConfig | null>(null)
  const configQueueRef = useRef<Promise<void>>(Promise.resolve())
  const hydratedForOpenRef = useRef(false)
  const [baseUrlDrafts, setBaseUrlDrafts] = useState<Record<string, string>>({})
  const [providerCatalogs, setProviderCatalogs] = useState<LlmProviderCatalog[]>([])
  const [apiKeyDrafts, setApiKeyDrafts] = useState<Record<string, string>>({})
  // Per-provider save/clear failure, shown inline. Cleared when the user edits
  // the field or a later attempt succeeds.
  const [apiKeySaveErrors, setApiKeySaveErrors] = useState<Record<string, string>>({})
  const [dataPathDraft, setDataPathDraft] = useState('')
  const [httpConnectTimeoutDraft, setHttpConnectTimeoutDraft] = useState('')
  const [httpReadTimeoutDraft, setHttpReadTimeoutDraft] = useState('')
  const [httpMaxRetriesDraft, setHttpMaxRetriesDraft] = useState('')
  const [storageSettingsError, setStorageSettingsError] = useState<string | null>(null)
  const [isSavingStorageSettings, setIsSavingStorageSettings] = useState(false)
  const [engineCatalog, setEngineCatalog] = useState<GetEngineCatalog200 | null>(null)
  const [appVersion, setAppVersion] = useState<string>()
  const updater = useUpdater()

  const applyServerConfig = (cfg: AppConfig) => {
    committedConfigRef.current = cfg
    setAppConfig(cfg)
  }

  useEffect(() => {
    if (!open) return
    void (async () => {
      try {
        const [config, catalog, engines] = await Promise.all([
          getConfig(),
          getLlmCatalog(),
          getEngineCatalog(),
        ])
        applyServerConfig(config)
        setProviderCatalogs(catalog.providers)
        setEngineCatalog(engines)
      } catch {}
    })()
  }, [open])

  useEffect(() => {
    if (!open) return
    let cancelled = false
    void (async () => {
      try {
        const meta = await getMeta()
        if (cancelled) return
        setAppVersion(meta.version)
      } catch {
        return
      }
    })()
    return () => {
      cancelled = true
    }
  }, [open])

  const checkForUpdates = updater.checkForUpdates
  useEffect(() => {
    if (!open || !isTauri()) return
    void checkForUpdates()
  }, [open, checkForUpdates])

  useEffect(() => {
    if (!open) hydratedForOpenRef.current = false
  }, [open])

  useEffect(() => {
    if (!open || !appConfig?.data || hydratedForOpenRef.current) return
    hydratedForOpenRef.current = true
    setDataPathDraft(appConfig.data.path)
    setHttpConnectTimeoutDraft(
      String(appConfig.http?.connect_timeout ?? DEFAULT_HTTP_CONNECT_TIMEOUT),
    )
    setHttpReadTimeoutDraft(String(appConfig.http?.read_timeout ?? DEFAULT_HTTP_READ_TIMEOUT))
    setHttpMaxRetriesDraft(String(appConfig.http?.max_retries ?? DEFAULT_HTTP_MAX_RETRIES))
    setStorageSettingsError(null)
    const drafts: Record<string, string> = {}
    for (const p of appConfig.providers ?? []) drafts[p.id] = p.base_url ?? ''
    setBaseUrlDrafts(drafts)
  }, [open, appConfig])

  const enqueueConfigMutation = (mutate: (base: AppConfig) => AppConfig): Promise<boolean> => {
    const run = configQueueRef.current.then(async () => {
      const base = committedConfigRef.current
      if (!base) return false
      try {
        const next = mutate(base)
        const saved = await updateConfig(next)
        const catalog = await getLlmCatalog()
        applyServerConfig(saved)
        setProviderCatalogs(catalog.providers)
        queryClient.invalidateQueries({ queryKey: getGetLlmCatalogQueryKey() })
        return true
      } catch {
        return false
      }
    })
    configQueueRef.current = run.then(
      () => undefined,
      () => undefined,
    )
    return run
  }

  // Reload config + provider catalog after a dedicated secret write. Best-effort
  // and cosmetic: the secret is already stored server-side by the time this
  // runs, so a failed refresh only leaves the placeholder/status stale until the
  // dialog reopens — never a lost key.
  const refreshConfigAndCatalog = async () => {
    try {
      const [cfg, catalog] = await Promise.all([getConfig(), getLlmCatalog()])
      applyServerConfig(cfg)
      setProviderCatalogs(catalog.providers)
      queryClient.invalidateQueries({ queryKey: getGetLlmCatalogQueryKey() })
    } catch {
      // leave existing state in place
    }
  }

  const handleApplyStorageSettings = async () => {
    if (!appConfig) return
    const path = dataPathDraft.trim()
    if (!path) {
      setStorageSettingsError('Required')
      return
    }
    const connectTimeout = Number.parseInt(httpConnectTimeoutDraft.trim(), 10)
    if (!Number.isInteger(connectTimeout) || connectTimeout <= 0) {
      setStorageSettingsError('Invalid HTTP connect timeout')
      return
    }
    const readTimeout = Number.parseInt(httpReadTimeoutDraft.trim(), 10)
    if (!Number.isInteger(readTimeout) || readTimeout <= 0) {
      setStorageSettingsError('Invalid HTTP read timeout')
      return
    }
    const maxRetries = Number.parseInt(httpMaxRetriesDraft.trim(), 10)
    if (!Number.isInteger(maxRetries) || maxRetries < 0) {
      setStorageSettingsError('Invalid HTTP max retries')
      return
    }

    setIsSavingStorageSettings(true)
    setStorageSettingsError(null)
    const ok = await enqueueConfigMutation((base) => ({
      ...base,
      data: { path },
      http: {
        connect_timeout: connectTimeout,
        read_timeout: readTimeout,
        max_retries: maxRetries,
      },
    }))
    setIsSavingStorageSettings(false)
    if (!ok) {
      setStorageSettingsError('Failed')
      return
    }
    if (!isTauri()) {
      setStorageSettingsError('Restart manually')
      return
    }
    try {
      const { relaunch } = await import('@tauri-apps/plugin-process')
      await relaunch()
    } catch {
      setStorageSettingsError('Restart manually')
    }
  }

  const storageSettingsUnchanged =
    dataPathDraft.trim() === appConfig?.data?.path &&
    httpConnectTimeoutDraft.trim() ===
      String(appConfig?.http?.connect_timeout ?? DEFAULT_HTTP_CONNECT_TIMEOUT) &&
    httpReadTimeoutDraft.trim() ===
      String(appConfig?.http?.read_timeout ?? DEFAULT_HTTP_READ_TIMEOUT) &&
    httpMaxRetriesDraft.trim() === String(appConfig?.http?.max_retries ?? DEFAULT_HTTP_MAX_RETRIES)

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='flex h-[600px] max-h-[85vh] w-[760px] max-w-[92vw] flex-col gap-0 overflow-hidden p-0'>
        <DialogTitle className='sr-only'>{t('settings.title')}</DialogTitle>
        <DialogDescription className='sr-only'>Settings</DialogDescription>

        <div className='flex h-full'>
          {/* Sidebar */}
          <nav className='flex w-[180px] shrink-0 flex-col gap-1 border-r border-border bg-muted/30 p-3'>
            <p className='mb-3 px-3 text-[10px] font-semibold tracking-widest text-muted-foreground uppercase'>
              {t('settings.title')}
            </p>
            {TABS.map(({ id, icon: Icon, labelKey }) => (
              <button
                key={id}
                onClick={() => setTab(id)}
                data-active={tab === id}
                className='flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-muted-foreground transition hover:text-foreground data-[active=true]:bg-accent data-[active=true]:text-accent-foreground'
              >
                <Icon className='size-4 shrink-0' />
                {t(labelKey)}
              </button>
            ))}
          </nav>

          {/* Content */}
          <ScrollArea className='min-h-0 flex-1'>
            <div className='p-6'>
              {tab === 'appearance' && <AppearancePane />}
              {tab === 'rendering' && <TextDefaultsPane />}
              {tab === 'engines' && engineCatalog && appConfig && (
                <EnginesPane
                  catalog={engineCatalog}
                  pipeline={appConfig.pipeline ?? {}}
                  onChange={(pipeline) => {
                    setAppConfig((cur) => (cur ? { ...cur, pipeline } : cur))
                    void enqueueConfigMutation((base) => ({ ...base, pipeline }))
                  }}
                />
              )}
              {tab === 'providers' && (
                <ProvidersPane
                  catalogs={providerCatalogs}
                  config={appConfig}
                  drafts={apiKeyDrafts}
                  baseUrlDrafts={baseUrlDrafts}
                  onBaseUrlChange={(id, v) => setBaseUrlDrafts((c) => ({ ...c, [id]: v }))}
                  onBaseUrlBlur={(id) => {
                    const value = (baseUrlDrafts[id] ?? '').trim()
                    void enqueueConfigMutation((base) => ({
                      ...base,
                      providers: withProviderBaseUrl(base.providers, id, value || null),
                    }))
                  }}
                  saveErrors={apiKeySaveErrors}
                  onApiKeyChange={(id, v) => {
                    setApiKeyDrafts((c) => ({ ...c, [id]: v }))
                    setApiKeySaveErrors((c) => {
                      if (!c[id]) return c
                      const n = { ...c }
                      delete n[id]
                      return n
                    })
                  }}
                  onSaveKey={(id) => {
                    // Write only this provider's secret via its dedicated
                    // endpoint — never round-trip the whole provider list (which
                    // could revert a concurrently-changed base_url or another
                    // provider). Capture the value now so a later edit can't
                    // change what gets saved.
                    const key = apiKeyDrafts[id]?.trim()
                    if (!key) return
                    void (async () => {
                      try {
                        await setProviderSecret(id, { secret: key })
                      } catch {
                        // Keep the typed key in the field on failure.
                        setApiKeySaveErrors((c) => ({
                          ...c,
                          [id]: t('settings.apiKeySaveFailed', {
                            defaultValue: 'Save failed — your key was not stored. Try again.',
                          }),
                        }))
                        return
                      }
                      setApiKeySaveErrors((c) => {
                        const n = { ...c }
                        delete n[id]
                        return n
                      })
                      setApiKeyDrafts((c) => {
                        const n = { ...c }
                        delete n[id]
                        return n
                      })
                      await refreshConfigAndCatalog()
                    })()
                  }}
                  onClearKey={(id) => {
                    void (async () => {
                      try {
                        await clearProviderSecret(id)
                      } catch {
                        setApiKeySaveErrors((c) => ({
                          ...c,
                          [id]: t('settings.apiKeyClearFailed', {
                            defaultValue: 'Clear failed — try again.',
                          }),
                        }))
                        return
                      }
                      setApiKeySaveErrors((c) => {
                        const n = { ...c }
                        delete n[id]
                        return n
                      })
                      setApiKeyDrafts((c) => {
                        const n = { ...c }
                        delete n[id]
                        return n
                      })
                      await refreshConfigAndCatalog()
                    })()
                  }}
                />
              )}
              {tab === 'ai' && <CodexSettingsPane />}
              {tab === 'runtime' && (
                <StoragePane
                  dataPath={dataPathDraft}
                  httpConnectTimeout={httpConnectTimeoutDraft}
                  httpReadTimeout={httpReadTimeoutDraft}
                  httpMaxRetries={httpMaxRetriesDraft}
                  error={storageSettingsError}
                  saving={isSavingStorageSettings}
                  unchanged={storageSettingsUnchanged}
                  onPathChange={(v) => {
                    setDataPathDraft(v)
                    setStorageSettingsError(null)
                  }}
                  onHttpConnectTimeoutChange={(v) => {
                    setHttpConnectTimeoutDraft(v)
                    setStorageSettingsError(null)
                  }}
                  onHttpReadTimeoutChange={(v) => {
                    setHttpReadTimeoutDraft(v)
                    setStorageSettingsError(null)
                  }}
                  onHttpMaxRetriesChange={(v) => {
                    setHttpMaxRetriesDraft(v)
                    setStorageSettingsError(null)
                  }}
                  onApply={() => void handleApplyStorageSettings()}
                />
              )}
              {tab === 'keybinds' && <KeybindsPane />}
              {tab === 'about' && (
                <AboutPane
                  version={appVersion}
                  latestVersion={updater.latestVersion}
                  status={updater.status}
                  isInstallingUpdate={updater.isInstalling}
                  onInstallUpdate={() => void updater.installUpdate()}
                />
              )}
            </div>
          </ScrollArea>
        </div>
      </DialogContent>
    </Dialog>
  )
}

// ── Appearance ────────────────────────────────────────────────────

const THEMES = [
  { value: 'light', icon: SunIcon, labelKey: 'settings.themeLight' },
  { value: 'dark', icon: MoonIcon, labelKey: 'settings.themeDark' },
  { value: 'system', icon: MonitorIcon, labelKey: 'settings.themeSystem' },
] as const

function AppearancePane() {
  const { t, i18n } = useTranslation()
  const { theme, setTheme } = useTheme()
  const locales = useMemo(() => supportedLanguages, [])
  return (
    <div className='space-y-8'>
      <Section title={t('settings.theme')}>
        <div className='grid grid-cols-3 gap-3'>
          {THEMES.map(({ value, icon: Icon, labelKey }) => (
            <button
              key={value}
              onClick={() => setTheme(value)}
              data-active={theme === value}
              className='flex flex-col items-center gap-2 rounded-xl border border-border bg-card px-4 py-4 text-muted-foreground transition hover:border-foreground/30 data-[active=true]:border-primary data-[active=true]:text-foreground'
            >
              <Icon className='size-5' />
              <span className='text-xs font-medium'>{t(labelKey)}</span>
            </button>
          ))}
        </div>
      </Section>

      <Section title={t('settings.language')}>
        <Select value={i18n.language} onValueChange={(v) => i18n.changeLanguage(v)}>
          <SelectTrigger className='w-full'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {locales.map((code) => (
              <SelectItem key={code} value={code}>
                {t(`menu.languages.${code}`, { defaultValue: code })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Section>
    </div>
  )
}

// ── Text defaults ─────────────────────────────────────────────────

const STROKE_DEFAULT_COLOR: [number, number, number, number] = [255, 255, 255, 255]
const STROKE_DEFAULT_WIDTH = 1.6

const colorToHex = (c: number[]) =>
  `#${c
    .slice(0, 3)
    .map((v) =>
      Math.max(0, Math.min(255, Math.round(v)))
        .toString(16)
        .padStart(2, '0'),
    )
    .join('')}`

const hexToColor = (hex: string, alpha: number): [number, number, number, number] => {
  const n = hex.replace('#', '')
  if (n.length !== 6) return [0, 0, 0, alpha]
  const r = Number.parseInt(n.slice(0, 2), 16)
  const g = Number.parseInt(n.slice(2, 4), 16)
  const b = Number.parseInt(n.slice(4, 6), 16)
  if ([r, g, b].some((v) => Number.isNaN(v))) return [0, 0, 0, alpha]
  return [r, g, b, alpha]
}

/**
 * Central place to set the global render defaults (font, size, bold/italic,
 * border, and text-edge padding). These are the same persisted values the
 * inline render panel edits when no block is selected; here they're grouped
 * so they're easy to find and adjust. Per-block overrides still win.
 */
function TextDefaultsPane() {
  const { t } = useTranslation()
  const { data: availableFonts = [] } = useListFonts()
  useGetGoogleFontsCatalog() // prefetch so Google families render with a preview

  const defaultFont = usePreferencesStore((s) => s.defaultFont)
  const setDefaultFont = usePreferencesStore((s) => s.setDefaultFont)
  const defaultFontSize = usePreferencesStore((s) => s.defaultFontSize)
  const setDefaultFontSize = usePreferencesStore((s) => s.setDefaultFontSize)
  const boxPadding = usePreferencesStore((s) => s.boxPadding)
  const setBoxPadding = usePreferencesStore((s) => s.setBoxPadding)
  const favoriteFonts = usePreferencesStore((s) => s.favoriteFonts)
  const toggleFavoriteFont = usePreferencesStore((s) => s.toggleFavoriteFont)
  const renderEffect = useEditorUiStore((s) => s.renderEffect)
  const setRenderEffect = useEditorUiStore((s) => s.setRenderEffect)
  const renderStroke = useEditorUiStore((s) => s.renderStroke)
  const setRenderStroke = useEditorUiStore((s) => s.setRenderStroke)

  // Re-render the open page (if any) so default changes preview immediately.
  const rerender = () => {
    const pageId = useSelectionStore.getState().pageId
    if (pageId) queueAutoRender(pageId)
  }

  const familyOptions = useMemo(() => {
    const families = new Map<string, FontFaceInfo>()
    for (const f of availableFonts) {
      const name = normalizeFamilyName(f.familyName)
      if (!families.has(name)) families.set(name, { ...f, familyName: name })
    }
    return Array.from(families.values()).sort((a, b) => a.familyName.localeCompare(b.familyName))
  }, [availableFonts])

  const currentFamily = defaultFont ? normalizeFamilyName(defaultFont) : ''

  const pickFont = (family: string) => {
    const variants = availableFonts.filter((f) => normalizeFamilyName(f.familyName) === family)
    const regular =
      variants.find((f) => {
        const ps = f.postScriptName.toLowerCase()
        return ps.includes('regular') || ps.includes('400')
      }) ?? variants[0]
    setDefaultFont(regular?.postScriptName ?? family)
    rerender()
  }

  const setSize = (size?: number) => {
    setDefaultFontSize(size)
    rerender()
  }

  const updateStroke = (next?: RenderStroke) => {
    setRenderStroke(next)
    rerender()
  }

  const toggleEffect = (key: 'bold' | 'italic') => {
    setRenderEffect({ ...renderEffect, [key]: !renderEffect[key] })
    rerender()
  }

  const size = defaultFontSize
  // Border default is three-state: auto (per-block model prediction), a
  // custom global outline, or explicitly off for blocks without their own.
  const strokeMode: 'auto' | 'custom' | 'off' =
    renderStroke === undefined ? 'auto' : renderStroke.enabled ? 'custom' : 'off'
  const strokeWidth = renderStroke?.widthPx ?? STROKE_DEFAULT_WIDTH
  // Undefined = automatic colour (the renderer contrasts against each
  // block's text colour). Only a deliberate pick stores a colour here.
  const strokeColor = renderStroke?.color
  const setStrokeMode = (mode: 'auto' | 'custom' | 'off') => {
    if (mode === 'auto') updateStroke(undefined)
    else updateStroke({ enabled: mode === 'custom', color: strokeColor, widthPx: strokeWidth })
  }

  const effects: { key: 'bold' | 'italic'; label: string; Icon: typeof BoldIcon }[] = [
    { key: 'bold', label: t('render.effectBold'), Icon: BoldIcon },
    { key: 'italic', label: t('render.effectItalic'), Icon: ItalicIcon },
  ]

  return (
    <div className='space-y-8'>
      <Section
        title={t('settings.textDefaults')}
        description={t('settings.textDefaultsDescription')}
      >
        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('render.fontLabel')}</Label>
          <div className='flex items-center gap-1'>
            <div className='min-w-0 flex-1'>
              <FontSelect
                value={currentFamily}
                options={familyOptions}
                favoriteFonts={favoriteFonts}
                onToggleFavorite={toggleFavoriteFont}
                disabled={familyOptions.length === 0}
                placeholder={t('settings.autoFont')}
                triggerStyle={currentFamily ? { fontFamily: currentFamily } : undefined}
                onChange={pickFont}
              />
            </div>
            <Button
              type='button'
              variant='ghost'
              size='icon-sm'
              aria-label={t('settings.resetToAuto')}
              title={t('settings.resetToAuto')}
              disabled={!defaultFont}
              className='size-7 shrink-0 text-muted-foreground hover:text-foreground'
              onClick={() => {
                setDefaultFont(undefined)
                rerender()
              }}
            >
              <RotateCcwIcon className='size-3.5' />
            </Button>
          </div>
        </div>

        <div className='flex flex-wrap items-end gap-6'>
          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('settings.defaultFontSize')}</Label>
            <div className='flex w-40 items-center rounded-md border border-input bg-background shadow-xs'>
              <Button
                type='button'
                variant='ghost'
                size='icon-sm'
                className='size-7 rounded-r-none border-r'
                onClick={() => setSize(Math.max(6, Math.round((size ?? 16) - 1)))}
              >
                <MinusIcon className='size-3' />
              </Button>
              <Input
                type='number'
                min='6'
                max='300'
                inputMode='numeric'
                className='h-7 min-w-0 flex-1 [appearance:textfield] rounded-none border-0 px-1 text-center text-xs shadow-none focus-visible:ring-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'
                value={size !== undefined ? Math.round(size) : ''}
                placeholder={t('settings.autoSize')}
                onChange={(e) => {
                  const v = e.target.value.trim()
                  if (v === '') {
                    setSize(undefined)
                    return
                  }
                  const n = Number.parseInt(v, 10)
                  if (Number.isFinite(n) && n >= 1) setSize(Math.min(300, n))
                }}
              />
              <Button
                type='button'
                variant='ghost'
                size='icon-sm'
                className='size-7 rounded-l-none border-l'
                onClick={() => setSize(Math.min(300, Math.round((size ?? 16) + 1)))}
              >
                <PlusIcon className='size-3' />
              </Button>
              <Button
                type='button'
                variant='ghost'
                size='icon-sm'
                aria-label={t('settings.resetToAuto')}
                title={t('settings.resetToAuto')}
                disabled={size === undefined}
                className='size-7 shrink-0 rounded-l-none border-l text-muted-foreground hover:text-foreground'
                onClick={() => setSize(undefined)}
              >
                <RotateCcwIcon className='size-3.5' />
              </Button>
            </div>
          </div>

          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('render.effectLabel')}</Label>
            <div className='flex items-center gap-1'>
              {effects.map(({ key, label, Icon }) => (
                <Button
                  key={key}
                  type='button'
                  variant='outline'
                  size='icon-sm'
                  aria-label={label}
                  className={cn(
                    'size-7',
                    renderEffect[key] &&
                      'border-primary bg-primary text-primary-foreground hover:bg-primary/90',
                  )}
                  onClick={() => toggleEffect(key)}
                >
                  <Icon className='size-3.5' />
                </Button>
              ))}
            </div>
          </div>
        </div>
      </Section>

      <Section title={t('render.effectBorder')} description={t('settings.borderDescription')}>
        <div className='flex items-center gap-2'>
          <Select value={strokeMode} onValueChange={(v) => setStrokeMode(v as typeof strokeMode)}>
            <SelectTrigger className='h-8 w-44 text-xs'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='auto'>{t('settings.borderAuto')}</SelectItem>
              <SelectItem value='custom'>{t('settings.borderCustom')}</SelectItem>
              <SelectItem value='off'>{t('settings.borderOff')}</SelectItem>
            </SelectContent>
          </Select>
          {strokeMode === 'custom' && (
            <>
              <ColorPicker
                value={colorToHex(strokeColor ?? STROKE_DEFAULT_COLOR)}
                className='size-7'
                onChange={(hex) =>
                  updateStroke({
                    enabled: true,
                    color: hexToColor(hex, strokeColor?.[3] ?? 255),
                    widthPx: strokeWidth,
                  })
                }
              />
              <div className='flex w-32 items-center rounded-md border border-input bg-background shadow-xs'>
                <Button
                  type='button'
                  variant='ghost'
                  size='icon-sm'
                  className='size-7 rounded-r-none border-r'
                  onClick={() =>
                    updateStroke({
                      enabled: true,
                      color: strokeColor,
                      widthPx: Math.max(0.2, Number((strokeWidth - 0.2).toFixed(1))),
                    })
                  }
                >
                  <MinusIcon className='size-3' />
                </Button>
                <Input
                  type='number'
                  step='0.1'
                  min='0.2'
                  max='24'
                  inputMode='decimal'
                  className='h-7 min-w-0 flex-1 [appearance:textfield] rounded-none border-0 px-1 text-center text-xs shadow-none focus-visible:ring-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'
                  value={strokeWidth}
                  onChange={(e) => {
                    const n = Number.parseFloat(e.target.value)
                    if (Number.isFinite(n)) {
                      updateStroke({
                        enabled: true,
                        color: strokeColor,
                        widthPx: Math.max(0.2, Math.min(24, n)),
                      })
                    }
                  }}
                />
                <Button
                  type='button'
                  variant='ghost'
                  size='icon-sm'
                  className='size-7 rounded-l-none border-l'
                  onClick={() =>
                    updateStroke({
                      enabled: true,
                      color: strokeColor,
                      widthPx: Math.min(24, Number((strokeWidth + 0.2).toFixed(1))),
                    })
                  }
                >
                  <PlusIcon className='size-3' />
                </Button>
              </div>
            </>
          )}
        </div>
      </Section>

      <Section title={t('settings.boxPadding')} description={t('settings.boxPaddingDescription')}>
        <div className='flex w-40 items-center rounded-md border border-input bg-background shadow-xs'>
          <Button
            type='button'
            variant='ghost'
            size='icon-sm'
            className='size-7 rounded-r-none border-r'
            onClick={() => {
              setBoxPadding(Math.max(0, boxPadding - 1))
              rerender()
            }}
          >
            <MinusIcon className='size-3' />
          </Button>
          <Input
            type='number'
            min='0'
            max='64'
            inputMode='numeric'
            className='h-7 min-w-0 flex-1 [appearance:textfield] rounded-none border-0 px-1 text-center text-xs shadow-none focus-visible:ring-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'
            value={Number.isFinite(boxPadding) ? boxPadding : 0}
            onChange={(e) => {
              const n = Number.parseInt(e.target.value, 10)
              if (Number.isFinite(n)) {
                setBoxPadding(Math.max(0, Math.min(64, n)))
                rerender()
              }
            }}
          />
          <Button
            type='button'
            variant='ghost'
            size='icon-sm'
            className='size-7 rounded-l-none border-l'
            onClick={() => {
              setBoxPadding(Math.min(64, boxPadding + 1))
              rerender()
            }}
          >
            <PlusIcon className='size-3' />
          </Button>
        </div>
      </Section>
    </div>
  )
}

// ── Engines ──────────────────────────────────────────────────────

function EnginesPane({
  catalog,
  pipeline,
  onChange,
}: {
  catalog: GetEngineCatalog200
  pipeline: import('@/lib/api/schemas').PipelineConfig
  onChange: (pipeline: import('@/lib/api/schemas').PipelineConfig) => void
}) {
  const { t } = useTranslation()

  const sections = [
    {
      label: t('settings.detector'),
      key: 'detector' as const,
      engines: catalog.detectors,
    },
    {
      label: t('settings.fontDetector'),
      key: 'font_detector' as const,
      engines: catalog.fontDetectors,
    },
    {
      label: t('settings.segmenter'),
      key: 'segmenter' as const,
      engines: catalog.segmenters,
    },
    {
      label: t('settings.bubbleSegmenter'),
      key: 'bubble_segmenter' as const,
      engines: catalog.bubbleSegmenters,
    },
    { label: t('settings.ocr'), key: 'ocr' as const, engines: catalog.ocr },
    {
      label: t('settings.translator'),
      key: 'translator' as const,
      engines: catalog.translators,
    },
    {
      label: t('settings.inpainter'),
      key: 'inpainter' as const,
      engines: catalog.inpainters,
    },
    {
      label: t('settings.renderer'),
      key: 'renderer' as const,
      engines: catalog.renderers,
    },
  ]

  return (
    <div className='space-y-4'>
      <p className='text-xs text-muted-foreground'>{t('settings.enginesDescription')}</p>
      {sections.map(({ label, key, engines }) => (
        <div key={key} className='space-y-1.5'>
          <Label className='text-xs'>{label}</Label>
          <Select
            value={pipeline[key] ?? engines[0]?.id ?? ''}
            onValueChange={(v) => onChange({ ...pipeline, [key]: v })}
          >
            <SelectTrigger className='w-full'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {engines.map((e) => (
                <SelectItem key={e.id} value={e.id}>
                  {e.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      ))}
    </div>
  )
}

// ── Providers ─────────────────────────────────────────────────────

function ProvidersPane({
  catalogs,
  config,
  drafts,
  baseUrlDrafts,
  saveErrors,
  onBaseUrlChange,
  onBaseUrlBlur,
  onApiKeyChange,
  onSaveKey,
  onClearKey,
}: {
  catalogs: LlmProviderCatalog[]
  config: UpdateConfigBody | null
  drafts: Record<string, string>
  baseUrlDrafts: Record<string, string>
  saveErrors: Record<string, string>
  onBaseUrlChange: (id: string, v: string) => void
  onBaseUrlBlur: (id: string) => void
  onApiKeyChange: (id: string, v: string) => void
  onSaveKey: (id: string) => void
  onClearKey: (id: string) => void
}) {
  const { t } = useTranslation()

  if (!catalogs.length)
    return (
      <p className='py-12 text-center text-sm text-muted-foreground'>
        {t('settings.loadingProviders')}
      </p>
    )

  return (
    <div className='space-y-6'>
      <Section title={t('settings.apiKeys')} description={t('settings.providersDescription')}>
        <Accordion type='multiple' className='-mx-1'>
          {catalogs.map((provider) => {
            const cfg = config?.providers?.find((p) => p.id === provider.id)
            const draft = drafts[provider.id] ?? ''
            const hasDraft = draft.trim().length > 0
            const statusColor =
              provider.status === 'ready'
                ? 'bg-green-500'
                : provider.status === 'missing_configuration'
                  ? 'bg-amber-400'
                  : provider.status === 'discovery_failed'
                    ? 'bg-red-500'
                    : 'bg-muted-foreground'

            return (
              <AccordionItem key={provider.id} value={provider.id} className='border-border'>
                <AccordionTrigger className='px-1 py-3 hover:no-underline'>
                  <div className='flex items-center gap-2.5'>
                    <span className={`size-2 shrink-0 rounded-full ${statusColor}`} />
                    <span className='text-sm font-medium'>{provider.name}</span>
                  </div>
                </AccordionTrigger>
                <AccordionContent className='space-y-4 px-1 pt-1 pb-4'>
                  {provider.error && (
                    <p className='text-xs text-muted-foreground'>{provider.error}</p>
                  )}

                  {provider.requiresBaseUrl && (
                    <div className='space-y-1.5'>
                      <Label className='text-xs'>{t('settings.localLlmBaseUrl')}</Label>
                      <Input
                        type='url'
                        value={baseUrlDrafts[provider.id] ?? ''}
                        onChange={(e) => onBaseUrlChange(provider.id, e.target.value)}
                        onBlur={() => onBaseUrlBlur(provider.id)}
                        placeholder='https://api.example.com/v1'
                      />
                    </div>
                  )}

                  <div className='space-y-1.5'>
                    <Label className='text-xs'>{t('settings.apiKey')}</Label>
                    <div className='flex gap-2'>
                      <Input
                        type='password'
                        value={draft}
                        onChange={(e) => onApiKeyChange(provider.id, e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter' && hasDraft) onSaveKey(provider.id)
                        }}
                        placeholder={
                          cfg?.api_key === '[REDACTED]'
                            ? t('settings.apiKeyPlaceholderStored')
                            : t('settings.apiKeyPlaceholderEmpty')
                        }
                        className='[&::-ms-reveal]:hidden'
                      />
                      {hasDraft ? (
                        <Button size='sm' onClick={() => onSaveKey(provider.id)}>
                          {t('settings.apiKeySave')}
                        </Button>
                      ) : cfg?.api_key === '[REDACTED]' ? (
                        <Button
                          variant='destructive'
                          size='sm'
                          onClick={() => onClearKey(provider.id)}
                        >
                          {t('settings.apiKeyClear')}
                        </Button>
                      ) : null}
                    </div>
                    {saveErrors[provider.id] && (
                      <p className='text-xs text-destructive'>{saveErrors[provider.id]}</p>
                    )}
                  </div>
                </AccordionContent>
              </AccordionItem>
            )
          })}
        </Accordion>
      </Section>
    </div>
  )
}

// ── Keybinds ──────────────────────────────────────────────────────

function CodexSettingsPane() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [login, setLogin] = useState<CodexDeviceLogin | null>(null)
  const [loginOpen, setLoginOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [copied, setCopied] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const { data: auth, refetch } = useGetCodexAuthStatus()

  const loginStatus = auth?.login?.status
  const signedIn = auth?.signedIn === true

  useEffect(() => {
    if (!loginOpen && loginStatus !== 'pending') return
    const id = window.setInterval(() => void refetch(), 2000)
    return () => window.clearInterval(id)
  }, [loginOpen, loginStatus, refetch])

  useEffect(() => {
    if (loginOpen && (signedIn || loginStatus === 'succeeded')) {
      const id = window.setTimeout(() => setLoginOpen(false), 700)
      return () => window.clearTimeout(id)
    }
  }, [loginOpen, loginStatus, signedIn])

  const statusLabel = useMemo(() => {
    if (signedIn) return auth?.accountId ? auth.accountId : t('ai.signedIn')
    if (loginStatus === 'failed') return t('ai.signInFailed')
    if (loginStatus === 'pending') return t('ai.signInPending')
    return t('ai.signedOut')
  }, [auth?.accountId, loginStatus, signedIn, t])

  const invalidateAuth = () =>
    queryClient.invalidateQueries({ queryKey: getGetCodexAuthStatusQueryKey() })

  const handleSignIn = async () => {
    setBusy(true)
    setActionError(null)
    try {
      const next = await startCodexDeviceLogin()
      setLogin(next)
      setCopied(false)
      setLoginOpen(true)
      void invalidateAuth()
      void openExternalUrl(next.verificationUrl)
    } catch (err) {
      setActionError(String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleLogout = async () => {
    setBusy(true)
    setActionError(null)
    try {
      await deleteCodexSession()
      await invalidateAuth()
    } catch (err) {
      setActionError(String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleCopyCode = async () => {
    if (!login?.userCode || typeof navigator === 'undefined') return
    await navigator.clipboard?.writeText(login.userCode)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1200)
  }

  return (
    <Section title={t('settings.codex')} description={t('settings.codexDescription')}>
      <div className='rounded-md border border-amber-200/70 bg-amber-50/80 p-3 text-xs leading-relaxed text-amber-900 dark:border-amber-900/70 dark:bg-amber-950/40 dark:text-amber-100'>
        {t('settings.codexTwoFactorDescription')}
      </div>
      <div className='rounded-md border border-border bg-card p-3'>
        <div className='flex items-center justify-between gap-3'>
          <div className='flex min-w-0 items-center gap-2'>
            <div className='flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary'>
              <SparklesIcon className='size-4' />
            </div>
            <div className='min-w-0'>
              <div className='text-sm font-medium text-foreground'>Codex</div>
              <div className='truncate text-xs text-muted-foreground'>{statusLabel}</div>
            </div>
          </div>
          {signedIn ? (
            <Button
              variant='outline'
              size='sm'
              className='gap-1.5'
              disabled={busy}
              onClick={() => void handleLogout()}
            >
              <LogOutIcon className='size-3.5' />
              {t('ai.signOut')}
            </Button>
          ) : (
            <Button
              variant='default'
              size='sm'
              className='gap-1.5'
              disabled={busy}
              onClick={() => void handleSignIn()}
            >
              {busy ? (
                <LoaderIcon className='size-3.5 animate-spin' />
              ) : (
                <LogInIcon className='size-3.5' />
              )}
              {t('ai.signIn')}
            </Button>
          )}
        </div>
        {(actionError || (auth?.login?.status === 'failed' && auth.login.error)) && (
          <p className='mt-2 line-clamp-3 text-xs text-destructive'>
            {actionError || auth?.login?.error}
          </p>
        )}
      </div>

      <Dialog open={loginOpen} onOpenChange={setLoginOpen}>
        <DialogContent className='w-[340px] max-w-[92vw] gap-3 p-4'>
          <DialogTitle className='text-sm'>{t('ai.signInTitle')}</DialogTitle>
          <DialogDescription className='sr-only'>{t('ai.signIn')}</DialogDescription>
          <div className='flex items-center justify-between gap-3 rounded-md border border-border bg-muted/40 px-3 py-2'>
            <div className='min-w-0'>
              <div className='text-[10px] font-semibold tracking-wide text-muted-foreground uppercase'>
                {t('ai.userCode')}
              </div>
              <div className='mt-0.5 font-mono text-xl font-semibold tracking-widest'>
                {login?.userCode ?? '...'}
              </div>
            </div>
            <Button
              variant='outline'
              size='icon-sm'
              disabled={!login}
              aria-label={copied ? t('common.copied') : t('common.copy')}
              onClick={() => void handleCopyCode()}
            >
              <CopyIcon className='size-3.5' />
            </Button>
          </div>
          <Button
            variant='outline'
            size='sm'
            className='w-full gap-1.5'
            disabled={!login}
            onClick={() => login && void openExternalUrl(login.verificationUrl)}
          >
            <ExternalLinkIcon className='size-3.5' />
            {t('ai.openBrowser')}
          </Button>
          <div className='flex items-center gap-2 text-xs text-muted-foreground'>
            {signedIn || loginStatus === 'succeeded' ? (
              <>
                <CheckCircleIcon className='size-4 text-green-500' />
                {t('ai.signInComplete')}
              </>
            ) : loginStatus === 'failed' ? (
              <>
                <AlertCircleIcon className='size-4 text-destructive' />
                <span className='line-clamp-2'>{auth?.login?.error ?? t('ai.signInFailed')}</span>
              </>
            ) : (
              <>
                <LoaderIcon className='size-4 animate-spin' />
                {t('ai.signInPending')}
              </>
            )}
          </div>
        </DialogContent>
      </Dialog>
    </Section>
  )
}

const SHORTCUT_ITEMS = [
  { key: 'select', labelKey: 'toolRail.select' },
  { key: 'block', labelKey: 'toolRail.block' },
  { key: 'brush', labelKey: 'toolRail.brush' },
  { key: 'eraser', labelKey: 'toolRail.eraser' },
  { key: 'repairBrush', labelKey: 'toolRail.repairBrush' },
  {
    key: 'increaseBrushSize',
    labelKey: 'settings.shortcutIncreaseBrushSize',
  },
  {
    key: 'decreaseBrushSize',
    labelKey: 'settings.shortcutDecreaseBrushSize',
  },
  { key: 'undo', labelKey: 'menu.undo' },
  { key: 'redo', labelKey: 'menu.redo' },
] as const

function KeybindsPane() {
  const { t } = useTranslation()
  const shortcuts = usePreferencesStore((state) => state.shortcuts)
  const setShortcuts = usePreferencesStore((state) => state.setShortcuts)
  const resetShortcutsStore = usePreferencesStore((state) => state.resetShortcuts)
  const [pendingShortcuts, setPendingShortcuts] = useState(shortcuts)
  const [recordingKey, setRecordingKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [isSaved, setIsSaved] = useState(false)
  const [liveShortcut, setLiveShortcut] = useState<string | null>(null)
  const isMac = useMemo(() => getPlatform() === 'mac', [])

  // Optimized conflict detection
  const conflictCounts = useMemo(() => {
    const counts = new Map<string, number>()
    Object.values(pendingShortcuts).forEach((val) => {
      counts.set(val, (counts.get(val) || 0) + 1)
    })
    return counts
  }, [pendingShortcuts])

  const isDirty = useMemo(
    () => !areShortcutsEqual(shortcuts, pendingShortcuts),
    [shortcuts, pendingShortcuts],
  )

  // Sync from store if it changes (e.g. externally via Reset)
  useEffect(() => {
    setPendingShortcuts(shortcuts)
  }, [shortcuts])

  useEffect(() => {
    if (!recordingKey) {
      setError(null)
      setLiveShortcut(null)
      return
    }

    setError(null)
    setLiveShortcut(null)
    const handleKeyDown = (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()
      setError(null)

      // Early exit for modifier-only events - but update preview!
      if (isModifierKey(e.key)) {
        setLiveShortcut(formatModifierCombination(e, isMac))
        return
      }

      // Allow Escape to cancel recording
      if (e.key === 'Escape') {
        setRecordingKey(null)
        setLiveShortcut(null)
        return
      }

      // Block system/function keys
      if (isKeyBlocked(e.key)) {
        setError(t('settings.shortcutInvalid'))
        return
      }

      const shortcut = formatShortcut(e, isMac)
      if (!shortcut) return

      setPendingShortcuts((prev) => ({ ...prev, [recordingKey]: shortcut }))
      setRecordingKey(null)
      setIsSaved(false)
      setLiveShortcut(null)
    }

    const handleKeyUp = (e: KeyboardEvent) => {
      if (isModifierKey(e.key)) {
        const combo = formatModifierCombination(e, isMac)
        setLiveShortcut(combo || null)
      }
    }

    const handleClickOutside = () => {
      setRecordingKey(null)
      setLiveShortcut(null)
    }

    window.addEventListener('keydown', handleKeyDown, { capture: true })
    window.addEventListener('keyup', handleKeyUp, { capture: true })
    window.addEventListener('click', handleClickOutside, { capture: true })
    return () => {
      window.removeEventListener('keydown', handleKeyDown, { capture: true })
      window.removeEventListener('keyup', handleKeyUp, { capture: true })
      window.removeEventListener('click', handleClickOutside, {
        capture: true,
      })
    }
  }, [recordingKey, pendingShortcuts, t, isMac])

  const [resetConfirmOpen, setResetConfirmOpen] = useState(false)

  const saveTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    return () => {
      if (saveTimeoutRef.current) {
        clearTimeout(saveTimeoutRef.current)
      }
    }
  }, [])

  const handleSave = () => {
    setShortcuts(pendingShortcuts)
    setIsSaved(true)

    if (saveTimeoutRef.current) {
      clearTimeout(saveTimeoutRef.current)
    }

    saveTimeoutRef.current = setTimeout(() => {
      setIsSaved(false)
      saveTimeoutRef.current = null
    }, 2000)
  }

  const handleReset = () => {
    setResetConfirmOpen(true)
  }

  const handleConfirmReset = () => {
    resetShortcutsStore()
    setResetConfirmOpen(false)
  }

  const renderShortcutKeys = (shortcutStr: string, kbdClass?: string) => {
    const parts = shortcutStr.split('+')

    return parts.map((part, i) => (
      <Fragment key={i}>
        <Kbd className={kbdClass}>{part}</Kbd>
        {i < parts.length - 1 && <span className='text-muted-foreground'>+</span>}
      </Fragment>
    ))
  }

  return (
    <div className='flex h-full flex-col gap-6'>
      <div className='grow space-y-6 overflow-y-auto pr-2'>
        <Section title={t('settings.keybinds')} description={t('settings.keybindsDescription')}>
          <div className='divide-y divide-border overflow-hidden rounded-xl border border-border bg-card'>
            {SHORTCUT_ITEMS.map((item) => {
              const currentVal = pendingShortcuts[item.key]
              const hasConflict = currentVal && (conflictCounts.get(currentVal) || 0) > 1
              const conflictingItem = hasConflict
                ? SHORTCUT_ITEMS.find(
                    (s) => s.key !== item.key && pendingShortcuts[s.key] === currentVal,
                  )
                : null

              return (
                <div key={item.key} className='flex items-center justify-between px-4 py-2'>
                  <div className='flex items-center gap-2'>
                    <span className='text-sm'>{t(item.labelKey)}</span>
                    {hasConflict && (
                      <div
                        title={`${t('settings.shortcutConflict')}${
                          conflictingItem ? `: ${t(conflictingItem.labelKey)}` : ''
                        }`}
                      >
                        <AlertTriangleIcon className='size-3.5 text-amber-500' />
                      </div>
                    )}
                  </div>
                  <Button
                    variant={recordingKey === item.key ? 'secondary' : 'ghost'}
                    size='sm'
                    onClick={(e) => {
                      e.stopPropagation()
                      setRecordingKey(item.key)
                    }}
                    className='group h-8 w-fit px-2 font-mono uppercase'
                  >
                    <div className='flex items-center gap-1'>
                      {recordingKey === item.key ? (
                        error ? (
                          <span className='text-xs text-destructive'>{error}</span>
                        ) : liveShortcut ? (
                          renderShortcutKeys(liveShortcut)
                        ) : (
                          <span className='text-xs text-muted-foreground italic'>
                            {t('settings.shortcutPressKey')}
                          </span>
                        )
                      ) : currentVal ? (
                        renderShortcutKeys(currentVal, 'bg-background')
                      ) : (
                        <span className='text-xs text-muted-foreground'>NONE</span>
                      )}
                    </div>
                  </Button>
                </div>
              )
            })}
          </div>
        </Section>
      </div>

      <div className='flex items-center justify-between border-t border-border pt-4'>
        <Button
          variant='ghost'
          size='sm'
          className='gap-2 text-muted-foreground hover:text-foreground'
          onClick={handleReset}
        >
          <RotateCcwIcon className='size-4' />
          {t('settings.shortcutReset')}
        </Button>
        <div className='flex items-center gap-2'>
          <Button
            variant='default'
            size='sm'
            disabled={!isDirty || isSaved}
            onClick={handleSave}
            className='min-w-32 gap-2'
          >
            {isSaved ? (
              <>
                <CheckCircleIcon className='size-4' />
                {t('common.saved')}
              </>
            ) : (
              <>
                <SaveIcon className='size-4' />
                {t('common.save')}
              </>
            )}
          </Button>
        </div>
      </div>
      <AlertDialog open={resetConfirmOpen} onOpenChange={setResetConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogTitle>{t('settings.shortcutReset')}</AlertDialogTitle>
          <AlertDialogDescription>{t('settings.shortcutResetDescription')}</AlertDialogDescription>
          <div className='flex justify-end gap-2'>
            <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction onClick={handleConfirmReset}>
              {t('common.confirm')}
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

// ── Storage ───────────────────────────────────────────────────────

function StoragePane({
  dataPath,
  httpConnectTimeout,
  httpReadTimeout,
  httpMaxRetries,
  error,
  saving,
  unchanged,
  onPathChange,
  onHttpConnectTimeoutChange,
  onHttpReadTimeoutChange,
  onHttpMaxRetriesChange,
  onApply,
}: {
  dataPath: string
  httpConnectTimeout: string
  httpReadTimeout: string
  httpMaxRetries: string
  error: string | null
  saving: boolean
  unchanged: boolean
  onPathChange: (v: string) => void
  onHttpConnectTimeoutChange: (v: string) => void
  onHttpReadTimeoutChange: (v: string) => void
  onHttpMaxRetriesChange: (v: string) => void
  onApply: () => void
}) {
  const { t } = useTranslation()
  const [confirmOpen, setConfirmOpen] = useState(false)

  return (
    <>
      <Section title={t('settings.runtime')} description={t('settings.runtimeDescription')}>
        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.dataPath')}</Label>
          <Input type='text' value={dataPath} onChange={(e) => onPathChange(e.target.value)} />
          <p className='text-xs leading-relaxed text-muted-foreground'>
            {t('settings.dataPathDescription')}
          </p>
        </div>

        <div className='grid gap-4 md:grid-cols-2'>
          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('settings.httpConnectTimeout')}</Label>
            <Input
              type='number'
              min='1'
              step='1'
              inputMode='numeric'
              value={httpConnectTimeout}
              onChange={(e) => onHttpConnectTimeoutChange(e.target.value)}
            />
            <p className='text-xs leading-relaxed text-muted-foreground'>
              {t('settings.httpConnectTimeoutDescription')}
            </p>
          </div>

          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('settings.httpReadTimeout')}</Label>
            <Input
              type='number'
              min='1'
              step='1'
              inputMode='numeric'
              value={httpReadTimeout}
              onChange={(e) => onHttpReadTimeoutChange(e.target.value)}
            />
            <p className='text-xs leading-relaxed text-muted-foreground'>
              {t('settings.httpReadTimeoutDescription')}
            </p>
          </div>
        </div>

        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.httpMaxRetries')}</Label>
          <Input
            type='number'
            min='0'
            step='1'
            inputMode='numeric'
            value={httpMaxRetries}
            onChange={(e) => onHttpMaxRetriesChange(e.target.value)}
          />
          <p className='text-xs leading-relaxed text-muted-foreground'>
            {t('settings.httpMaxRetriesDescription')}
          </p>
        </div>

        {error && <p className='text-xs text-destructive'>{error}</p>}
        <div className='flex justify-end pt-1'>
          <Button
            onClick={() => setConfirmOpen(true)}
            disabled={!dataPath.trim() || saving || unchanged}
          >
            {saving ? t('settings.restartApplying') : t('settings.restartApply')}
          </Button>
        </div>
      </Section>

      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogTitle>{t('settings.restartApply')}</AlertDialogTitle>
          <AlertDialogDescription>
            {t('settings.restartRequiredDescription')}
          </AlertDialogDescription>
          <div className='flex justify-end gap-2'>
            <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                setConfirmOpen(false)
                onApply()
              }}
            >
              {t('settings.restartApply')}
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

// ── About ─────────────────────────────────────────────────────────

function AboutPane({
  version,
  latestVersion,
  status,
  isInstallingUpdate,
  onInstallUpdate,
}: {
  version?: string
  latestVersion?: string
  status: UpdaterStatus
  isInstallingUpdate: boolean
  onInstallUpdate: () => void
}) {
  const { t } = useTranslation()

  return (
    <div className='flex h-full flex-col items-center justify-center gap-5 py-8'>
      <img src='/icon-large.png' alt='Koharu' className='size-20' draggable={false} />
      <div className='text-center'>
        <h2 className='text-lg font-bold tracking-wide text-foreground'>Koharu</h2>
        <p className='mt-1 text-sm text-muted-foreground'>{t('settings.aboutTagline')}</p>
      </div>

      <div className='w-full max-w-sm rounded-xl border border-border bg-card p-4'>
        <div className='space-y-3 text-sm'>
          <InfoRow label={t('settings.aboutVersion')}>
            <div className='flex flex-col items-end gap-0.5'>
              <span className='font-mono text-xs font-medium'>{version || '...'}</span>
              {status === 'loading' && (
                <LoaderIcon className='size-3.5 animate-spin text-muted-foreground' />
              )}
              {status === 'latest' && (
                <span className='flex items-center gap-1 text-xs text-green-500'>
                  <CheckCircleIcon className='size-3.5' />
                  {t('settings.aboutLatest')}
                </span>
              )}
              {status === 'outdated' && (
                <Button
                  variant='link'
                  size='xs'
                  onClick={onInstallUpdate}
                  disabled={isInstallingUpdate}
                  className='h-auto gap-1 p-0 text-amber-500'
                >
                  {isInstallingUpdate ? (
                    <LoaderIcon className='size-3.5 animate-spin' />
                  ) : (
                    <AlertCircleIcon className='size-3.5' />
                  )}
                  {t('settings.aboutUpdate', { version: latestVersion })}
                </Button>
              )}
            </div>
          </InfoRow>
          <InfoRow label={t('settings.aboutAuthor')}>
            <Button
              variant='link'
              size='xs'
              onClick={() => void openExternalUrl('https://github.com/mayocream')}
            >
              Mayo
            </Button>
          </InfoRow>
          <InfoRow label={t('settings.aboutRepository')}>
            <Button
              variant='link'
              size='xs'
              onClick={() => void openExternalUrl(`https://github.com/${GITHUB_REPO}`)}
            >
              GitHub
            </Button>
          </InfoRow>
        </div>
      </div>
    </div>
  )
}

// ── Shared ────────────────────────────────────────────────────────

function Section({
  title,
  description,
  children,
}: {
  title: string
  description?: string
  children: ReactNode
}) {
  return (
    <div className='space-y-3'>
      <div>
        <h3 className='text-sm font-semibold text-foreground'>{title}</h3>
        {description && (
          <p className='mt-0.5 text-xs leading-relaxed text-muted-foreground'>{description}</p>
        )}
      </div>
      {children}
    </div>
  )
}

function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className='flex items-center justify-between'>
      <span className='text-muted-foreground'>{label}</span>
      <div className='flex items-center'>{children}</div>
    </div>
  )
}
