'use client'

import { useEffect } from 'react'

import { closeProject, reopenLastProject } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

const BACK = 3
const FORWARD = 4

/** A modal dialog or a menu is open: the side buttons leave it alone. */
function somethingOpen(): boolean {
  return !!document.querySelector(
    '[data-slot="dialog-content"], [data-slot="alert-dialog-content"], [role="menu"]',
  )
}

/**
 * Mouse side buttons: back leaves the open project (like the back arrow),
 * forward reopens the project opened last. The webview's own history
 * back/forward is suppressed; there is no page history to go through.
 */
export function useMouseNavigation(hasProject: boolean) {
  useEffect(() => {
    const suppress = (event: MouseEvent) => {
      if (event.button === BACK || event.button === FORWARD) event.preventDefault()
    }
    const onMouseUp = (event: MouseEvent) => {
      if (event.button !== BACK && event.button !== FORWARD) return
      event.preventDefault()
      if (somethingOpen()) return
      const run =
        event.button === BACK
          ? hasProject
            ? closeProject()
            : undefined
          : hasProject
            ? undefined
            : reopenLastProject()
      void run?.catch((err) => useEditorUiStore.getState().showError(String(err)))
    }
    window.addEventListener('mousedown', suppress)
    window.addEventListener('auxclick', suppress)
    window.addEventListener('mouseup', onMouseUp)
    return () => {
      window.removeEventListener('mousedown', suppress)
      window.removeEventListener('auxclick', suppress)
      window.removeEventListener('mouseup', onMouseUp)
    }
  }, [hasProject])
}
