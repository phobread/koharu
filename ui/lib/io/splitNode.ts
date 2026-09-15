import { isTextNode } from '@/hooks/useCurrentPage'
import type { Node, NodeDataPatch, Page, TextStyleRange, Transform } from '@/lib/api/schemas'
import { applyOp, applyOpFromScene, queueAutoRender } from '@/lib/io/scene'
import { ops } from '@/lib/ops'
import { sliceTextStyleRanges, utf16OffsetToUtf8 } from '@/lib/richText'
import {
  mergeTextBlocks,
  splitTextBlock,
  splitTextBlockAt,
  type BlockSplit,
  type SplitField,
} from '@/lib/splitBlock'
import { useSelectionStore } from '@/lib/stores/selectionStore'

/**
 * Apply a computed block split to the scene: the original node keeps half A;
 * a new node takes half B (style / font prediction / direction copied, stale
 * sprite dropped so it re-renders). Both halves lock their layout box — the
 * split is an explicit size choice.
 *
 * Only half A stays selected. Splitting exists mostly to isolate unwanted
 * text (dates, SFX) from dialogue — keeping both halves selected made the
 * follow-up right-click act on BOTH (e.g. un-inpaint restored the half the
 * user wanted to keep). A single selection also reopens the quick editor on
 * the surviving half.
 */
export async function applyBlockSplit(page: Page, nodeId: string, split: BlockSplit) {
  const op = buildSplitOp(page, nodeId, split)
  if (!op) return
  await applyOp(op)
  useSelectionStore.getState().selectMany([nodeId])
  queueAutoRender(page.id)
}

/** Caret comes from the local editor; text/ranges come from the saved state
 * after its queued edits finish, never from an older component snapshot. */
export async function splitBlock(
  pageId: string,
  nodeId: string,
  cut?: { field: SplitField; offset: number },
) {
  const applied = await applyOpFromScene((scene) => {
    const page = scene.pages[pageId]
    const node = page?.nodes[nodeId]
    if (!node || !isTextNode(node) || !node.transform) return null
    const data = node.kind.text
    const direction =
      (cut?.field === 'text'
        ? data.sourceDirection
        : (data.writingDirection ?? data.renderedDirection ?? data.sourceDirection)) ?? 'horizontal'
    const split = cut
      ? splitTextBlockAt(node.transform, data, cut, direction)
      : splitTextBlock(node.transform, data, direction)
    return split ? (buildSplitOp(page, nodeId, split) ?? null) : null
  })
  if (applied) {
    useSelectionStore.getState().selectMany([nodeId])
    queueAutoRender(pageId)
  }
}

function buildSplitOp(page: Page, nodeId: string, split: BlockSplit) {
  const node = page.nodes[nodeId]
  if (!node || !isTextNode(node) || !node.transform) return
  const data = node.kind.text

  const at = Object.keys(page.nodes).indexOf(nodeId) + 1
  const newId = crypto.randomUUID()
  const originalTranslation = data.translation ?? ''
  const rangesA = sliceTextStyleRanges(
    originalTranslation,
    data.styleRanges ?? [],
    split.a.translationSpan.start,
    split.a.translationSpan.end,
  )
  const rangesB = sliceTextStyleRanges(
    originalTranslation,
    data.styleRanges ?? [],
    split.b.translationSpan.start,
    split.b.translationSpan.end,
  )
  const updateA = ops.updateNode(page.id, nodeId, {
    transform: split.a.transform,
    data: {
      text: {
        text: split.a.text,
        translation: split.a.translation ?? '',
        styleRanges: rangesA,
        lockLayoutBox: true,
      },
    } as NodeDataPatch,
  })
  const newNode: Node = {
    id: newId,
    transform: split.b.transform,
    visible: true,
    kind: {
      text: {
        text: split.b.text,
        translation: split.b.translation,
        styleRanges: rangesB,
        style: data.style ?? undefined,
        fontPrediction: data.fontPrediction ?? undefined,
        sourceDirection: data.sourceDirection ?? undefined,
        renderedDirection: data.renderedDirection ?? undefined,
        writingDirection: data.writingDirection ?? undefined,
        sourceLang: data.sourceLang ?? undefined,
        lockLayoutBox: true,
      },
    },
  }
  return ops.batch('Split block', [updateA, ops.addNode(page.id, at, newNode)])
}

/**
 * Merge the selected text blocks back into one (the inverse of a split, and
 * a fix-up for detector over-segmentation): the first block in page/reading
 * order survives with the union box and the joined texts; the rest are
 * removed. Selects the survivor and queues a re-render.
 */
export async function applyBlockMerge(page: Page, nodeIds: string[]) {
  const merged = buildMergeOp(page, nodeIds)
  if (!merged) return
  await applyOp(merged.op)
  useSelectionStore.getState().selectMany([merged.survivorId])
  queueAutoRender(page.id)
}

export async function mergeBlocks(pageId: string, nodeIds: string[]) {
  let survivorId: string | undefined
  const applied = await applyOpFromScene((scene) => {
    const page = scene.pages[pageId]
    if (!page) return null
    const merged = buildMergeOp(page, nodeIds)
    survivorId = merged?.survivorId
    return merged?.op ?? null
  })
  if (applied && survivorId) {
    useSelectionStore.getState().selectMany([survivorId])
    queueAutoRender(pageId)
  }
}

function buildMergeOp(page: Page, nodeIds: string[]) {
  const wanted = new Set(nodeIds)
  // Page order = reading order, so texts join in the order they're read.
  const entries: {
    node: Node
    transform: Transform
    text?: string | null
    translation?: string | null
    styleRanges: TextStyleRange[]
  }[] = []
  for (const id of Object.keys(page.nodes)) {
    if (!wanted.has(id)) continue
    const n = page.nodes[id]
    if (!n || !isTextNode(n) || !n.transform) continue
    entries.push({
      node: n,
      transform: n.transform,
      text: n.kind.text.text,
      translation: n.kind.text.translation,
      styleRanges: n.kind.text.styleRanges ?? [],
    })
  }
  if (entries.length < 2) return

  const merged = mergeTextBlocks(entries)
  if (!merged) return

  const nodes = entries.map((e) => e.node)
  const survivor = nodes[0]
  const mergedRanges = mergeStyleRanges(
    merged.translation ?? '',
    entries.map((entry) => ({
      text: entry.translation ?? '',
      ranges: entry.styleRanges,
    })),
  )
  const batch = [
    ops.updateNode(page.id, survivor.id, {
      transform: merged.transform,
      data: {
        text: {
          text: merged.text,
          translation: merged.translation ?? '',
          styleRanges: mergedRanges,
          lockLayoutBox: true,
        },
      } as NodeDataPatch,
    }),
  ]
  // Removal indices must track the shrinking node list so undo re-inserts
  // each node where it actually was.
  const keys = Object.keys(page.nodes)
  for (const n of nodes.slice(1)) {
    const idx = keys.indexOf(n.id)
    batch.push(ops.removeNode(page.id, n.id, n, idx < 0 ? 0 : idx))
    if (idx >= 0) keys.splice(idx, 1)
  }
  return { op: ops.batch('Merge blocks', batch), survivorId: survivor.id }
}

function mergeStyleRanges(
  mergedText: string,
  parts: { text: string; ranges: TextStyleRange[] }[],
): TextStyleRange[] {
  const merged: TextStyleRange[] = []
  let searchFrom = 0
  for (const part of parts) {
    const fragment = part.text.trim()
    if (!fragment) continue
    const index = mergedText.indexOf(fragment, searchFrom)
    if (index < 0) continue
    const byteOffset = utf16OffsetToUtf8(mergedText, index)
    const trimStart = part.text.length - part.text.trimStart().length
    const local = sliceTextStyleRanges(
      part.text,
      part.ranges,
      trimStart,
      trimStart + fragment.length,
    )
    merged.push(
      ...local.map((range) => ({
        ...range,
        start: range.start + byteOffset,
        end: range.end + byteOffset,
      })),
    )
    searchFrom = index + fragment.length
  }
  return merged
}
