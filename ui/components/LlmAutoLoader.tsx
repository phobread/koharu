'use client'

import { useEffect, useMemo, useRef, useState } from 'react'

import { putCurrentLlm, useGetCatalog, useGetCurrentLlm } from '@/lib/api/default/default'
import {
  flattenCatalogModels,
  llmTargetKey,
  preferredDefaultModel,
  sameLlmTarget,
} from '@/lib/llmTargets'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

export function LlmAutoLoader() {
  const { data: llmCatalog } = useGetCatalog()
  const { data: llmState } = useGetCurrentLlm()
  const [editorHydrated, setEditorHydrated] = useState(useEditorUiStore.persist.hasHydrated())
  const autoLoadAttemptedKeys = useRef(new Set<string>())
  const llmModels = useMemo(() => flattenCatalogModels(llmCatalog), [llmCatalog])
  const selectedTarget = useEditorUiStore((s) => s.selectedTarget)

  const selectedModel = useMemo(
    () => llmModels.find(({ model }) => sameLlmTarget(model.target, selectedTarget)),
    [llmModels, selectedTarget],
  )

  useEffect(() => {
    const unsubscribe = useEditorUiStore.persist.onFinishHydration(() => setEditorHydrated(true))
    if (useEditorUiStore.persist.hasHydrated()) setEditorHydrated(true)
    return unsubscribe
  }, [])

  useEffect(() => {
    if (!editorHydrated) return
    if (llmModels.length === 0) return
    const cur = useEditorUiStore.getState()
    const currentModel = llmModels.find(({ model }) =>
      sameLlmTarget(model.target, cur.selectedTarget),
    )
    if (cur.selectedTarget && !currentModel) return
    const nextModel = currentModel?.model ?? preferredDefaultModel(llmModels)
    if (!nextModel) return
    const nextLanguages = nextModel.languages
    const nextLanguage =
      cur.selectedLanguage && nextLanguages.includes(cur.selectedLanguage)
        ? cur.selectedLanguage
        : nextLanguages[0]
    if (
      sameLlmTarget(cur.selectedTarget, nextModel.target) &&
      cur.selectedLanguage === nextLanguage
    ) {
      return
    }
    useEditorUiStore.setState({
      selectedTarget: nextModel.target,
      selectedLanguage: nextLanguage,
    })
  }, [editorHydrated, llmModels])

  useEffect(() => {
    if (!editorHydrated) return
    if (!llmCatalog) return
    if (!selectedTarget) return
    if (!selectedModel) return
    if (!llmState) return
    if (llmState.status === 'loading') return
    if (llmState.status === 'ready' && sameLlmTarget(llmState.target, selectedTarget)) return

    const selectedKey = llmTargetKey(selectedTarget)
    if (autoLoadAttemptedKeys.current.has(selectedKey)) return
    autoLoadAttemptedKeys.current.add(selectedKey)

    void putCurrentLlm({ target: selectedTarget }).catch((e) =>
      useEditorUiStore.getState().showError(String(e)),
    )
  }, [editorHydrated, llmCatalog, llmState, selectedModel, selectedTarget])

  return null
}
