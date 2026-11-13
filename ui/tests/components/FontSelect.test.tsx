import { describe, expect, it } from 'vitest'

import { FontSelect } from '@/components/ui/font-select'

import { renderWithQuery } from '../helpers'

const options = [
  {
    familyName: 'CC Wild Words',
    postScriptName: 'CCWildWordsRoman',
    source: 'system' as const,
    cached: true,
  },
]

describe('FontSelect', () => {
  it('shows the family of the chosen font', () => {
    const { getByTestId } = renderWithQuery(
      <FontSelect
        data-testid='font'
        value='CCWildWordsRoman'
        options={options}
        onChange={() => {}}
      />,
    )
    expect(getByTestId('font').textContent).toBe('CC Wild Words')
  })

  it('still names a font the list no longer offers', () => {
    // Windows' own fonts are left out of the list, but a box may use one.
    const { getByTestId } = renderWithQuery(
      <FontSelect
        data-testid='font'
        value='Comic Sans MS'
        options={options}
        placeholder='Auto'
        onChange={() => {}}
      />,
    )
    expect(getByTestId('font').textContent).toBe('Comic Sans MS')
  })

  it('falls back to the placeholder when no font is chosen', () => {
    const { getByTestId } = renderWithQuery(
      <FontSelect
        data-testid='font'
        value=''
        options={options}
        placeholder='Auto'
        onChange={() => {}}
      />,
    )
    expect(getByTestId('font').textContent).toBe('Auto')
  })
})
