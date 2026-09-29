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
  const texts = pageTexts(page)
  const has = (kind: 'image' | 'mask', role: string) => hasLayer(page, kind, role)

  if (texts.length === 0 && !has('mask', 'segment')) return { kind: 'notStarted' }

  const steps: MissingStep[] = []
  if (texts.some((t) => t.text == null && t.translation == null)) steps.push('ocr')
  if (texts.some((t) => !!t.text?.trim() && t.translation == null)) steps.push('translate')
  if (!has('image', 'inpainted')) steps.push('clean')
  if (!has('image', 'rendered')) steps.push('render')
  return steps.length === 0 ? { kind: 'done' } : { kind: 'needs', steps }
}

function pageTexts(page: Page) {
  return Object.values(page.nodes).flatMap((n) => ('text' in n.kind ? [n.kind.text] : []))
}

function hasLayer(page: Page, kind: 'image' | 'mask', role: string): boolean {
  return Object.values(page.nodes).some((n) => {
    const k = (n.kind as Record<string, { role?: string } | undefined>)[kind]
    return k?.role === role
  })
}

/** What an image export sent out, by 1-based page number. */
export type ExportSummary = {
  count: number
  /** Translated, but not rendered yet: the image has no translation. */
  unrendered: number[]
  /** No translation: the cleaned image went out. */
  cleaned: number[]
  /** Not cleaned yet: the original went out. */
  original: number[]
}

/**
 * Summarise an export of `ids` (every page when omitted), taking from each
 * page what the server does (crates/koharu-rpc/src/routes/projects.rs,
 * `ExportFormat::Best`): the rendered image, else the cleaned one, else the
 * original.
 */
export function exportSummary(pages: Record<string, Page>, ids?: string[]): ExportSummary {
  const wanted = ids ? new Set(ids) : undefined
  const summary: ExportSummary = { count: 0, unrendered: [], cleaned: [], original: [] }
  Object.entries(pages).forEach(([id, page], index) => {
    if (wanted && !wanted.has(id)) return
    const number = index + 1
    const rendered = hasLayer(page, 'image', 'rendered')
    const cleaned = hasLayer(page, 'image', 'inpainted')
    if (!rendered && !cleaned && !hasLayer(page, 'image', 'source')) return
    summary.count += 1
    if (rendered) return
    if (pageTexts(page).some((t) => !!t.translation?.trim())) summary.unrendered.push(number)
    else if (cleaned) summary.cleaned.push(number)
    else summary.original.push(number)
  })
  return summary
}
