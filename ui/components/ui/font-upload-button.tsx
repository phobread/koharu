'use client'

import { useQueryClient } from '@tanstack/react-query'
import { UploadIcon } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { getListFontsQueryKey, uploadFont } from '@/lib/api/default/default'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { cn } from '@/lib/utils'

const ACCEPT = '.ttf,.otf,.ttc,.otc,.woff,.woff2'

/**
 * Icon button that lets the user pick a font file from disk and import it.
 * On success the font list is refetched and the new face is handed back via
 * `onUploaded` so the caller can select it.
 */
export function FontUploadButton({
  onUploaded,
  className,
  disabled,
}: {
  onUploaded?: (postScriptName: string) => void
  className?: string
  disabled?: boolean
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const inputRef = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const showError = useEditorUiStore((s) => s.showError)

  const handleFile = async (file: File) => {
    setBusy(true)
    try {
      const buffer = await file.arrayBuffer()
      const added = await uploadFont({ filename: file.name }, new Blob([buffer]))
      await queryClient.invalidateQueries({ queryKey: getListFontsQueryKey() })
      const postScriptName = added?.[0]?.postScriptName
      if (postScriptName) onUploaded?.(postScriptName)
    } catch (e) {
      showError(t('render.uploadFontError', { defaultValue: 'Could not import that font file' }))
      console.error('font upload failed', e)
    } finally {
      setBusy(false)
      if (inputRef.current) inputRef.current.value = ''
    }
  }

  return (
    <>
      <input
        ref={inputRef}
        type='file'
        accept={ACCEPT}
        className='hidden'
        data-testid='font-upload-input'
        onChange={(e) => {
          const file = e.target.files?.[0]
          if (file) void handleFile(file)
        }}
      />
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            type='button'
            variant='outline'
            size='icon-sm'
            className={cn('shrink-0', className)}
            disabled={disabled || busy}
            data-testid='font-upload-button'
            aria-label={t('render.uploadFont', { defaultValue: 'Upload font' })}
            onClick={() => inputRef.current?.click()}
          >
            <UploadIcon className='size-3.5' />
          </Button>
        </TooltipTrigger>
        <TooltipContent side='bottom' sideOffset={4}>
          {t('render.uploadFont', { defaultValue: 'Upload font' })}
        </TooltipContent>
      </Tooltip>
    </>
  )
}
