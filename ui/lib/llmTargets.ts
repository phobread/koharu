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

/**
 * Model to preselect when nothing is saved. Only ever picks the explicit
 * provider defaults — NEVER an arbitrary catalog entry. A `models[0]`
 * fallback here once auto-selected the first *local* model, and the
 * auto-loader then kicked off its multi-gigabyte weights download without
 * the user asking for anything.
 */
export const preferredDefaultModel = (models: SelectableLlmModel[]): LlmCatalogModel | undefined =>
  models.find(({ model }) => sameLlmTarget(model.target, DEFAULT_LLM_TARGET))?.model ??
  models.find(
    ({ model, provider }) =>
      provider?.id === 'claude' && model.name.trim().toLowerCase() === 'claude opus 4.5',
  )?.model

/**
 * Catalog models plus a synthesized entry for the saved target when the
 * catalog doesn't (yet) list it. Provider model lists come from live
 * discovery calls that can be slow or fail; without this the picker shows an
 * empty placeholder and the user reads it as "my saved model got reset".
 */
export function withSelectedTarget(
  models: SelectableLlmModel[],
  target?: LlmTarget | null,
  language?: string,
): SelectableLlmModel[] {
  if (!target) return models
  if (models.some(({ model }) => sameLlmTarget(model.target, target))) return models
  return [
    { model: { target, name: target.modelId, languages: language ? [language] : [] } },
    ...models,
  ]
}
