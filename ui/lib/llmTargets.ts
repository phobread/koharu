import type { LlmCatalog, LlmCatalogModel, LlmProviderCatalog, LlmTarget } from '@/lib/api/schemas'

export type SelectableLlmModel = { model: LlmCatalogModel; provider?: LlmProviderCatalog }

export const DEFAULT_LLM_TARGET: LlmTarget = {
  kind: 'provider',
  providerId: 'claude',
  modelId: 'claude-opus-4-5-20251101',
}

export function llmTargetKey(t: LlmTarget): string {
  return `${t.kind}:${t.providerId ?? ''}:${t.modelId}`
}

export function sameLlmTarget(a?: LlmTarget | null, b?: LlmTarget | null): boolean {
  if (!a || !b) return false
  return (
    a.kind === b.kind &&
    a.modelId === b.modelId &&
    (a.providerId ?? null) === (b.providerId ?? null)
  )
}

export const flattenCatalogModels = (catalog?: LlmCatalog): SelectableLlmModel[] => [
  ...(catalog?.localModels ?? []).map((model) => ({ model })),
  ...(catalog?.providers ?? [])
    .filter((p) => p.status === 'ready')
    .flatMap((p) => p.models.map((model) => ({ model, provider: p }))),
]

export const preferredDefaultModel = (models: SelectableLlmModel[]): LlmCatalogModel | undefined =>
  models.find(({ model }) => sameLlmTarget(model.target, DEFAULT_LLM_TARGET))?.model ??
  models.find(
    ({ model, provider }) =>
      provider?.id === 'claude' && model.name.trim().toLowerCase() === 'claude opus 4.5',
  )?.model ??
  models[0]?.model
