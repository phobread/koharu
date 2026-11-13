import { afterEach, describe, expect, it, vi } from 'vitest'

import { renderDefaultsForPipeline } from '@/lib/io/renderDefaults'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'

type Preferences = ReturnType<typeof usePreferencesStore.getState>

/** Preferences as saved, with some changed; nothing is written back. */
function withPreferences(changed: Partial<Preferences>) {
  const saved = usePreferencesStore.getState()
  vi.spyOn(usePreferencesStore, 'getState').mockReturnValue({ ...saved, ...changed })
}

describe('renderDefaultsForPipeline', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('lets text over artwork grow only when the switch in Settings is on', () => {
    expect(renderDefaultsForPipeline().growOverArtwork).toBe(false)
    withPreferences({ growTextOverArtwork: true })
    expect(renderDefaultsForPipeline().growOverArtwork).toBe(true)
  })

  it('sends the box padding along', () => {
    withPreferences({ boxPadding: 3 })
    expect(renderDefaultsForPipeline().boxPadding).toBe(3)
  })
})
