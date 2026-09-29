import { renderHook } from '@testing-library/react'
import { useDrag } from '@use-gesture/react'
import { describe, expect, it } from 'vitest'

import { useBlockDrafting } from '@/hooks/useBlockDrafting'

// The box editor's text fields sit inside the canvas element this gesture is
// bound to. use-gesture's keyboard drag would preventDefault every arrow key
// bubbling up from them, freezing the caret. It must be off, and the switch
// only works under `pointer`.
describe('canvas drag gesture', () => {
  it('binds no key handlers, so arrow keys reach the box editor', () => {
    const { result } = renderHook(() =>
      useBlockDrafting({
        mode: 'select',
        page: null,
        areaSelect: true,
        pointerToDocument: () => null,
        clearSelection: () => {},
        onCreateBlock: () => {},
      }),
    )
    const handlers = Object.keys(result.current.bind())
    expect(handlers).not.toContain('onKeyDown')
    expect(handlers).not.toContain('onKeyUp')
  })

  it('a top-level keys option is ignored by use-gesture', () => {
    const { result } = renderHook(() =>
      // @ts-expect-error -- not a drag option; kept to document the trap
      useDrag(() => {}, { keys: false }),
    )
    expect(Object.keys(result.current())).toContain('onKeyDown')
  })
})
