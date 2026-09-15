import { fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { RichTextDraftTextarea } from '@/components/ui/rich-text-draft-textarea'

describe('RichTextDraftTextarea', () => {
  it('retains the latest draft ranges when an earlier queued edit is acknowledged', async () => {
    const onPatch = vi.fn()
    const props = {
      'data-testid': 'translation',
      inheritedColor: [0, 0, 0, 255],
      onPatch,
      splitLabel: 'Split',
      onSplit: vi.fn(),
    }
    const { rerender } = render(<RichTextDraftTextarea {...props} value='猫' styleRanges={[]} />)
    const textarea = screen.getByTestId('translation') as HTMLTextAreaElement
    await userEvent.click(textarea)
    fireEvent.change(textarea, { target: { value: '猫🙂' } })
    fireEvent.change(textarea, { target: { value: '猫🙂犬' } })
    rerender(<RichTextDraftTextarea {...props} value='猫🙂' styleRanges={[]} />)
    textarea.setSelectionRange(3, 4)
    fireEvent.select(textarea)
    await userEvent.click(screen.getByRole('button', { name: 'render.effectBold' }))
    expect(onPatch).toHaveBeenLastCalledWith({
      styleRanges: [{ start: 7, end: 10, style: { bold: true } }],
    })

    // Saving the second text edit must not erase the newer formatting either.
    rerender(<RichTextDraftTextarea {...props} value='猫🙂犬' styleRanges={[]} />)
    fireEvent.change(textarea, { target: { value: '猫🙂犬!' } })
    expect(onPatch).toHaveBeenLastCalledWith({
      translation: '猫🙂犬!',
      styleRanges: [{ start: 7, end: 10, style: { bold: true } }],
    })
    fireEvent.blur(textarea)
    rerender(
      <RichTextDraftTextarea
        {...props}
        value='猫🙂犬'
        styleRanges={[{ start: 7, end: 10, style: { bold: true } }]}
      />,
    )
    expect(textarea.value).toBe('猫🙂犬!')
    // An unrelated external state, such as undo, is accepted.
    rerender(<RichTextDraftTextarea {...props} value='undo' styleRanges={[]} />)
    fireEvent.change(textarea, { target: { value: 'undo!' } })
    expect(onPatch).toHaveBeenLastCalledWith({ translation: 'undo!', styleRanges: [] })
  })

  it('formats only the selected word', async () => {
    const onPatch = vi.fn()
    render(
      <RichTextDraftTextarea
        data-testid='translation'
        value='Hello world'
        styleRanges={[]}
        inheritedColor={[0, 0, 0, 255]}
        onPatch={onPatch}
        splitLabel='Split'
        onSplit={vi.fn()}
      />,
    )

    const textarea = screen.getByTestId('translation') as HTMLTextAreaElement
    textarea.setSelectionRange(6, 11)
    fireEvent.select(textarea)
    await userEvent.click(screen.getByRole('button', { name: 'render.effectBold' }))

    expect(onPatch).toHaveBeenCalledWith({
      styleRanges: [{ start: 6, end: 11, style: { bold: true } }],
    })
  })

  it('rebases formatting when the translation is edited', () => {
    const onPatch = vi.fn()
    render(
      <RichTextDraftTextarea
        data-testid='translation'
        value='Hello world'
        styleRanges={[{ start: 6, end: 11, style: { italic: true } }]}
        inheritedColor={[0, 0, 0, 255]}
        onPatch={onPatch}
        splitLabel='Split'
        onSplit={vi.fn()}
      />,
    )

    fireEvent.change(screen.getByTestId('translation'), { target: { value: 'Hello brave world' } })
    expect(onPatch).toHaveBeenCalledWith({
      translation: 'Hello brave world',
      styleRanges: [{ start: 12, end: 17, style: { italic: true } }],
    })
  })

  it('replaces the focused draft when an unrelated external change arrives', async () => {
    const onPatch = vi.fn()
    const props = {
      'data-testid': 'translation',
      inheritedColor: [0, 0, 0, 255],
      onPatch,
      splitLabel: 'Split',
      onSplit: vi.fn(),
    }
    const { rerender } = render(
      <RichTextDraftTextarea {...props} value='original' styleRanges={[]} />,
    )
    const textarea = screen.getByTestId('translation') as HTMLTextAreaElement
    await userEvent.click(textarea)
    fireEvent.change(textarea, { target: { value: 'local draft' } })

    rerender(<RichTextDraftTextarea {...props} value='undone' styleRanges={[]} />)

    expect(textarea.value).toBe('undone')
  })

  it('uses the current selection after replacing selected text', async () => {
    const onPatch = vi.fn()
    render(
      <RichTextDraftTextarea
        data-testid='translation'
        value='Hello world'
        styleRanges={[]}
        inheritedColor={[0, 0, 0, 255]}
        onPatch={onPatch}
        splitLabel='Split'
        onSplit={vi.fn()}
      />,
    )

    const textarea = screen.getByTestId('translation') as HTMLTextAreaElement
    textarea.setSelectionRange(6, 11)
    fireEvent.select(textarea)
    fireEvent.change(textarea, { target: { value: 'Hello X' } })
    textarea.setSelectionRange(7, 7)

    const bold = screen.getByRole('button', { name: 'render.effectBold' })
    expect(bold).toBeDisabled()
    expect(onPatch).toHaveBeenLastCalledWith({
      translation: 'Hello X',
      styleRanges: [],
    })
  })
})
