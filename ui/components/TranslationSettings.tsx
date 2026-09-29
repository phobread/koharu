'use client'

import { LoaderCircleIcon } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { LlmModelSelect } from '@/components/ui/llm-model-select'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import {
  deleteCurrentLlm,
  putCurrentLlm,
  useGetCatalog,
  useGetCurrentLlm,
} from '@/lib/api/default/default'
import {
  flattenCatalogModels,
  llmTargetKey,
  sameLlmTarget,
  withSelectedTarget,
} from '@/lib/llmTargets'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'
import { flushServerConfigStorage } from '@/lib/stores/serverConfigStorage'

/**
 * Translation model, target language and instructions: set once, so they
 * live in Settings rather than on the canvas toolbar.
 */
export function TranslationSettings() {
  const { t } = useTranslation()
  const { data: llmCatalog } = useGetCatalog()
  const { data: llmState } = useGetCurrentLlm()
  const llmReady = llmState?.status === 'ready'
  const llmLoading = llmState?.status === 'loading'
  const [busy, setBusy] = useState(false)
  const llmModels = useMemo(() => flattenCatalogModels(llmCatalog), [llmCatalog])
  const selectedTarget = useEditorUiStore((s) => s.selectedTarget)
  const customSystemPrompt = usePreferencesStore((s) => s.customSystemPrompt)
  const setCustomSystemPrompt = usePreferencesStore((s) => s.setCustomSystemPrompt)
  const llmSelectedLanguage = useEditorUiStore((s) => s.selectedLanguage)

  // Keep the saved selection visible even while provider discovery is slow
  // or failing — otherwise the picker shows a placeholder and the selection
  // looks reset when it isn't.
  const displayModels = useMemo(
    () => withSelectedTarget(llmModels, selectedTarget, llmSelectedLanguage),
    [llmModels, selectedTarget, llmSelectedLanguage],
  )
  const selectedModel = useMemo(
    () => displayModels.find(({ model }) => sameLlmTarget(model.target, selectedTarget)),
    [displayModels, selectedTarget],
  )
  const selectedTargetKey = selectedTarget ? llmTargetKey(selectedTarget) : undefined
  const selectedModelLanguages = selectedModel?.model.languages ?? []
  const selectedIsLoaded = llmReady && sameLlmTarget(llmState?.target, selectedTarget)

  const handleSetSelectedModel = (key: string) => {
    const next = displayModels.find(({ model }) => llmTargetKey(model.target) === key)
    if (!next) return
    const nextLanguages = next.model.languages
    const nextLanguage =
      llmSelectedLanguage && nextLanguages.includes(llmSelectedLanguage)
        ? llmSelectedLanguage
        : nextLanguages[0]
    useEditorUiStore.setState({ selectedTarget: next.model.target, selectedLanguage: nextLanguage })
    window.setTimeout(() => void flushServerConfigStorage(), 0)
  }

  const handleSetSelectedLanguage = (language: string) => {
    if (!selectedModelLanguages.includes(language)) return
    useEditorUiStore.setState({ selectedLanguage: language })
    window.setTimeout(() => void flushServerConfigStorage(), 0)
  }

  const handleToggleLoadUnload = async () => {
    const target = useEditorUiStore.getState().selectedTarget
    if (!target) return
    setBusy(true)
    try {
      if (selectedIsLoaded) {
        await deleteCurrentLlm()
      } else {
        await putCurrentLlm({ target })
      }
    } catch (e) {
      useEditorUiStore.getState().showError(String(e))
    } finally {
      setBusy(false)
    }
  }

  const indicatorBusy = busy || llmLoading
  const status = selectedIsLoaded
    ? t('llm.paneReady', 'Loaded — ready to translate')
    : indicatorBusy
      ? t('llm.paneLoading', 'Loading…')
      : t('llm.paneNotLoaded', 'Not loaded')

  return (
    <div className='space-y-6' data-testid='translation-settings'>
      <div className='space-y-2'>
        <div>
          <h3 className='text-sm font-semibold text-foreground'>{t('llm.model')}</h3>
          <p className='mt-0.5 text-xs text-muted-foreground' data-testid='llm-status'>
            {status}
          </p>
        </div>
        <div className='flex items-center gap-1.5'>
          <LlmModelSelect
            data-testid='llm-model-select'
            value={selectedTargetKey}
            options={displayModels}
            getKey={({ model }) => llmTargetKey(model.target)}
            placeholder={t('llm.selectPlaceholder')}
            onChange={handleSetSelectedModel}
            triggerClassName='min-w-0 flex-1'
          />
          <Button
            data-testid='llm-load-toggle'
            data-llm-ready={selectedIsLoaded ? 'true' : 'false'}
            data-llm-loading={indicatorBusy ? 'true' : 'false'}
            variant={selectedIsLoaded ? 'outline' : 'default'}
            size='sm'
            onClick={() => void handleToggleLoadUnload()}
            disabled={!selectedTarget || indicatorBusy}
            className='shrink-0 gap-1'
          >
            {indicatorBusy ? <LoaderCircleIcon className='size-3.5 animate-spin' /> : null}
            {selectedIsLoaded ? t('llm.unload') : t('llm.load')}
          </Button>
        </div>
      </div>

      {selectedModelLanguages.length > 0 && (
        <div className='space-y-2'>
          <h3 className='text-sm font-semibold text-foreground'>
            {t('llm.targetLanguage', 'Translate into')}
          </h3>
          <Select
            value={llmSelectedLanguage ?? selectedModelLanguages[0]}
            onValueChange={handleSetSelectedLanguage}
          >
            <SelectTrigger data-testid='llm-language-select' className='w-full'>
              <SelectValue placeholder={t('llm.languagePlaceholder')} />
            </SelectTrigger>
            <SelectContent position='popper'>
              {selectedModelLanguages.map((language, index) => (
                <SelectItem
                  key={language}
                  value={language}
                  data-testid={`llm-language-option-${index}`}
                >
                  {t(`llm.languages.${language}`, { defaultValue: language })}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}

      <div className='space-y-2'>
        <h3 className='text-sm font-semibold text-foreground'>
          {t('llm.instructions', 'Instructions for the translator')}
        </h3>
        <Textarea
          data-testid='llm-system-prompt'
          value={customSystemPrompt ?? ''}
          onChange={(e) => setCustomSystemPrompt(e.target.value)}
          onBlur={() => void flushServerConfigStorage()}
          placeholder={t('llm.systemPromptPlaceholder')}
          rows={6}
          className='min-h-0 resize-y text-xs leading-snug md:text-xs'
        />
      </div>
    </div>
  )
}
