import type { Page } from '@/lib/api/schemas'

/** A step a page still lacks, as the page list names it. */
export type MissingStep = 'ocr' | 'translate' | 'clean' | 'render'

export type PageStatus =
  | { kind: 'notStarted' }
  | { kind: 'done' }
  | { kind: 'needs'; steps: MissingStep[] }

/**
 * What a page still needs, by the same rules as "Process unfinished pages"
 * (crates/koharu-app/src/pipeline/missing.rs): boxes never read and with no
 * translation need OCR, boxes with text but no translation need translating,
 * and a page without a cleaned or rendered image needs that step. A page
 * that was detected but has no boxes left is not "not started".
 */
export function pageStatus(page: Page): PageStatus {
  const nodes = Object.values(page.nodes)
  const texts = nodes.flatMap((n) => ('text' in n.kind ? [n.kind.text] : []))
  const has = (kind: 'image' | 'mask', role: string) =>
    nodes.some((n) => {
      const k = (n.kind as Record<string, { role?: string } | undefined>)[kind]
      return k?.role === role
    })

  if (texts.length === 0 && !has('mask', 'segment')) return { kind: 'notStarted' }

  const steps: MissingStep[] = []
  if (texts.some((t) => t.text == null && t.translation == null)) steps.push('ocr')
  if (texts.some((t) => !!t.text?.trim() && t.translation == null)) steps.push('translate')
  if (!has('image', 'inpainted')) steps.push('clean')
  if (!has('image', 'rendered')) steps.push('render')
  return steps.length === 0 ? { kind: 'done' } : { kind: 'needs', steps }
}
