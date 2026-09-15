import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { UpdaterProvider } from '@/components/Updater'

const check = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/plugin-updater', () => ({ check }))
vi.mock('@/lib/backend', () => ({ isTauri: () => true, openExternalUrl: vi.fn() }))

describe('personal fork startup', () => {
  it('shows the app without checking upstream or displaying an update prompt', () => {
    render(
      <UpdaterProvider>
        <div>Ready to translate</div>
      </UpdaterProvider>,
    )
    expect(screen.getByText('Ready to translate')).toBeInTheDocument()
    expect(check).not.toHaveBeenCalled()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })
})
