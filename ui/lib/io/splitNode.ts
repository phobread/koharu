import { isTextNode } from '@/hooks/useCurrentPage'
import type { Node, NodeDataPatch, Page, Transform } from '@/lib/api/schemas'
import { applyOp, queueAutoRender } from '@/lib/io/scene'
import { ops } from '@/lib/ops'
import { mergeTextBlocks, type BlockSplit } from '@/lib/splitBlock'
import { useSelectionStore } from '@/lib/stores/selectionStore'

/**
 * Apply a computed block split to the scene: the original node keeps half A;
 * a new node takes half B (style / font prediction / direction copied, stale
 * sprite dropped so it re-renders). Both halves lock their layout box — the
 * split is an explicit size choice. Selects both halves and queues a
 * re-render.
 */
export async function applyBlockSplit(page: Page, nodeId: string, split: BlockSplit) {
  const node = page.nodes[nodeId]
  if (!node || !isTextNode(node) || !node.transform) return
  const data = node.kind.text

  const at = Object.keys(page.nodes).length
  const newId = crypto.randomUUID()
  const updateA = ops.updateNode(page.id, nodeId, {
    transform: split.a.transform,
    data: {
      text: {
        text: split.a.text,
        translation: split.a.translation,
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
        style: data.style ?? undefined,
        fontPrediction: data.fontPrediction ?? undefined,
        sourceDirection: data.sourceDirection ?? undefined,
        sourceLang: data.sourceLang ?? undefined,
        lockLayoutBox: true,
      },
    },
  }
  await applyOp(ops.batch('Split block', [updateA, ops.addNode(page.id, at, newNode)]))
  useSelectionStore.getState().selectMany([nodeId, newId])
  queueAutoRender(page.id)
}

/**
 * Merge the selected text blocks back into one (the inverse of a split, and
 * a fix-up for detector over-segmentation): the first block in page/reading
 * order survives with the union box and the joined texts; the rest are
 * removed. Selects the survivor and queues a re-render.
 */
export async function applyBlockMerge(page: Page, nodeIds: string[]) {
  const wanted = new Set(nodeIds)
  // Page order = reading order, so texts join in the order they're read.
  const entries: { node: Node; transform: Transform; text?: string | null; translation?: string | null }[] = []
  for (const id of Object.keys(page.nodes)) {
    if (!wanted.has(id)) continue
    const n = page.nodes[id]
    if (!n || !isTextNode(n) || !n.transform) continue
    entries.push({
      node: n,
      transform: n.transform,
      text: n.kind.text.text,
      translation: n.kind.text.translation,
    })
  }
  if (entries.length < 2) return

  const merged = mergeTextBlocks(entries)
  if (!merged) return

  const nodes = entries.map((e) => e.node)
  const survivor = nodes[0]
  const batch = [
    ops.updateNode(page.id, survivor.id, {
      transform: merged.transform,
      data: {
        text: {
          text: merged.text,
          translation: merged.translation,
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
  await applyOp(ops.batch('Merge blocks', batch))
  useSelectionStore.getState().selectMany([survivor.id])
  queueAutoRender(page.id)
}
