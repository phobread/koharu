'use client'

import * as ScrollAreaPrimitive from '@radix-ui/react-scroll-area'
import { useGesture } from '@use-gesture/react'
import { useCallback, useEffect, useMemo, useRef } from 'react'
import type React from 'react'
import { useTranslation } from 'react-i18next'

import { CanvasToolbar } from '@/components/canvas/CanvasToolbar'
import {
  fitCanvasToViewport,
  setCanvasDocumentSize,
  setCanvasViewport,
} from '@/components/canvas/canvasViewport'
import { SubToolRail } from '@/components/canvas/SubToolRail'
import { TextBlockLayer } from '@/components/canvas/TextBlockLayer'
import { ToolRail } from '@/components/canvas/ToolRail'
import {
  resolvePinchMemoScaleRatio,
  resolvePinchNextScaleRatio,
} from '@/components/canvas/zoomGestures'
import { Image } from '@/components/Image'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu'
import { useBlobData } from '@/hooks/useBlobData'
import { useBlockContextMenu } from '@/hooks/useBlockContextMenu'
import { useBlockDrafting, type BlockDraft } from '@/hooks/useBlockDrafting'
import { useBrushCursor } from '@/hooks/useBrushCursor'
import { useBrushLayerDisplay } from '@/hooks/useBrushLayerDisplay'
import { useCanvasZoom } from '@/hooks/useCanvasZoom'
import { findImageBlob, findMaskBlob, isTextNode, useCurrentPage } from '@/hooks/useCurrentPage'
import { useKeyboardShortcuts } from '@/hooks/useKeyboardShortcuts'
import { useMaskDrawing } from '@/hooks/useMaskDrawing'
import { usePointerToDocument } from '@/hooks/usePointerToDocument'
import { useRenderBrushDrawing } from '@/hooks/useRenderBrushDrawing'
import type { Node, Transform } from '@/lib/api/schemas'
import { applyOp } from '@/lib/io/scene'
import { applyBlockMerge, applyBlockSplit } from '@/lib/io/splitNode'
import { uninpaintBlocks } from '@/lib/io/uninpaintBlock'
import { ops } from '@/lib/ops'
import { splitTextBlock } from '@/lib/splitBlock'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

const BRUSH_CURSOR = 'none'

// Wheel-zoom multiplier per notch — multiplicative, so spanning the whole
// 10–100% range takes ~25 notches instead of 90 one-percent steps.
const ZOOM_WHEEL_FACTOR = 1.1

/**
 * Primary canvas viewport.
 *
 * Reads the active page from the scene mirror; derives layer blob hashes from
 * role-keyed nodes (`Image { source | inpainted | rendered | custom }`,
 * `Mask { segment | brushInpaint }`). Mutations (text-block add/delete,
 * mask edits, brush strokes) dispatch through `applyCommand` or the V2 mask
 * PUT endpoint — no V1 shim layer.
 */
export function Workspace() {
  useKeyboardShortcuts()

  const scale = useEditorUiStore((s) => s.scale)
  const showSegmentationMask = useEditorUiStore((s) => s.showSegmentationMask)
  const showInpaintedImage = useEditorUiStore((s) => s.showInpaintedImage)
  const showBrushLayer = useEditorUiStore((s) => s.showBrushLayer)
  const showRenderedImage = useEditorUiStore((s) => s.showRenderedImage)
  const showTextBlocksOverlay = useEditorUiStore((s) => s.showTextBlocksOverlay)
  const mode = useEditorUiStore((s) => s.mode)
  const autoFitEnabled = useEditorUiStore((s) => s.autoFitEnabled)

  const page = useCurrentPage()
  const clearSelection = useSelectionStore((s) => s.clear)

  // Derive role-keyed blob hashes off the active page.
  const imageHash = useMemo(() => (page ? findImageBlob(page, 'source') : null), [page])
  const segmentHash = useMemo(() => (page ? findMaskBlob(page, 'segment') : null), [page])
  const inpaintedHash = useMemo(() => (page ? findImageBlob(page, 'inpainted') : null), [page])
  const brushLayerHash = useMemo(() => (page ? findMaskBlob(page, 'brushInpaint') : null), [page])
  const renderedHash = useMemo(() => (page ? findImageBlob(page, 'rendered') : null), [page])

  const imageData = useBlobData(imageHash ?? undefined)
  const segmentData = useBlobData(segmentHash ?? undefined)
  const inpaintedData = useBlobData(inpaintedHash ?? undefined)
  const brushLayerData = useBlobData(brushLayerHash ?? undefined)
  const renderedData = useBlobData(renderedHash ?? undefined)

  useEffect(() => {
    if (page) setCanvasDocumentSize(page.width, page.height)
  }, [page?.width, page?.height])

  const viewportRef = useRef<HTMLDivElement | null>(null)
  const canvasRef = useRef<HTMLDivElement | null>(null)
  const { setScale: applyScale } = useCanvasZoom()
  const scaleRatio = scale / 100

  const handleViewportRef = useCallback((el: HTMLDivElement | null) => {
    viewportRef.current = el
    setCanvasViewport(el)
  }, [])

  const pointerToDocument = usePointerToDocument(scaleRatio, canvasRef)

  const createTextNode = useCallback(
    async (draft: BlockDraft) => {
      if (!page) return
      const at = Object.keys(page.nodes).length
      const nodeId = crypto.randomUUID()
      const transform: Transform = {
        x: draft.x,
        y: draft.y,
        width: draft.width,
        height: draft.height,
        rotationDeg: 0,
      }
      const node: Node = {
        id: nodeId,
        transform,
        visible: true,
        kind: { text: { lockLayoutBox: true } },
      }
      await applyOp(ops.addNode(page.id, at, node))
      useSelectionStore.getState().selectMany([nodeId])
    },
    [page],
  )

  const removeTextNode = useCallback(
    async (nodeId: string) => {
      if (!page) return
      const node = page.nodes[nodeId]
      if (!node) return
      const idx = Object.keys(page.nodes).indexOf(nodeId)
      await applyOp(ops.removeNode(page.id, nodeId, node, idx < 0 ? 0 : idx))
    },
    [page],
  )

  // Split a text block into two halves along its longer side, dividing the
  // text between them (see `applyBlockSplit` for how the halves land in the
  // scene).
  const splitTextNode = useCallback(
    async (nodeId: string) => {
      if (!page) return
      const node = page.nodes[nodeId]
      if (!node || !isTextNode(node) || !node.transform) return
      const data = node.kind.text
      const split = splitTextBlock(node.transform, {
        text: data.text,
        translation: data.translation,
      })
      await applyBlockSplit(page, nodeId, split)
    },
    [page],
  )

  // Merge the selected blocks back into one — the inverse of a split (halves
  // tile the original box, so the union reconstructs it exactly).
  const selectedCount = useSelectionStore((s) => s.nodeIds.size)
  const mergeSelectedBlocks = useCallback(async () => {
    if (!page) return
    const ids = Array.from(useSelectionStore.getState().nodeIds).filter((id): id is string => !!id)
    await applyBlockMerge(page, ids)
  }, [page])

  // Clear the segment mask under the selected blocks and re-inpaint, so the
  // original art shows again (for falsely detected blocks or text worth
  // keeping as drawn, like "...").
  const uninpaintSelectedBlocks = useCallback(async () => {
    if (!page || !segmentData) return
    const ids = Array.from(useSelectionStore.getState().nodeIds).filter((id): id is string => !!id)
    try {
      await uninpaintBlocks(page, ids, segmentData)
    } catch (e) {
      useEditorUiStore.getState().showError(String(e))
    }
  }, [page, segmentData])

  const { draftBlock, bind: bindBlockDraft } = useBlockDrafting({
    mode,
    page,
    pointerToDocument,
    clearSelection,
    onCreateBlock: (draft) => {
      void createTextNode(draft)
    },
  })

  const { brushCursorRef, isBrushMode, brushSize } = useBrushCursor(canvasRef, mode, page?.id)

  const maskPointerEnabled = useMemo(
    () =>
      mode === 'repairBrush' || (mode === 'eraser' && (showSegmentationMask || !showBrushLayer)),
    [mode, showSegmentationMask, showBrushLayer],
  )
  const brushPointerEnabled = useMemo(
    () => mode === 'brush' || (mode === 'eraser' && !showSegmentationMask && showBrushLayer),
    [mode, showSegmentationMask, showBrushLayer],
  )

  const maskDrawing = useMaskDrawing({
    mode,
    page,
    segmentData,
    pointerToDocument,
    showMask: showSegmentationMask,
    enabled: maskPointerEnabled,
  })
  const brushLayerDisplay = useBrushLayerDisplay({
    page,
    brushLayerData,
    visible: showBrushLayer,
  })
  const brushDrawing = useRenderBrushDrawing({
    mode,
    page,
    pointerToDocument,
    enabled: brushPointerEnabled,
    action: mode === 'eraser' ? 'erase' : 'paint',
    targetCanvasRef: brushLayerDisplay.canvasRef,
  })
  const blockDraftBindings = bindBlockDraft()
  const maskBindings = maskDrawing.bind()
  const brushBindings = brushDrawing.bind()

  useEffect(() => {
    if (page && autoFitEnabled) fitCanvasToViewport()
  }, [page?.id, autoFitEnabled])

  const {
    contextMenuNodeId,
    handleContextMenu,
    handleDeleteBlock,
    handleSplitBlock,
    clearContextMenu,
  } = useBlockContextMenu({
    page,
    pointerToDocument,
    onSelect: (nodeId) => {
      if (!nodeId) {
        useSelectionStore.getState().clear()
        return
      }
      // Keep a multi-selection intact when right-clicking inside it (e.g. to
      // merge the selected blocks); otherwise select the hit node alone.
      if (!useSelectionStore.getState().nodeIds.has(nodeId)) {
        useSelectionStore.getState().selectMany([nodeId])
      }
    },
    onRemove: (nodeId) => {
      void removeTextNode(nodeId)
    },
    onSplit: (nodeId) => {
      void splitTextNode(nodeId)
    },
  })
  const { t } = useTranslation()

  useGesture(
    {
      onDrag: ({ first, movement: [mx, my], memo, cancel, ctrlKey, event }) => {
        if (!page) return memo
        const viewport = viewportRef.current
        if (first) {
          // Pan with ctrl+drag or a middle-button drag; a plain left drag
          // belongs to selection/drafting, so hand the gesture back untouched.
          const middle = 'buttons' in event && ((event.buttons as number) & 4) !== 0
          if (!ctrlKey && !middle) {
            if (cancel) cancel()
            return memo
          }
          if (!viewport) return memo
          return { scrollLeft: viewport.scrollLeft, scrollTop: viewport.scrollTop }
        }
        if (!memo || !viewport) return memo
        viewport.scrollLeft = memo.scrollLeft - mx
        viewport.scrollTop = memo.scrollTop - my
        return memo
      },
      onWheel: ({ ctrlKey, shiftKey, delta: [dx, dy], event }) => {
        if (!page) return
        const viewport = viewportRef.current
        if (ctrlKey) {
          if (event.cancelable) event.preventDefault()
          const direction = Math.sign(dy)
          if (!direction) return
          const oldScale = useEditorUiStore.getState().scale
          const canvas = canvasRef.current
          const before = canvas?.getBoundingClientRect()
          applyScale(direction > 0 ? oldScale / ZOOM_WHEEL_FACTOR : oldScale * ZOOM_WHEEL_FACTOR)
          // Keep the document point under the cursor fixed: once the resized
          // canvas has laid out, shift the scroll offsets by however far that
          // point moved. Without overflow the browser clamps the scroll back
          // to 0 and the centred layout takes over, which is what we want.
          if (!canvas || !before || !viewport) return
          const { clientX, clientY } = event
          const docX = (clientX - before.left) / (oldScale / 100)
          const docY = (clientY - before.top) / (oldScale / 100)
          requestAnimationFrame(() => {
            const applied = useEditorUiStore.getState().scale / 100
            const after = canvas.getBoundingClientRect()
            viewport.scrollLeft += after.left + docX * applied - clientX
            viewport.scrollTop += after.top + docY * applied - clientY
          })
          return
        }
        // Shift+wheel pans horizontally. Some browsers swap the axes
        // themselves (the delta arrives on X), others don't — take whichever
        // axis actually moved.
        if (shiftKey && viewport) {
          if (event.cancelable) event.preventDefault()
          viewport.scrollLeft += dx !== 0 ? dx : dy
        }
      },
      onPinch: ({ canceled, movement: [movementScale], memo }) => {
        if (!page || canceled) return memo
        const memoScaleRatio = resolvePinchMemoScaleRatio(
          memo,
          useEditorUiStore.getState().scale / 100,
        )
        const nextScaleRatio = resolvePinchNextScaleRatio(memoScaleRatio, movementScale)
        applyScale(nextScaleRatio * 100)
        return memoScaleRatio
      },
    },
    {
      target: viewportRef,
      eventOptions: { passive: false },
      drag: { filterTaps: true, pointer: { mouse: true, buttons: [1, 4] } },
      wheel: { preventDefault: false },
      pinch: {
        threshold: 0.1,
        enabled: true,
        pinchOnWheel: false,
        preventDefault: true,
        scaleBounds: { min: 0.1, max: 1 },
        from: () => [useEditorUiStore.getState().scale / 100, 0],
      },
    },
  )

  const handleCanvasPointerDownCapture = (event: React.PointerEvent<HTMLDivElement>) => {
    // Clicking the artwork deselects. Anything interactive for blocks (the
    // boxes, their handles, the quick editor) lives inside the
    // `data-text-block-layer` subtree, so a pointerdown outside it is the
    // picture itself. Other modes keep their own semantics: block mode
    // clears via drafting, brush strokes shouldn't drop the selection.
    if (mode !== 'select') return
    const target = event.target instanceof Element ? event.target : null
    if (!target?.closest('[data-text-block-layer]')) {
      clearSelection()
    }
  }
  const handleCanvasContextMenu = (event: React.MouseEvent<HTMLDivElement>) => {
    handleContextMenu(event)
  }

  const canvasCursor = useMemo(
    () => (isBrushMode ? BRUSH_CURSOR : mode === 'block' ? 'cell' : 'default'),
    [isBrushMode, mode],
  )

  const canvasDimensions = useMemo(
    () =>
      page
        ? { width: page.width * scaleRatio, height: page.height * scaleRatio }
        : { width: 0, height: 0 },
    [page?.width, page?.height, scaleRatio],
  )

  return (
    <div className='relative flex min-h-0 min-w-0 flex-1 bg-muted'>
      <ToolRail />
      <SubToolRail />
      <div className='relative flex min-h-0 min-w-0 flex-1 flex-col'>
        <CanvasToolbar />
        <ScrollAreaPrimitive.Root className='flex min-h-0 min-w-0 flex-1'>
          <ScrollAreaPrimitive.Viewport
            ref={handleViewportRef}
            data-testid='workspace-viewport'
            className='grid size-full place-content-center-safe'
            onMouseDown={(e) => {
              // The middle button starts our pan drag — suppress the
              // browser's autoscroll marker.
              if (e.button === 1) e.preventDefault()
            }}
          >
            {page ? (
              <ContextMenu
                onOpenChange={(open) => {
                  if (!open) clearContextMenu()
                }}
              >
                <ContextMenuTrigger asChild>
                  <div className='grid place-items-center'>
                    <div
                      ref={canvasRef}
                      data-testid='workspace-canvas'
                      className='relative rounded-md border border-border bg-card shadow-sm'
                      style={{
                        ...canvasDimensions,
                        cursor: canvasCursor,
                        touchAction: 'none',
                      }}
                      onPointerDownCapture={handleCanvasPointerDownCapture}
                      onContextMenuCapture={handleCanvasContextMenu}
                      {...blockDraftBindings}
                    >
                      <div
                        ref={brushCursorRef}
                        className='pointer-events-none absolute z-50 rounded-full border border-white shadow-[0_0_0_1px_rgba(0,0,0,0.5),0_1px_3px_rgba(0,0,0,0.3)] transition-opacity duration-75'
                        style={{
                          opacity: 0,
                          width: brushSize * scaleRatio,
                          height: brushSize * scaleRatio,
                        }}
                      />
                      <div className='absolute inset-0'>
                        <Image
                          data={imageData}
                          dataKey={imageHash ?? undefined}
                          transition={false}
                        />
                        <canvas
                          ref={maskDrawing.canvasRef}
                          data-testid='workspace-mask-canvas'
                          className='absolute inset-0 z-20'
                          style={{
                            width: '100%',
                            height: '100%',
                            opacity: showSegmentationMask ? 0.8 : 0,
                            pointerEvents: maskPointerEnabled ? 'auto' : 'none',
                            touchAction: 'none',
                            transition: 'opacity 120ms ease',
                          }}
                          {...maskBindings}
                        />
                        {inpaintedData && (
                          <Image
                            data-testid='workspace-inpainted-image'
                            data={inpaintedData}
                            visible={showInpaintedImage}
                            transition={true}
                          />
                        )}
                        <canvas
                          ref={brushLayerDisplay.canvasRef}
                          data-testid='workspace-brush-display-canvas'
                          className='absolute inset-0'
                          style={{
                            width: '100%',
                            height: '100%',
                            opacity: brushLayerDisplay.visible ? 1 : 0,
                            pointerEvents: 'none',
                            zIndex: 10,
                            transition: 'opacity 120ms ease',
                          }}
                        />
                        <canvas
                          ref={brushDrawing.canvasRef}
                          data-testid='workspace-brush-canvas'
                          className='absolute inset-0'
                          style={{
                            width: '100%',
                            height: '100%',
                            opacity: brushDrawing.visible ? 1 : 0,
                            pointerEvents: brushPointerEnabled ? 'auto' : 'none',
                            touchAction: 'none',
                            zIndex: 20,
                            transition: 'opacity 120ms ease',
                          }}
                          {...brushBindings}
                        />
                        {renderedData && showRenderedImage && (
                          <Image
                            data-testid='workspace-rendered-image'
                            data={renderedData}
                            transition={true}
                            style={{ zIndex: 40 }}
                          />
                        )}
                        {/* Above the rendered image: boxes, the quick editor
                            and the original-art peek stay visible and usable
                            in the Translated view too. */}
                        {showTextBlocksOverlay && (
                          <TextBlockLayer
                            showSprites={!showRenderedImage}
                            scale={scaleRatio}
                            style={{ zIndex: 50 }}
                          />
                        )}
                      </div>
                      {draftBlock && (
                        <div
                          className='pointer-events-none absolute rounded-md border-2 border-dashed border-primary bg-primary/10'
                          style={{
                            left: draftBlock.x * scaleRatio,
                            top: draftBlock.y * scaleRatio,
                            width: Math.max(0, draftBlock.width * scaleRatio),
                            height: Math.max(0, draftBlock.height * scaleRatio),
                          }}
                        />
                      )}
                    </div>
                  </div>
                </ContextMenuTrigger>
                <ContextMenuContent className='min-w-32'>
                  <ContextMenuItem
                    disabled={contextMenuNodeId === null}
                    onSelect={handleSplitBlock}
                  >
                    {t('workspace.splitBlock')}
                  </ContextMenuItem>
                  <ContextMenuItem
                    disabled={selectedCount < 2}
                    onSelect={() => void mergeSelectedBlocks()}
                  >
                    {t('workspace.mergeBlocks')}
                  </ContextMenuItem>
                  <ContextMenuItem
                    disabled={contextMenuNodeId === null || !segmentData}
                    onSelect={() => void uninpaintSelectedBlocks()}
                  >
                    {t('workspace.uninpaintBlock')}
                  </ContextMenuItem>
                  <ContextMenuItem
                    disabled={contextMenuNodeId === null}
                    onSelect={handleDeleteBlock}
                  >
                    {t('workspace.deleteBlock')}
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            ) : (
              <div className='flex h-full w-full items-center justify-center text-sm text-muted-foreground'>
                {t('workspace.importPrompt')}
              </div>
            )}
          </ScrollAreaPrimitive.Viewport>
          <ScrollAreaPrimitive.Scrollbar
            orientation='vertical'
            className='flex w-2 touch-none p-px select-none'
          >
            <ScrollAreaPrimitive.Thumb className='flex-1 rounded bg-muted-foreground/40' />
          </ScrollAreaPrimitive.Scrollbar>
          <ScrollAreaPrimitive.Scrollbar
            orientation='horizontal'
            className='flex h-2 touch-none p-px select-none'
          >
            <ScrollAreaPrimitive.Thumb className='rounded bg-muted-foreground/40' />
          </ScrollAreaPrimitive.Scrollbar>
        </ScrollAreaPrimitive.Root>
      </div>
    </div>
  )
}
