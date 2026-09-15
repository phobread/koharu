'use client'

import * as React from 'react'
import { useEffect, useRef, useState } from 'react'

import { Textarea } from '@/components/ui/textarea'

export type DraftTextareaProps = Omit<
  React.ComponentProps<typeof Textarea>,
  'value' | 'onChange'
> & {
  value: string
  onValueChange: (value: string, element: HTMLTextAreaElement) => void
  /** Allow an authoritative parent to replace the draft while focused. */
  syncWhileFocused?: boolean
}

export function DraftTextarea({
  value,
  onValueChange,
  syncWhileFocused = false,
  onFocus,
  onBlur,
  onCompositionStart,
  onCompositionEnd,
  ...props
}: DraftTextareaProps) {
  const [draftValue, setDraftValue] = useState(value)
  const draftValueRef = useRef(value)
  const isFocusedRef = useRef(false)
  const isComposingRef = useRef(false)
  const pendingCommitRef = useRef<string | null>(null)
  const lastExternalValueRef = useRef(value)

  const commitValue = (nextValue: string, element: HTMLTextAreaElement) => {
    pendingCommitRef.current = null
    onValueChange(nextValue, element)
  }

  useEffect(() => {
    draftValueRef.current = draftValue
  }, [draftValue])

  useEffect(() => {
    lastExternalValueRef.current = value

    // While the user is composing, preserve the IME's active draft. Ordinary
    // focused edits also stay local unless an authoritative parent explicitly
    // opts into replacing them (the rich editor filters stale acknowledgements
    // before enabling this path).
    if (isComposingRef.current || (isFocusedRef.current && !syncWhileFocused)) {
      return
    }

    setDraftValue(value)
  }, [syncWhileFocused, value])

  return (
    <Textarea
      {...props}
      value={draftValue}
      onFocus={(event) => {
        isFocusedRef.current = true
        onFocus?.(event)
      }}
      onBlur={(event) => {
        if (pendingCommitRef.current !== null) {
          commitValue(pendingCommitRef.current, event.currentTarget)
        }
        isComposingRef.current = false
        isFocusedRef.current = false
        onBlur?.(event)
      }}
      onCompositionStart={(event) => {
        isComposingRef.current = true
        onCompositionStart?.(event)
      }}
      onCompositionEnd={(event) => {
        isComposingRef.current = false
        const committedValue = event.currentTarget.value
        setDraftValue(committedValue)
        commitValue(committedValue, event.currentTarget)
        onCompositionEnd?.(event)
      }}
      onChange={(event) => {
        const nextValue = event.target.value
        setDraftValue(nextValue)
        if (isComposingRef.current) {
          pendingCommitRef.current = nextValue
          return
        }
        commitValue(nextValue, event.currentTarget)
      }}
    />
  )
}
