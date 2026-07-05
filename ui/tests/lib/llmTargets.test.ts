import { describe, expect, it } from 'vitest'

import type { LlmTarget } from '@/lib/api/schemas'
import {
  DEFAULT_LLM_TARGET,
  preferredDefaultModel,
  withSelectedTarget,
  type SelectableLlmModel,
} from '@/lib/llmTargets'

const localModel = (modelId: string): SelectableLlmModel => ({
  model: {
    target: { kind: 'local', modelId },
    name: modelId,
    languages: ['en-US'],
  },
})

const providerModel = (
  providerId: string,
  modelId: string,
  name = modelId,
): SelectableLlmModel => ({
  model: {
    target: { kind: 'provider', providerId, modelId },
    name,
    languages: ['en-US'],
  },
  provider: {
    id: providerId,
    name: providerId,
    requiresApiKey: true,
    requiresBaseUrl: false,
    hasApiKey: true,
    baseUrl: null,
    status: 'ready',
    error: null,
    models: [],
  },
})

describe('preferredDefaultModel', () => {
  it('never falls back to an arbitrary (local) model', () => {
    // A local-only catalog used to auto-select models[0], and the auto-loader
    // would then download its multi-GB weights unprompted.
    const models = [localModel('vntl-llama3-8b-v2'), localModel('hunyuan-mt-7b')]
    expect(preferredDefaultModel(models)).toBeUndefined()
  })

  it('still picks the explicit default when present', () => {
    const models = [
      localModel('vntl-llama3-8b-v2'),
      providerModel('claude', DEFAULT_LLM_TARGET.modelId),
    ]
    expect(preferredDefaultModel(models)?.target).toEqual(DEFAULT_LLM_TARGET)
  })
})

describe('withSelectedTarget', () => {
  const saved: LlmTarget = {
    kind: 'provider',
    providerId: 'openai-compatible',
    modelId: 'anthropic/claude-opus-4.5',
  }

  it('synthesizes an option for a saved target the catalog does not list', () => {
    const models = [localModel('vntl-llama3-8b-v2')]
    const out = withSelectedTarget(models, saved, 'en-US')
    expect(out).toHaveLength(2)
    expect(out[0].model.target).toEqual(saved)
    expect(out[0].model.name).toBe('anthropic/claude-opus-4.5')
    expect(out[0].model.languages).toEqual(['en-US'])
  })

  it('returns the catalog unchanged when the target is already listed', () => {
    const models = [providerModel('openai-compatible', 'anthropic/claude-opus-4.5')]
    expect(withSelectedTarget(models, saved, 'en-US')).toBe(models)
  })

  it('returns the catalog unchanged without a saved target', () => {
    const models = [localModel('vntl-llama3-8b-v2')]
    expect(withSelectedTarget(models, undefined, 'en-US')).toBe(models)
  })
})
