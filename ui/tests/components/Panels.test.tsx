import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const auth = vi.hoisted(() => ({ signedIn: false }))
vi.mock('@/lib/api/default/default', () => ({
  useGetCodexAuthStatus: () => ({ data: auth }),
}))
vi.mock('@/components/panels/TextBlocksPanel', () => ({
  TextBlocksPanel: function Draft() {
    const [value, setValue] = useState('')
    return (
      <input
        aria-label='Local translation draft'
        value={value}
        onChange={(event) => setValue(event.target.value)}
      />
    )
  },
}))
vi.mock('@/components/panels/RenderControlsPanel', () => ({
  RenderControlsPanel: () => <div>Render controls</div>,
}))
vi.mock('@/components/panels/LayersPanel', () => ({
  LayersPanel: () => <div>Layers controls</div>,
}))
vi.mock('@/components/panels/AiPanel', () => ({ AiPanel: () => <div>AI controls</div> }))

import { Panels } from '@/components/Panels'

beforeEach(() => {
  auth.signedIn = false
})

describe('Inspector tabs', () => {
  it('retains an unsaved draft across Properties and Layers visits', async () => {
    const user = userEvent.setup()
    render(<Panels />)
    const draft = screen.getByRole('textbox', { name: 'Local translation draft' })
    await user.type(draft, 'Keep this draft')
    await user.click(screen.getByTestId('panels-tab-layout'))
    expect(screen.getByTestId('panels-layout')).toHaveAttribute('data-state', 'active')
    await user.click(screen.getByTestId('panels-tab-layers'))
    await user.click(screen.getByTestId('panels-tab-textblocks'))
    expect(screen.getByRole('textbox', { name: 'Local translation draft' })).toBe(draft)
    expect(draft).toHaveValue('Keep this draft')
  })

  it('supports arrow-key navigation between inspector sections', async () => {
    const user = userEvent.setup()
    render(<Panels />)
    screen.getByTestId('panels-tab-textblocks').focus()
    await user.keyboard('{ArrowRight}')
    await waitFor(() => expect(screen.getByTestId('panels-tab-layout')).toHaveFocus())
    expect(screen.getByTestId('panels-tab-layout')).toHaveAttribute('aria-selected', 'true')
    await user.keyboard('{End}')
    await waitFor(() => expect(screen.getByTestId('panels-tab-layers')).toHaveFocus())
  })

  it('preserves the signed-in AI panel and returns to Text after sign-out', async () => {
    const user = userEvent.setup()
    auth.signedIn = true
    const view = render(<Panels />)
    await user.click(screen.getByTestId('panels-tab-ai'))
    expect(screen.getByTestId('panels-ai')).toHaveAttribute('data-state', 'active')
    auth.signedIn = false
    view.rerender(<Panels />)
    await waitFor(() =>
      expect(screen.getByTestId('panels-tab-textblocks')).toHaveAttribute('aria-selected', 'true'),
    )
    expect(screen.queryByTestId('panels-tab-ai')).not.toBeInTheDocument()
    expect(screen.queryByText('AI controls')).not.toBeInTheDocument()
  })
})
