/**
 * The box the editor shows and edits for a text block.
 *
 * A block's stored box marks where the original lettering is: OCR crops from
 * it, and mask rebuilding, cleanup and deletion read it. The renderer can lay
 * a block's translation out in its speech bubble instead of that box, so the
 * text may reach past it. The editor then shows the box around the text as
 * drawn, so moving or resizing starts from the text; the stored box only
 * changes when the user does that.
 */

import type { TextData, Transform } from '@/lib/api/schemas'

/** Pixels of slack for rounding in stored sprite placements. */
const SLACK = 2

export function displayBox(
  box: Transform,
  text: Pick<TextData, 'spriteTransform' | 'translation' | 'lockLayoutBox'>,
): Transform {
  const sprite = text.spriteTransform
  if (!sprite || !text.translation?.trim() || text.lockLayoutBox) return box
  if ((box.rotationDeg ?? 0) !== 0) return box
  const outside =
    sprite.x < box.x - SLACK ||
    sprite.y < box.y - SLACK ||
    sprite.x + sprite.width > box.x + box.width + SLACK ||
    sprite.y + sprite.height > box.y + box.height + SLACK
  // Text fitted into its box is always centred in it; bubble layout isn't.
  const offCentre =
    Math.abs(sprite.x + sprite.width / 2 - (box.x + box.width / 2)) > SLACK ||
    Math.abs(sprite.y + sprite.height / 2 - (box.y + box.height / 2)) > SLACK
  if (!outside && !offCentre) return box
  return { x: sprite.x, y: sprite.y, width: sprite.width, height: sprite.height, rotationDeg: 0 }
}
