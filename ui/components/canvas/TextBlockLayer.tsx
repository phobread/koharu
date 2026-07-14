'use client'

import { useDrag } from '@use-gesture/react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useHotkeys } from 'react-hotkeys-hook'

import { BlockQuickEditor } from '@/components/canvas/BlockQuickEditor'
import { useBlobImage } from '@/hooks/useBlobData'
import {
  findImageBlob,
  isTextNode,
  useCurrentPage,
  useTextNodes,
  type TextNodeEntry,
} from '@/hooks/useCurrentPage'
import type { NodeDataPatch, Transform } from '@/lib/api/schemas'
import { applyOp, queueAutoRender } from '@/lib/io/scene'
import { ops } from '@/lib/ops'
import {
  cornerScaleFactor,
  resizeRotatedBox,
  scaleRotatedBox,
  snapRotationDeg,
  type Box,
} from '@/lib/rotatedBox'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'
import { mergeTextStyle } from '@/lib/textStyle'

/** Explicit font sizes committed by a corner-drag scale stay in sane bounds. */
const MIN_SCALED_FONT_PX = 4
const MAX_SCALED_FONT_PX = 300

type TextBlockLayerProps = {
  showSprites?: boolean
  scale: number
  style?: React.CSSProperties
}

/**
 * Overlay for the active page's Text nodes. Each rectangle is draggable /
 * resizable; commits dispatch `Op::UpdateNode { transform }` through
 * `applyCommand`. Selection is driven by `selectionStore.nodeIds`.
 */
export function TextBlockLayer({ showSprites, scale, style }: TextBlockLayerProps) {
  const nodes = useTextNodes()
  const page = useCurrentPage()
  const selectedIds = useSelectionStore((s) => s.nodeIds)
  const select = useSelectionStore((s) => s.select)
  const mode = useEditorUiStore((s) => s.mode)
  const interactive = mode === 'select' || mode === 'block'

  const hasSelection = useMemo(() => {
    for (const id of selectedIds) if (id) return true
    return false
  }, [selectedIds])

  const removeNode = async (id: string) => {
    if (!page) return
    const node = page.nodes[id]
    if (!node) return
    const idx = Object.keys(page.nodes).indexOf(id)
    await applyOp(ops.removeNode(page.id, id, node, idx < 0 ? 0 : idx))
    if ('text' in node.kind) queueAutoRender(page.id)
  }

  const removeSelected = async () => {
    if (!page) return
    // Snapshot selection now: each op invalidates the page state by removing a
    // node, so we can't iterate against a stale closure mid-loop.
    const ids = Array.from(selectedIds).filter((id): id is string => !!id)
    for (const id of ids) {
      await removeNode(id)
    }
  }

  // Live sprite preview while a corner drag scales a block (Canva-style).
  const [spritePreview, setSpritePreview] = useState<{ id: string; factor: number } | null>(null)

  // Quick editor: floats next to the block when exactly one is selected, so
  // the OCR'd source and translation can be checked/fixed in place. The ✕
  // hides it for that block; deselecting resets so a fresh click reopens it.
  const [quickEditorHiddenFor, setQuickEditorHiddenFor] = useState<string | null>(null)
  const selectedTextNodes = useMemo(
    () => nodes.filter((n) => selectedIds.has(n.id)),
    [nodes, selectedIds],
  )
  const quickEditNode = interactive && selectedTextNodes.length === 1 ? selectedTextNodes[0] : null
  const quickEditNodeId = quickEditNode?.id ?? null
  useEffect(() => {
    if (!quickEditNodeId) setQuickEditorHiddenFor(null)
  }, [quickEditNodeId])

  // While the quick editor is open, the edited block can reveal the original
  // art inside its box (sprite hidden) so the source text is right there to
  // compare against — or show the translated result instead. The choice is a
  // toggle in the editor header and sticks for the session.
  const [showOriginalUnderEdit, setShowOriginalUnderEdit] = useState(true)
  const editingNode =
    quickEditNode && quickEditorHiddenFor !== quickEditNode.id ? quickEditNode : null
  const peekNode = showOriginalUnderEdit ? editingNode : null
  const { data: originalSrc } = useBlobImage((page && findImageBlob(page, 'source')) ?? undefined)

  const updateTransform = async (id: string, t: Transform, scaleFactor?: number) => {
    if (!page) return
    const node = page.nodes[id]
    // Corner drags scale the text with the box: multiply the block's current
    // size (explicit override, else the last auto-fit result) and persist it
    // as an explicit override so the re-render honours the new size.
    let scaledStyle
    if (scaleFactor && node && isTextNode(node)) {
      const data = node.kind.text
      const base = data.style?.fontSize ?? data.renderedFontSizePx
      if (base) {
        const next = Math.min(Math.max(base * scaleFactor, MIN_SCALED_FONT_PX), MAX_SCALED_FONT_PX)
        scaledStyle = mergeTextStyle(data.style, {
          fontSize: Math.round(next * 10) / 10,
        })
      }
    }
    const patch: NodeDataPatch = {
      text: scaledStyle ? { lockLayoutBox: true, style: scaledStyle } : { lockLayoutBox: true },
    }
    await applyOp(ops.updateNode(page.id, id, { transform: t, data: patch }))
    queueAutoRender(page.id)
  }

  useHotkeys(
    'delete',
    () => {
      if (hasSelection && interactive) void removeSelected()
    },
    { enabled: hasSelection && interactive },
    [selectedIds, interactive],
  )

  return (
    <div
      data-text-block-layer
      style={{
        ...style,
        position: 'absolute',
        inset: 0,
        width: '100%',
        height: '100%',
        pointerEvents: 'none',
      }}
    >
      {showSprites &&
        nodes
          .filter((n) => n.id !== peekNode?.id)
          .map((n, i) => (
            <BlockSprite
              key={`sprite-${n.id ?? i}`}
              node={n}
              scale={scale}
              previewFactor={spritePreview?.id === n.id ? spritePreview.factor : undefined}
            />
          ))}
      {page && peekNode && originalSrc && (
        <OriginalArtPeek page={page} node={peekNode} scale={scale} src={originalSrc} />
      )}
      {nodes.map((n, i) => (
        <TextBlockItem
          key={n.id}
          node={n}
          index={i}
          scale={scale}
          selected={selectedIds.has(n.id)}
          interactive={interactive}
          onSelect={(id, additive) => {
            select(id, additive)
            // Tapping a box always brings its quick editor back, even after ✕.
            setQuickEditorHiddenFor((prev) => (prev === id ? null : prev))
          }}
          onCommit={(t, scaleFactor) => void updateTransform(n.id, t, scaleFactor)}
          onScalePreview={(factor) =>
            setSpritePreview(factor === null ? null : { id: n.id, factor })
          }
        />
      ))}
      {page && editingNode && (
        <BlockQuickEditor
          // Remount per block: the draft textareas keep the user's in-progress
          // text while focused, and selecting another block never blurs them
          // (block drags preventDefault the focus change) — without the key,
          // block B's editor would inherit block A's draft.
          key={editingNode.id}
          page={page}
          node={editingNode}
          index={nodes.findIndex((n) => n.id === editingNode.id)}
          scale={scale}
          showOriginal={showOriginalUnderEdit}
          onToggleOriginal={() => setShowOriginalUnderEdit((v) => !v)}
          onClose={() => setQuickEditorHiddenFor(editingNode.id)}
        />
      )}
    </div>
  )
}

type TextBlockItemProps = {
  node: TextNodeEntry
  index: number
  scale: number
  selected: boolean
  interactive: boolean
  onSelect: (id: string, additive: boolean) => void
  onCommit: (transform: Transform, scaleFactor?: number) => void
  onScalePreview: (factor: number | null) => void
}

const isAdditiveEvent = (event: unknown): boolean => {
  if (!event || typeof event !== 'object') return false
  const e = event as { shiftKey?: boolean; metaKey?: boolean; ctrlKey?: boolean }
  return !!(e.shiftKey || e.metaKey || e.ctrlKey)
}

const RESIZE_HANDLE_SIZE = 8

type ResizeEdge = { top: boolean; bottom: boolean; left: boolean; right: boolean }

function TextBlockItem({
  node,
  index,
  scale,
  selected,
  interactive,
  onSelect,
  onCommit,
  onScalePreview,
}: TextBlockItemProps) {
  const boxRef = useRef<HTMLDivElement>(null)
  const dragStart = useRef<Box>({ x: 0, y: 0, width: 0, height: 0 })
  const edgeRef = useRef<ResizeEdge | null>(null)
  const isResizeRef = useRef(false)
  const isRotateRef = useRef(false)
  const rotateStart = useRef({ cx: 0, cy: 0, pointerDeg: 0, boxDeg: 0 })

  const t = node.transform
  // Slant: boxes rotate about their centre, matching the baked-in sprite
  // rotation on the render side.
  const deg = t.rotationDeg ?? 0

  const setBox = (x: number, y: number, w: number, h: number) => {
    const el = boxRef.current
    if (!el) return
    el.style.transform = `translate(${x}px, ${y}px) rotate(${deg}deg)`
    el.style.width = `${w}px`
    el.style.height = `${h}px`
  }

  const commitBox = (b: Box, scaleFactor?: number) => {
    onCommit(
      {
        x: Math.round(b.x / scale),
        y: Math.round(b.y / scale),
        width: Math.max(4, Math.round(b.width / scale)),
        height: Math.max(4, Math.round(b.height / scale)),
        rotationDeg: deg,
      },
      scaleFactor,
    )
  }

  const bind = useDrag(
    ({ first, last, movement: [mx, my], xy, event, tap }) => {
      if (!interactive) return
      event?.stopPropagation()
      const additive = isAdditiveEvent(event)
      if (tap) {
        // A tap on a handle armed a mode that never got its drag — disarm.
        isRotateRef.current = false
        isResizeRef.current = false
        edgeRef.current = null
        onSelect(node.id, additive)
        return
      }
      if (isRotateRef.current) {
        // Rotate about the box centre: the angle change follows the pointer's
        // bearing from the centre. Rotation preserves the centre, so the
        // bounding rect's centre IS the box centre, at any zoom.
        const el = boxRef.current
        if (first && el) {
          const rect = el.getBoundingClientRect()
          const cx = rect.left + rect.width / 2
          const cy = rect.top + rect.height / 2
          rotateStart.current = {
            cx,
            cy,
            pointerDeg: (Math.atan2(xy[1] - cy, xy[0] - cx) * 180) / Math.PI,
            boxDeg: deg,
          }
        }
        const rs = rotateStart.current
        const pointerDeg = (Math.atan2(xy[1] - rs.cy, xy[0] - rs.cx) * 180) / Math.PI
        const shift = !!(event as { shiftKey?: boolean } | undefined)?.shiftKey
        const next =
          Math.round(snapRotationDeg(rs.boxDeg + pointerDeg - rs.pointerDeg, shift) * 10) / 10
        if (el)
          el.style.transform = `translate(${t.x * scale}px, ${t.y * scale}px) rotate(${next}deg)`
        if (last) {
          isRotateRef.current = false
          if (next !== deg) {
            onCommit({
              x: t.x,
              y: t.y,
              width: t.width,
              height: t.height,
              rotationDeg: next,
            })
          }
        }
        return
      }
      if (first) {
        dragStart.current = {
          x: t.x * scale,
          y: t.y * scale,
          width: t.width * scale,
          height: t.height * scale,
        }
        // Keep multi-selection intact when dragging a node that's already selected;
        // otherwise this click is a single-select (unless the modifier is held).
        if (additive || !selected) onSelect(node.id, additive)
      }
      const start = dragStart.current
      const edge = edgeRef.current
      const isCorner = !!edge && (edge.left || edge.right) && (edge.top || edge.bottom)
      if (isResizeRef.current && edge && isCorner) {
        // Corner drag scales the whole block Canva-style: uniform factor
        // (aspect locked), opposite corner anchored on screen, text size
        // follows on commit. The dominant drag axis (in the box's local
        // frame, so slanted blocks feel right) drives the factor.
        const minFactor = Math.max((4 * scale) / start.width, (4 * scale) / start.height)
        const factor = cornerScaleFactor(start, edge, mx, my, deg, minFactor)
        const b = scaleRotatedBox(start, edge, factor, deg)
        setBox(b.x, b.y, b.width, b.height)
        onScalePreview(factor)
        if (last) {
          isResizeRef.current = false
          edgeRef.current = null
          onScalePreview(null)
          commitBox(b, factor)
        }
      } else if (isResizeRef.current && edge) {
        const b = resizeRotatedBox(start, edge, mx, my, deg, 4 * scale)
        setBox(b.x, b.y, b.width, b.height)
        if (last) {
          isResizeRef.current = false
          edgeRef.current = null
          commitBox(b)
        }
      } else {
        setBox(start.x + mx, start.y + my, start.width, start.height)
        if (last) {
          onCommit({
            x: Math.round((start.x + mx) / scale),
            y: Math.round((start.y + my) / scale),
            width: t.width,
            height: t.height,
            rotationDeg: deg,
          })
        }
      }
    },
    {
      pointer: { buttons: 1, touch: true },
      filterTaps: true,
      preventDefault: true,
      eventOptions: { passive: false },
    },
  )

  const handleEdgePointerDown = (edge: ResizeEdge) => {
    if (!interactive || !selected) return
    isResizeRef.current = true
    edgeRef.current = edge
  }

  const handleRotatePointerDown = () => {
    if (!interactive || !selected) return
    isRotateRef.current = true
  }

  const w = t.width * scale
  const h = t.height * scale

  return (
    <div
      ref={boxRef}
      {...bind()}
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        transform: `translate(${t.x * scale}px, ${t.y * scale}px) rotate(${deg}deg)`,
        width: w,
        height: h,
        pointerEvents: interactive ? 'auto' : 'none',
        zIndex: selected ? 20 : 10,
        touchAction: 'none',
        cursor: interactive ? 'move' : 'default',
      }}
    >
      <div
        className={`absolute inset-0 rounded-md ${
          selected
            ? 'border-[3px] border-primary bg-primary/15'
            : 'border-2 border-rose-400/60 bg-rose-400/5'
        }`}
      />
      <div
        className={`pointer-events-none absolute -top-1.5 -left-1.5 flex h-4 w-4 items-center justify-center rounded-full text-[9px] font-semibold text-white shadow ${
          selected ? 'bg-primary' : 'bg-rose-400'
        }`}
      >
        {index + 1}
      </div>
      {selected && interactive && <ResizeHandles onEdgePointerDown={handleEdgePointerDown} />}
      {selected && interactive && (
        <>
          {/* Rotator: drag the knob above the box to slant it about its
              centre. Lives inside the rotated frame, so it stays glued to
              the box's top edge at any angle. */}
          <div
            className='pointer-events-none absolute bg-primary/60'
            style={{ top: -14, left: '50%', width: 1, height: 12 }}
          />
          <div
            data-testid='block-rotate-handle'
            onPointerDown={handleRotatePointerDown}
            style={{
              position: 'absolute',
              top: -28,
              left: '50%',
              marginLeft: -9,
              width: 18,
              height: 18,
              cursor: 'grab',
              zIndex: 30,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
          >
            <div className='h-3 w-3 rounded-full border-2 border-primary bg-background shadow-sm' />
          </div>
        </>
      )}
    </div>
  )
}

/**
 * Window onto the untouched source image, clipped to the edited block's box:
 * the original text shows through while the quick editor is open, even when
 * the inpainted or rendered page image is what's displayed underneath.
 */
function OriginalArtPeek({
  page,
  node,
  scale,
  src,
}: {
  page: { width: number; height: number }
  node: TextNodeEntry
  scale: number
  src: string
}) {
  const t = node.transform
  const deg = t.rotationDeg ?? 0
  return (
    <div
      data-testid='original-art-peek'
      className='pointer-events-none absolute overflow-hidden rounded-sm shadow-[0_0_0_1px_rgba(0,0,0,0.25)]'
      style={{
        left: t.x * scale,
        top: t.y * scale,
        width: t.width * scale,
        height: t.height * scale,
        // The window follows the block's slant; the image inside counter-
        // rotates about the same centre so the art itself stays upright.
        transform: `rotate(${deg}deg)`,
      }}
    >
      <img
        alt=''
        src={src}
        draggable={false}
        className='max-w-none select-none'
        style={{
          position: 'absolute',
          left: -t.x * scale,
          top: -t.y * scale,
          width: page.width * scale,
          height: page.height * scale,
          transform: `rotate(${-deg}deg)`,
          transformOrigin: `${(t.x + t.width / 2) * scale}px ${(t.y + t.height / 2) * scale}px`,
        }}
      />
    </div>
  )
}

function BlockSprite({
  node,
  scale,
  previewFactor,
}: {
  node: TextNodeEntry
  scale: number
  previewFactor?: number
}) {
  const sprite = (node.data.sprite as string | null | undefined) ?? undefined
  const { data: src } = useBlobImage(sprite)
  // A block whose translation was cleared (e.g. un-inpainted) may keep a
  // stale sprite blob until the next render finishes — don't show it.
  if (!src || !node.data.translation) return null
  const spriteT = node.data.spriteTransform
  const x = (spriteT?.x ?? node.transform.x) * scale
  const y = (spriteT?.y ?? node.transform.y) * scale
  // While a corner drag scales the block, mirror the factor on the sprite
  // about its centre for live feedback; the crisp re-render lands on commit.
  const f = previewFactor ?? 1
  const pw = (spriteT?.width ?? node.transform.width) * scale
  const ph = (spriteT?.height ?? node.transform.height) * scale
  return (
    <img
      alt=''
      src={src}
      draggable={false}
      className='pointer-events-none absolute select-none'
      style={{
        top: 0,
        left: 0,
        transformOrigin: 'top left',
        transform: `translate(${x + (pw * (1 - f)) / 2}px, ${y + (ph * (1 - f)) / 2}px) scale(${scale * f})`,
      }}
    />
  )
}

function ResizeHandles({ onEdgePointerDown }: { onEdgePointerDown: (edge: ResizeEdge) => void }) {
  const s = RESIZE_HANDLE_SIZE
  const half = s / 2

  const edges: { edge: ResizeEdge; style: React.CSSProperties; cursor: string }[] = [
    {
      edge: { top: true, left: true, bottom: false, right: false },
      cursor: 'nwse-resize',
      style: { top: -half, left: -half, width: s, height: s },
    },
    {
      edge: { top: true, left: false, bottom: false, right: true },
      cursor: 'nesw-resize',
      style: { top: -half, right: -half, width: s, height: s },
    },
    {
      edge: { top: false, left: true, bottom: true, right: false },
      cursor: 'nesw-resize',
      style: { bottom: -half, left: -half, width: s, height: s },
    },
    {
      edge: { top: false, left: false, bottom: true, right: true },
      cursor: 'nwse-resize',
      style: { bottom: -half, right: -half, width: s, height: s },
    },
    {
      edge: { top: true, left: false, bottom: false, right: false },
      cursor: 'ns-resize',
      style: { top: -half, left: s, right: s, height: s },
    },
    {
      edge: { top: false, left: false, bottom: true, right: false },
      cursor: 'ns-resize',
      style: { bottom: -half, left: s, right: s, height: s },
    },
    {
      edge: { top: false, left: true, bottom: false, right: false },
      cursor: 'ew-resize',
      style: { left: -half, top: s, bottom: s, width: s },
    },
    {
      edge: { top: false, left: false, bottom: false, right: true },
      cursor: 'ew-resize',
      style: { right: -half, top: s, bottom: s, width: s },
    },
  ]

  return (
    <>
      {edges.map((e, i) => {
        const isCorner = (e.edge.left || e.edge.right) && (e.edge.top || e.edge.bottom)
        return (
          <div
            key={i}
            onPointerDown={() => onEdgePointerDown(e.edge)}
            style={{
              position: 'absolute',
              ...e.style,
              cursor: e.cursor,
              zIndex: 30,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
          >
            {/* Visible dot so the resize affordance is discoverable: corners
                scale the whole block, edges stretch/reflow. */}
            {isCorner && (
              <div className='h-2 w-2 rounded-sm border border-primary bg-background shadow-sm' />
            )}
          </div>
        )
      })}
    </>
  )
}
