import type { TFunction } from 'i18next'

import { hideOfficialLettering } from '@/lib/api/default/default'
import { awaitPendingSceneEdits, invalidateScene, queueAutoRender } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

/**
 * Hide the official release's lettering at a point of the page: a line the
 * release put where the raw page has none, which cleanup keeps because no
 * text box claims it. The backend puts an empty text box over the whole piece
 * and cleans that spot again, as one undo step; the Translated view is
 * rendered again here. Tells the user what happened, also when there was
 * nothing to hide.
 */
export async function hideOfficialLetteringAt(
  pageId: string,
  point: { x: number; y: number },
  t: TFunction,
): Promise<void> {
  const ui = useEditorUiStore.getState()
  try {
    // The backend decides from the page's boxes: let queued edits land first.
    await awaitPendingSceneEdits()
    ui.showNotice(t('official.hiding', "Hiding the release's lettering…"))
    const res = await hideOfficialLettering(pageId, { x: point.x, y: point.y })
    if (res.status === 'hidden') {
      await invalidateScene()
      queueAutoRender(pageId)
      ui.showNotice(
        t(
          'official.hidden',
          "Hidden. An empty text box now keeps the release's lettering away from this spot. Delete the box to bring it back.",
        ),
      )
    } else if (res.status === 'stillShown') {
      ui.showNotice(
        t(
          'official.hideStillShown',
          "This lettering can't be hidden on its own: it goes with a sound in the same bubble. Put a text box on that sound instead.",
        ),
      )
    } else {
      ui.showNotice(
        t('official.hideNothing', 'There is no lettering from the release at this spot.'),
      )
    }
  } catch (e) {
    ui.showError(e instanceof Error ? e.message : String(e))
  }
}
