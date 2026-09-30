//! Official release pages (`Image { Official }`).
//!
//! Some chapters also exist as an official English release: the same pages
//! with the same artwork, re-lettered — dialogue and onomatopoeia alike. The
//! owner keeps their own dialogue translation but wants the release's
//! onomatopoeia. So wherever the owner has no text, the cleaned page shows
//! the release: every place where the release differs from the raw page is
//! either the owner's dialogue (the part of the bubble their box sits in,
//! the box itself, what cleanup erased near a box) or a *piece* copied from
//! the release.
//!
//! Inside bubbles the decision is made per bubble part, not by distance: the
//! bubble segmenter often merges touching bubbles under one id, so a text box
//! claims only its own lobe of a merged bubble (the renderer's
//! [`BubbleIndex::balloon_shapes`]), and a moan or "..." bubble touching it
//! still comes over — also when the owner brushed its Korean out by hand.
//! Outside bubbles, changed pixels are grouped into words, lines and
//! effects; a group touching a box over artwork, or what cleanup erased near
//! a box, is the owner's; any other group is a piece.
//!
//! Every cleanup path starts from the same base — `Source` with the pieces
//! pasted ([`cleanup_base`]) — and pastes the pieces again over its result,
//! so a re-clean, a repair stroke or a deleted box never brings the raw
//! lettering back where the release has its own.

use std::collections::HashMap;

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, GrayImage, Luma, RgbaImage};
use imageproc::distance_transform::{Norm, distance_transform};
use imageproc::morphology::{dilate, open};
use imageproc::region_labelling::{Connectivity, connected_components};
use koharu_core::{
    BlobRef, ImageData, ImageDataPatch, ImageRole, MaskRole, Node, NodeDataPatch, NodeId, NodeKind,
    NodePatch, Op, Page, PageId, Scene, Transform,
};
use koharu_renderer::text::latin::{BalloonShape, BubbleIndex, LayoutBox};
use rayon::prelude::*;

use crate::blobs::BlobStore;

/// A pixel counts as changed when some channel differs by more than this.
/// The release and the raw share their artwork up to JPEG noise (mean
/// difference 4-8 levels on BadEnd, no colour shift).
const CHANGED_THRESHOLD: u8 = 60;
/// Changed specks smaller than this are compression ringing.
const MIN_PIECE_PX: usize = 30;
/// Outside bubbles, changed pixels this close together form one group (a
/// word, a line, a sound effect).
const GROUP_RADIUS: u8 = 20;
/// Inside a bubble, the tighter grouping of letters into words.
const BUBBLE_GROUP_RADIUS: u8 = 8;
/// A group this close to a box over artwork is that box's text.
const BOX_MARGIN: u32 = 8;
/// ...or this close to an erased area touching a box (the original place
/// of lettering whose box the owner moved a little).
const ERASE_MARGIN: u8 = 4;
/// Erased pixels this close together form one area (a text block).
const ERASE_MERGE: u8 = 12;
/// Outside bubbles, a stroke whose bounding box is at least this share of
/// the page width on both sides is a sound effect's, not a letter's
/// (letters are ~1.5-2 % of a 3000 px page).
const BIG_STROKE_FRAC: f32 = 0.03;
const BIG_STROKE_MIN: u32 = 24;
/// Lettering this close to a box's own bubble part still belongs to it (the
/// part is the lobe's whole interior, so this only covers anti-aliasing; a
/// wider ring swallows moan bubbles overlapping the lobe).
const LOBE_RING: u8 = 6;
/// Outside bubbles, a group lying mostly within this distance of the
/// owner's bubble parts is dialogue spilling past the bubble mask.
const SPILL_ZONE: u8 = 12;
/// A box straddling bubbles claims each bubble covering this share of it.
const STRADDLE_SHARE: f32 = 0.15;
/// Pieces are pasted this much wider (to include their anti-aliased edges),
/// but never within this distance of the owner's dialogue.
const PASTE_GROW: u8 = 2;

/// Where a page takes the official release's pixels, with the release.
pub struct Pieces {
    official: RgbaImage,
    mask: GrayImage,
    pixels: u64,
}

impl Pieces {
    pub fn is_empty(&self) -> bool {
        self.pixels == 0
    }

    /// Number of pixels copied from the release.
    pub fn pixels(&self) -> u64 {
        self.pixels
    }

    pub fn mask(&self) -> &GrayImage {
        &self.mask
    }

    /// `image` with the pieces copied in from the release, in its own
    /// colour type (engines store RGB layers).
    pub fn apply(&self, image: &DynamicImage) -> DynamicImage {
        if image.dimensions() != self.mask.dimensions() || self.is_empty() {
            return image.clone();
        }
        let mut out = image.to_rgba8();
        self.apply_to(&mut out, None);
        if image.color().has_alpha() {
            DynamicImage::ImageRgba8(out)
        } else {
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(out).to_rgb8())
        }
    }

    /// Copy the pieces into `out`, except where `keep` (a painted RGBA
    /// overlay such as the brush layer) has paint. Returns whether any pixel
    /// changed.
    pub fn apply_to(&self, out: &mut RgbaImage, keep: Option<&RgbaImage>) -> bool {
        if out.dimensions() != self.mask.dimensions() {
            return false;
        }
        let mut changed = false;
        for (x, y, m) in self.mask.enumerate_pixels() {
            if m.0[0] == 0 {
                continue;
            }
            if keep.is_some_and(|k| k.get_pixel(x, y).0[3] > 0) {
                continue;
            }
            let pixel = *self.official.get_pixel(x, y);
            if *out.get_pixel(x, y) != pixel {
                out.put_pixel(x, y, pixel);
                changed = true;
            }
        }
        changed
    }
}

/// The picture cleanup works from: `Source`, with the release's pieces when
/// the page has an official image. Also returns the pieces, for pasting
/// over the cleanup result.
pub fn cleanup_base(
    scene: &Scene,
    page: PageId,
    blobs: &BlobStore,
) -> Result<(DynamicImage, Option<Pieces>)> {
    let page = scene
        .page(page)
        .with_context(|| format!("page {page} not found"))?;
    let source =
        load_role(page, ImageRole::Source, blobs)?.context("page has no Source image node")?;
    let pieces = page_pieces_with_source(page, blobs, &source, None)?;
    let base = match &pieces {
        Some(pieces) => pieces.apply(&source),
        None => source,
    };
    Ok((base, pieces))
}

/// The pieces of `page`'s official image, if it has one that matches the
/// source. `erase` overrides the page's segment mask (a deletion computes
/// the pieces for the mask it is about to store).
pub fn page_pieces(
    page: &Page,
    blobs: &BlobStore,
    erase: Option<&GrayImage>,
) -> Result<Option<Pieces>> {
    if page.official_node().is_none() {
        return Ok(None);
    }
    let Some(source) = load_role(page, ImageRole::Source, blobs)? else {
        return Ok(None);
    };
    page_pieces_with_source(page, blobs, &source, erase)
}

fn page_pieces_with_source(
    page: &Page,
    blobs: &BlobStore,
    source: &DynamicImage,
    erase: Option<&GrayImage>,
) -> Result<Option<Pieces>> {
    let Some(official) = load_role(page, ImageRole::Official, blobs)? else {
        return Ok(None);
    };
    pieces_from(page, blobs, source, &official, erase)
}

/// The pieces `official` gives `page` (whose own official image, if any, is
/// ignored).
fn pieces_from(
    page: &Page,
    blobs: &BlobStore,
    source: &DynamicImage,
    official: &DynamicImage,
    erase: Option<&GrayImage>,
) -> Result<Option<Pieces>> {
    if official.dimensions() != source.dimensions() {
        tracing::warn!(
            page = %page.id,
            official = ?official.dimensions(),
            source = ?source.dimensions(),
            "official image and source differ in size; ignoring the official image"
        );
        return Ok(None);
    }
    let erase_mask = match erase {
        Some(mask) => Some(mask.clone()),
        None => load_mask(page, MaskRole::Segment, blobs)?.map(|m| m.to_luma8()),
    };
    let bubbles = load_mask(page, MaskRole::Bubble, blobs)?.map(|m| m.to_luma8());
    let boxes = page
        .nodes
        .values()
        .filter(|node| matches!(node.kind, NodeKind::Text(_)))
        .map(|node| node.transform)
        .collect::<Vec<_>>();
    let raw = source.to_rgba8();
    let official = official.to_rgba8();
    let mask = piece_mask(
        &raw,
        &official,
        &boxes,
        erase_mask.as_ref(),
        bubbles.as_ref(),
    );
    let pixels = mask.pixels().filter(|p| p.0[0] != 0).count() as u64;
    Ok(Some(Pieces {
        official,
        mask,
        pixels,
    }))
}

fn load_role(page: &Page, role: ImageRole, blobs: &BlobStore) -> Result<Option<DynamicImage>> {
    let Some(blob) = page.nodes.values().find_map(|node| match &node.kind {
        NodeKind::Image(img) if img.role == role => Some(img.blob.clone()),
        _ => None,
    }) else {
        return Ok(None);
    };
    blobs
        .load_image(&blob)
        .with_context(|| format!("load {role:?} image blob {}", blob.hash()))
        .map(Some)
}

fn load_mask(page: &Page, role: MaskRole, blobs: &BlobStore) -> Result<Option<DynamicImage>> {
    let Some(blob) = page.nodes.values().find_map(|node| match &node.kind {
        NodeKind::Mask(mask) if mask.role == role => Some(mask.blob.clone()),
        _ => None,
    }) else {
        return Ok(None);
    };
    blobs
        .load_image(&blob)
        .with_context(|| format!("load {role:?} mask blob {}", blob.hash()))
        .map(Some)
}

// ---------------------------------------------------------------------------
// Adding a release to a project
// ---------------------------------------------------------------------------

/// A release page the owner picked: its file name and bytes as read.
pub struct ReleaseFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// What adding release pages to a project does. `ops` is one batch.
#[derive(Default)]
pub struct ReleasePlan {
    /// Official images added or replaced, and cleaned layers that took the
    /// release's pieces (with their now stale rendered images dropped).
    pub ops: Vec<Op>,
    /// Pages that got a release page, with its file name.
    pub matched: Vec<(PageId, String)>,
    /// Pages no file clearly matched.
    pub unmatched_pages: Vec<PageId>,
    /// Files that matched no page (or could not be read as images).
    pub unused_files: Vec<String>,
    /// Pages whose rendered image was dropped; they need rendering again.
    pub rerender: Vec<PageId>,
}

/// Cleaned layers are patched this many pages at a time (each holds a few
/// page-sized buffers).
const PLAN_THREADS: usize = 3;

/// Pair `files` with the scene's pages by picture and plan adding them as
/// official images. Pages already cleaned get the release's pieces at once,
/// so finished pages need no new cleanup.
pub fn plan_release(
    scene: &Scene,
    blobs: &BlobStore,
    files: &[ReleaseFile],
) -> Result<ReleasePlan> {
    // Thumbnails only: a chapter of decoded 12 MP pages would not fit.
    let file_thumbs = files
        .par_iter()
        .map(|file| {
            image::load_from_memory(&file.bytes)
                .ok()
                .map(|img| PairingThumb::new(&img))
        })
        .collect::<Vec<_>>();
    let pages = scene.pages.values().collect::<Vec<_>>();
    let page_thumbs = pages
        .par_iter()
        .map(|page| {
            load_role(page, ImageRole::Source, blobs)
                .ok()
                .flatten()
                .map(|img| PairingThumb::new(&img))
        })
        .collect::<Vec<_>>();
    let (file_index, file_thumbs): (Vec<usize>, Vec<PairingThumb>) = file_thumbs
        .into_iter()
        .enumerate()
        .filter_map(|(i, t)| t.map(|t| (i, t)))
        .unzip();
    let (page_index, page_thumbs): (Vec<usize>, Vec<PairingThumb>) = page_thumbs
        .into_iter()
        .enumerate()
        .filter_map(|(i, t)| t.map(|t| (i, t)))
        .unzip();
    let mut partner = vec![None; pages.len()];
    for (k, candidate) in pair_by_picture(&page_thumbs, &file_thumbs)
        .into_iter()
        .enumerate()
    {
        if let Some(c) = candidate {
            partner[page_index[k]] = Some(file_index[c]);
        }
    }

    let mut plan = ReleasePlan::default();
    let mut used = vec![false; files.len()];
    let mut jobs = Vec::new();
    for (page, file) in pages.iter().zip(&partner) {
        match file {
            Some(f) => {
                used[*f] = true;
                jobs.push((*page, &files[*f]));
            }
            None => plan.unmatched_pages.push(page.id),
        }
    }
    plan.unused_files = files
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|(file, _)| file.name.clone())
        .collect();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(PLAN_THREADS)
        .build()
        .context("start the official-page workers")?;
    let planned = pool.install(|| {
        jobs.par_iter()
            .map(|(page, file)| plan_page(page, blobs, file))
            .collect::<Vec<_>>()
    });
    for ((page, file), result) in jobs.iter().zip(planned) {
        let page_plan =
            result.with_context(|| format!("add {} to page {}", file.name, page.name))?;
        plan.ops.extend(page_plan.ops);
        if page_plan.rerender {
            plan.rerender.push(page.id);
        }
        plan.matched.push((page.id, file.name.clone()));
    }
    Ok(plan)
}

struct PagePlan {
    ops: Vec<Op>,
    rerender: bool,
}

fn plan_page(page: &Page, blobs: &BlobStore, file: &ReleaseFile) -> Result<PagePlan> {
    let official =
        image::load_from_memory(&file.bytes).with_context(|| format!("decode {}", file.name))?;
    let (width, height) = official.dimensions();
    let blob = blobs
        .put_bytes(&file.bytes)
        .context("store the official page")?;
    let mut ops = Vec::new();
    let mut rerender = false;

    // A page already cleaned takes the pieces now. Its rendered image still
    // shows the raw lettering there: drop it, the renderer makes it again.
    let inpainted = page.nodes.iter().find_map(|(id, node)| match &node.kind {
        NodeKind::Image(img) if img.role == ImageRole::Inpainted => Some((*id, img.blob.clone())),
        _ => None,
    });
    if let Some((inpainted_id, inpainted_blob)) = inpainted
        && let Some(source) = load_role(page, ImageRole::Source, blobs)?
        && let Some(pieces) = pieces_from(page, blobs, &source, &official, None)?
        && !pieces.is_empty()
    {
        let cleaned = blobs
            .load_image(&inpainted_blob)
            .context("load the cleaned page")?;
        let brush = load_mask(page, MaskRole::BrushInpaint, blobs)?
            .map(|m| m.to_rgba8())
            .filter(|m| m.dimensions() == cleaned.dimensions());
        let mut out = cleaned.to_rgba8();
        if pieces.apply_to(&mut out, brush.as_ref()) {
            let out = if cleaned.color().has_alpha() {
                DynamicImage::ImageRgba8(out)
            } else {
                DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(out).to_rgb8())
            };
            let (w, h) = out.dimensions();
            let new_blob = blobs.put_webp(&out).context("store the cleaned page")?;
            if let Some((index, (id, node))) =
                page.nodes.iter().enumerate().find(|(_, (_, node))| {
                    matches!(&node.kind, NodeKind::Image(img) if img.role == ImageRole::Rendered)
                })
            {
                ops.push(Op::RemoveNode {
                    page: page.id,
                    id: *id,
                    prev_node: node.clone(),
                    prev_index: index,
                });
                rerender = true;
            }
            ops.push(image_blob_op(page.id, inpainted_id, new_blob, w, h, None));
        }
    }

    match page.official_node() {
        Some((id, _)) => ops.push(image_blob_op(
            page.id,
            *id,
            blob,
            width,
            height,
            Some(file.name.clone()),
        )),
        None => {
            let at = page
                .nodes
                .values()
                .position(|node| {
                    matches!(&node.kind, NodeKind::Image(img) if img.role == ImageRole::Source)
                })
                .map_or(0, |i| i + 1);
            // The rendered image dropped above sits after the source, so
            // this slot is still in range.
            ops.push(Op::AddNode {
                page: page.id,
                node: Node {
                    id: NodeId::new(),
                    transform: Transform::default(),
                    visible: false,
                    kind: NodeKind::Image(ImageData {
                        role: ImageRole::Official,
                        blob,
                        opacity: 1.0,
                        natural_width: width,
                        natural_height: height,
                        name: Some(file.name.clone()),
                    }),
                },
                at,
            });
        }
    }
    Ok(PagePlan { ops, rerender })
}

fn image_blob_op(
    page: PageId,
    id: NodeId,
    blob: BlobRef,
    width: u32,
    height: u32,
    name: Option<String>,
) -> Op {
    Op::UpdateNode {
        page,
        id,
        patch: NodePatch {
            data: Some(NodeDataPatch::Image(ImageDataPatch {
                blob: Some(blob),
                natural_width: Some(width),
                natural_height: Some(height),
                name: name.map(Some),
                ..Default::default()
            })),
            ..Default::default()
        },
        prev: NodePatch::default(),
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl Rect {
    /// Axis-aligned page bounds of a (possibly rotated) box, clipped.
    fn of(t: &Transform, w: u32, h: u32) -> Self {
        let (cx, cy) = (t.x + t.width / 2.0, t.y + t.height / 2.0);
        let (sin, cos) = t.rotation_deg.to_radians().sin_cos();
        let (hw, hh) = (t.width / 2.0, t.height / 2.0);
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for (dx, dy) in [(-hw, -hh), (hw, -hh), (-hw, hh), (hw, hh)] {
            let x = cx + dx * cos - dy * sin;
            let y = cy + dx * sin + dy * cos;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        let clip = |v: f32, hi: u32| (v.max(0.0) as u32).min(hi);
        Self {
            x0: clip(min_x.floor(), w),
            y0: clip(min_y.floor(), h),
            x1: clip(max_x.ceil(), w),
            y1: clip(max_y.ceil(), h),
        }
    }

    fn grown(self, by: u32, w: u32, h: u32) -> Self {
        Self {
            x0: self.x0.saturating_sub(by),
            y0: self.y0.saturating_sub(by),
            x1: (self.x1 + by).min(w),
            y1: (self.y1 + by).min(h),
        }
    }

    fn area(self) -> u32 {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }

    fn layout_box(self) -> LayoutBox {
        LayoutBox {
            x: self.x0 as f32,
            y: self.y0 as f32,
            width: (self.x1 - self.x0) as f32,
            height: (self.y1 - self.y0) as f32,
        }
    }

    fn paint(self, mask: &mut GrayImage) {
        for y in self.y0..self.y1 {
            for x in self.x0..self.x1 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
    }
}

/// Where the page takes the release's pixels (255) — see the module docs.
/// `raw` and `official` must be the same size; `erase` and `bubbles` are
/// ignored when their size differs.
pub fn piece_mask(
    raw: &RgbaImage,
    official: &RgbaImage,
    boxes: &[Transform],
    erase: Option<&GrayImage>,
    bubbles: Option<&GrayImage>,
) -> GrayImage {
    let (w, h) = raw.dimensions();
    let mut out = GrayImage::new(w, h);
    if official.dimensions() != (w, h) || w == 0 || h == 0 {
        return out;
    }
    let erase = erase.filter(|m| m.dimensions() == (w, h));
    let bubbles = bubbles.filter(|m| m.dimensions() == (w, h));
    let rects = boxes.iter().map(|t| Rect::of(t, w, h)).collect::<Vec<_>>();
    // Independent page-sized passes, side by side (a deletion waits on this).
    let (changed, (owners, erase_near)) = rayon::join(
        || changed_pixels(raw, official),
        || {
            rayon::join(
                || ownership(&rects, bubbles, w, h),
                || erase.map(|m| boxed_erasure(m, &rects)),
            )
        },
    );
    if !changed.pixels().any(|p| p.0[0] != 0) {
        return out;
    }
    let own_distance = distance_transform(&owners.own, Norm::LInf);
    // Inside and outside bubbles decide disjoint pixels.
    let ((ours_in, theirs_in), (ours_out, theirs_out)) = rayon::join(
        || decide_in_bubbles(&changed, bubbles, &owners, &own_distance),
        || {
            decide_outside_bubbles(
                &changed,
                bubbles,
                &owners.over_art,
                &own_distance,
                erase_near.as_ref(),
            )
        },
    );
    let union = |a: &GrayImage, b: &GrayImage| {
        GrayImage::from_fn(w, h, |x, y| {
            Luma([a.get_pixel(x, y).0[0] | b.get_pixel(x, y).0[0]])
        })
    };
    let (grown, guard) = rayon::join(
        || dilate(&union(&theirs_in, &theirs_out), Norm::LInf, PASTE_GROW),
        || dilate(&union(&ours_in, &ours_out), Norm::LInf, PASTE_GROW),
    );
    for ((o, g), k) in out.pixels_mut().zip(grown.pixels()).zip(guard.pixels()) {
        if g.0[0] != 0 && k.0[0] == 0 {
            o.0[0] = 255;
        }
    }
    out
}

/// Inside bubbles: words of one bubble id, decided together. Returns the
/// owner's and the release's changed pixels.
fn decide_in_bubbles(
    changed: &GrayImage,
    bubbles: Option<&GrayImage>,
    owners: &Ownership,
    own_distance: &GrayImage,
) -> (GrayImage, GrayImage) {
    let (w, h) = changed.dimensions();
    let mut ours = GrayImage::new(w, h);
    let mut theirs = GrayImage::new(w, h);
    let Ownership { claimed, whole, .. } = owners;
    if let Some(bubbles) = bubbles {
        let mut inside = GrayImage::new(w, h);
        for (x, y, p) in changed.enumerate_pixels() {
            if p.0[0] != 0 && bubbles.get_pixel(x, y).0[0] != 0 {
                inside.put_pixel(x, y, Luma([255]));
            }
        }
        let mut spread = dilate(&inside, Norm::LInf, BUBBLE_GROUP_RADIUS);
        for (x, y, p) in spread.enumerate_pixels_mut() {
            if bubbles.get_pixel(x, y).0[0] == 0 {
                p.0[0] = 0;
            }
        }
        let labels = connected_components(&spread, Connectivity::Eight, Luma([0u8]));
        // A word is the owner's when it mostly lies in a box's own lobe —
        // "mostly", because the raw's lettering in a neighbouring moan
        // bubble can reach the seam. What cleanup erased doesn't count
        // here: in a bubble without a box it is a moan brushed out by hand,
        // which the release fills better than an empty bubble.
        let mut groups: HashMap<(u32, u8), Group> = HashMap::new();
        for (x, y, p) in inside.enumerate_pixels() {
            if p.0[0] == 0 {
                continue;
            }
            let id = bubbles.get_pixel(x, y).0[0];
            let g = groups.entry((labels.get_pixel(x, y).0[0], id)).or_default();
            g.pixels += 1;
            if claimed[id as usize] && own_distance.get_pixel(x, y).0[0] <= LOBE_RING {
                g.near += 1;
            }
        }
        for (x, y, p) in inside.enumerate_pixels() {
            if p.0[0] == 0 {
                continue;
            }
            let id = bubbles.get_pixel(x, y).0[0];
            let g = groups[&(labels.get_pixel(x, y).0[0], id)];
            let owned = whole[id as usize] || g.mostly_near();
            let target = if owned { &mut ours } else { &mut theirs };
            target.put_pixel(x, y, Luma([255]));
        }
    }
    (ours, theirs)
}

/// Outside bubbles: letters grouped into lines, big strokes on their own.
/// A sound effect's strokes are much bigger than letters; left in the
/// grouping they bridge an effect to the narration beside it (and the raw's
/// own effect, which the release removed, can sit under the owner's
/// narration box). Returns the owner's and the release's changed pixels.
fn decide_outside_bubbles(
    changed: &GrayImage,
    bubbles: Option<&GrayImage>,
    over_art: &[Rect],
    own_distance: &GrayImage,
    erase_near: Option<&GrayImage>,
) -> (GrayImage, GrayImage) {
    let (w, h) = changed.dimensions();
    let mut ours = GrayImage::new(w, h);
    let mut theirs = GrayImage::new(w, h);
    let in_bubble = |x: u32, y: u32| bubbles.map_or(0, |b| b.get_pixel(x, y).0[0]);
    let mut outside = GrayImage::new(w, h);
    for (x, y, p) in changed.enumerate_pixels() {
        if p.0[0] != 0 && in_bubble(x, y) == 0 {
            outside.put_pixel(x, y, Luma([255]));
        }
    }
    let strokes = connected_components(&outside, Connectivity::Eight, Luma([0u8]));
    let stroke_bounds = label_bounds(&strokes);
    let big_side = ((w as f32 * BIG_STROKE_FRAC) as u32).max(BIG_STROKE_MIN);
    let big = stroke_bounds
        .iter()
        .filter(|(_, b)| (b.x1 - b.x0).min(b.y1 - b.y0) >= big_side)
        .map(|(label, _)| *label)
        .collect::<std::collections::HashSet<_>>();
    let mut letters = GrayImage::new(w, h);
    for (x, y, p) in strokes.enumerate_pixels() {
        if p.0[0] != 0 && !big.contains(&p.0[0]) {
            letters.put_pixel(x, y, Luma([255]));
        }
    }
    let mut spread = dilate(&letters, Norm::LInf, GROUP_RADIUS);
    for (x, y, p) in spread.enumerate_pixels_mut() {
        if in_bubble(x, y) != 0 {
            p.0[0] = 0;
        }
    }
    let lines = connected_components(&spread, Connectivity::Eight, Luma([0u8]));
    let mut block = GrayImage::new(w, h);
    for rect in over_art {
        rect.grown(BOX_MARGIN, w, h).paint(&mut block);
    }
    // Groups: a line of letters (key: its label) or one big stroke (key:
    // its stroke label, offset past every line label).
    let offset = lines.pixels().map(|p| p.0[0]).max().unwrap_or(0) + 1;
    let key = |x: u32, y: u32| {
        let stroke = strokes.get_pixel(x, y).0[0];
        if big.contains(&stroke) {
            offset + stroke
        } else {
            lines.get_pixel(x, y).0[0]
        }
    };
    // Lines set in a typeface — the release's narration or notes, even when
    // placed far from the owner's box — never come over.
    let typeset = typeset_lines(&lines, &strokes, &stroke_bounds, &big);
    let mut groups: HashMap<u32, Group> = HashMap::new();
    for (x, y, p) in outside.enumerate_pixels() {
        if p.0[0] == 0 {
            continue;
        }
        let k = key(x, y);
        let g = groups.entry(k).or_default();
        if k < offset && typeset.contains(&k) {
            g.touches = true;
        }
        g.pixels += 1;
        let blocked = block.get_pixel(x, y).0[0] != 0
            || erase_near.is_some_and(|m| m.get_pixel(x, y).0[0] != 0);
        // Dialogue spilling past a bubble's mask lies near its lobe.
        if blocked || own_distance.get_pixel(x, y).0[0] <= SPILL_ZONE {
            g.near += 1;
        }
        if blocked {
            g.touches = true;
        }
    }
    for (x, y, p) in outside.enumerate_pixels() {
        if p.0[0] == 0 {
            continue;
        }
        let k = key(x, y);
        let g = groups[&k];
        // A line touching a box is its text; a big stroke only when it
        // mostly lies there (an effect may clip a narration box's corner).
        let owned = if k >= offset {
            g.mostly_near()
        } else {
            g.touches || g.mostly_near()
        };
        let target = if owned { &mut ours } else { &mut theirs };
        target.put_pixel(x, y, Luma([255]));
    }
    (ours, theirs)
}

/// Tally of one group of changed pixels.
#[derive(Default, Clone, Copy)]
struct Group {
    pixels: u32,
    /// Pixels near the owner's bubble parts.
    near: u32,
    /// Touches a box over artwork or what cleanup erases.
    touches: bool,
}

impl Group {
    fn mostly_near(self) -> bool {
        self.near * 2 >= self.pixels
    }
}

/// Which parts of the page's bubbles are the owner's.
struct Ownership {
    /// Each box's own lobe of its bubble, the box itself, and whole bubbles
    /// for boxes that can't be given a lobe.
    own: GrayImage,
    /// Bubble ids holding (part of) a box.
    claimed: [bool; 256],
    /// Bubble ids that are the owner's in full.
    whole: [bool; 256],
    /// Boxes over artwork (or straddling bubbles).
    over_art: Vec<Rect>,
}

fn ownership(rects: &[Rect], bubbles: Option<&GrayImage>, w: u32, h: u32) -> Ownership {
    let mut own = GrayImage::new(w, h);
    let mut claimed = [false; 256];
    let mut whole = [false; 256];
    let Some(bubbles) = bubbles else {
        return Ownership {
            own,
            claimed,
            whole,
            over_art: rects.to_vec(),
        };
    };
    let mut over_art = Vec::new();
    let index = BubbleIndex::new(bubbles.clone());
    let mut anchors: HashMap<u8, Vec<Rect>> = HashMap::new();
    for rect in rects {
        if rect.area() == 0 {
            continue;
        }
        match index.confident_match(rect.layout_box()) {
            Some(id) => anchors.entry(id).or_default().push(*rect),
            None => {
                over_art.push(*rect);
                for (id, count) in bubble_counts(bubbles, *rect) {
                    if count as f32 >= STRADDLE_SHARE * rect.area() as f32 {
                        claimed[id as usize] = true;
                        whole[id as usize] = true;
                    }
                }
            }
        }
    }
    for (id, list) in &anchors {
        claimed[*id as usize] = true;
        let layout = list.iter().map(|r| r.layout_box()).collect::<Vec<_>>();
        for (rect, shape) in list.iter().zip(index.balloon_shapes(*id, &layout)) {
            match shape {
                Some(shape) => paint_shape(&mut own, &shape),
                None => whole[*id as usize] = true,
            }
            rect.paint(&mut own);
        }
    }
    for (x, y, id) in bubbles.enumerate_pixels() {
        if whole[id.0[0] as usize] && id.0[0] != 0 {
            own.put_pixel(x, y, Luma([255]));
        }
    }
    Ownership {
        own,
        claimed,
        whole,
        over_art,
    }
}

/// The owner's parts of the page's bubbles (white), for evaluating the rule.
#[doc(hidden)]
pub fn owned_bubble_parts(boxes: &[Transform], bubbles: &GrayImage) -> GrayImage {
    let (w, h) = bubbles.dimensions();
    let rects = boxes.iter().map(|t| Rect::of(t, w, h)).collect::<Vec<_>>();
    ownership(&rects, Some(bubbles), w, h).own
}

/// Pixels where the release differs from the raw page, without specks.
fn changed_pixels(raw: &RgbaImage, official: &RgbaImage) -> GrayImage {
    let (w, h) = raw.dimensions();
    let mut changed = GrayImage::new(w, h);
    for ((c, r), o) in changed
        .pixels_mut()
        .zip(raw.pixels())
        .zip(official.pixels())
    {
        let d = (0..3).map(|i| r.0[i].abs_diff(o.0[i])).max().unwrap_or(0);
        if d > CHANGED_THRESHOLD {
            c.0[0] = 255;
        }
    }
    let changed = open(&changed, Norm::LInf, 1);
    let labels = connected_components(&changed, Connectivity::Eight, Luma([0u8]));
    let mut sizes: HashMap<u32, usize> = HashMap::new();
    for p in labels.pixels() {
        if p.0[0] != 0 {
            *sizes.entry(p.0[0]).or_default() += 1;
        }
    }
    GrayImage::from_fn(w, h, |x, y| {
        let label = labels.get_pixel(x, y).0[0];
        Luma([if label != 0 && sizes[&label] >= MIN_PIECE_PX {
            255
        } else {
            0
        }])
    })
}

/// The erased areas (text blocks of the erase mask) touching a box, grown
/// by `ERASE_MARGIN`. Areas touching no box are repair strokes — a brushed
/// out sound effect the release has its own version of.
fn boxed_erasure(erase: &GrayImage, rects: &[Rect]) -> GrayImage {
    let (w, h) = erase.dimensions();
    let solid = GrayImage::from_fn(w, h, |x, y| {
        Luma([if erase.get_pixel(x, y).0[0] > 127 {
            255
        } else {
            0
        }])
    });
    let areas = connected_components(
        &dilate(&solid, Norm::LInf, ERASE_MERGE),
        Connectivity::Eight,
        Luma([0u8]),
    );
    let mut touching = std::collections::HashSet::new();
    for rect in rects {
        let r = rect.grown(BOX_MARGIN, w, h);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let area = areas.get_pixel(x, y).0[0];
                if area != 0 {
                    touching.insert(area);
                }
            }
        }
    }
    let kept = GrayImage::from_fn(w, h, |x, y| {
        let keep =
            solid.get_pixel(x, y).0[0] != 0 && touching.contains(&areas.get_pixel(x, y).0[0]);
        Luma([if keep { 255 } else { 0 }])
    });
    dilate(&kept, Norm::LInf, ERASE_MARGIN)
}

type Labels = image::ImageBuffer<Luma<u32>, Vec<u32>>;

/// Bounding box of every label.
fn label_bounds(labels: &Labels) -> HashMap<u32, Rect> {
    let mut bounds: HashMap<u32, Rect> = HashMap::new();
    for (x, y, p) in labels.enumerate_pixels() {
        let label = p.0[0];
        if label == 0 {
            continue;
        }
        let b = bounds.entry(label).or_insert(Rect {
            x0: x,
            y0: y,
            x1: x + 1,
            y1: y + 1,
        });
        b.x0 = b.x0.min(x);
        b.y0 = b.y0.min(y);
        b.x1 = b.x1.max(x + 1);
        b.y1 = b.y1.max(y + 1);
    }
    bounds
}

/// A typeset line (group of lines) has at least this many letters
/// (strokes)...
const TYPESET_MIN_LETTERS: usize = 16;
/// ...of readable size (median height at least this share of the page
/// width; BadEnd narration is ~1.7 %)...
const TYPESET_MIN_HEIGHT_FRAC: f32 = 0.011;
/// ...and even height (median absolute deviation within this share of the
/// median)...
const TYPESET_HEIGHT_SPREAD: f32 = 0.25;
/// ...and runs at least this many letter heights wide. Hand-drawn effects,
/// moans, dots and hearts are few, small, big or uneven marks.
const TYPESET_MIN_RUN: f32 = 8.0;

/// Line groups (labels of `lines`) that look set in a typeface.
fn typeset_lines(
    lines: &Labels,
    strokes: &Labels,
    stroke_bounds: &HashMap<u32, Rect>,
    big: &std::collections::HashSet<u32>,
) -> std::collections::HashSet<u32> {
    let min_height = lines.width() as f32 * TYPESET_MIN_HEIGHT_FRAC;
    // Each letter's line: sample the line label at one of its pixels.
    let mut letters_of: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for (x, y, p) in strokes.enumerate_pixels() {
        let stroke = p.0[0];
        if stroke == 0 || big.contains(&stroke) || !seen.insert(stroke) {
            continue;
        }
        let line = lines.get_pixel(x, y).0[0];
        if line != 0 {
            letters_of.entry(line).or_default().push(stroke);
        }
    }
    let line_bounds = label_bounds(lines);
    letters_of
        .into_iter()
        .filter(|(line, letters)| {
            if letters.len() < TYPESET_MIN_LETTERS {
                return false;
            }
            let mut heights = letters
                .iter()
                .map(|s| {
                    let b = stroke_bounds[s];
                    (b.y1 - b.y0) as f32
                })
                .collect::<Vec<_>>();
            heights.sort_by(f32::total_cmp);
            let median = heights[heights.len() / 2];
            let mut deviation = heights
                .iter()
                .map(|h| (h - median).abs())
                .collect::<Vec<_>>();
            deviation.sort_by(f32::total_cmp);
            let spread = deviation[deviation.len() / 2];
            let run = line_bounds.get(line).map_or(0.0, |b| (b.x1 - b.x0) as f32);
            median >= min_height.max(1.0)
                && spread <= TYPESET_HEIGHT_SPREAD * median
                && run >= TYPESET_MIN_RUN * median
        })
        .map(|(line, _)| line)
        .collect()
}

fn bubble_counts(bubbles: &GrayImage, rect: Rect) -> HashMap<u8, u32> {
    let mut counts = HashMap::new();
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let id = bubbles.get_pixel(x, y).0[0];
            if id != 0 {
                *counts.entry(id).or_default() += 1;
            }
        }
    }
    counts
}

fn paint_shape(mask: &mut GrayImage, shape: &BalloonShape) {
    let (w, h) = mask.dimensions();
    for (i, row) in shape.rows.iter().enumerate() {
        let Some((left, right)) = row else { continue };
        let y = (shape.frame.y + i as f32).round();
        if y < 0.0 || y >= h as f32 {
            continue;
        }
        let x0 = (shape.frame.x + left).floor().max(0.0) as u32;
        let x1 = ((shape.frame.x + right).ceil().max(0.0) as u32).min(w);
        for x in x0..x1 {
            mask.put_pixel(x, y as u32, Luma([255]));
        }
    }
}

// ---------------------------------------------------------------------------
// Pairing pages with their official release
// ---------------------------------------------------------------------------

/// Width of the grey thumbnails pages are compared by.
const PAIR_THUMB_WIDTH: u32 = 64;
/// A true pair differs by lettering only: 1-5 levels mean absolute
/// difference on BadEnd, other pages 55+.
const PAIR_MAX_DIFF: f32 = 12.0;
/// ...and must beat the runner-up clearly (blank or near-blank pages look
/// alike).
const PAIR_MIN_MARGIN: f32 = 10.0;

/// What a page (or a candidate release page) is compared by.
pub struct PairingThumb {
    width: u32,
    height: u32,
    pixels: Vec<f32>,
}

impl PairingThumb {
    pub fn new(image: &DynamicImage) -> Self {
        let (width, height) = image.dimensions();
        let tw = PAIR_THUMB_WIDTH.min(width.max(1));
        let th = ((height as f32 * tw as f32 / width.max(1) as f32).round() as u32).max(1);
        let thumb = image
            .resize_exact(tw, th, image::imageops::FilterType::Triangle)
            .to_luma8();
        Self {
            width,
            height,
            pixels: thumb.pixels().map(|p| p.0[0] as f32).collect(),
        }
    }

    fn difference(&self, other: &Self) -> Option<f32> {
        if (self.width, self.height) != (other.width, other.height)
            || self.pixels.len() != other.pixels.len()
        {
            return None;
        }
        let sum: f32 = self
            .pixels
            .iter()
            .zip(&other.pixels)
            .map(|(a, b)| (a - b).abs())
            .sum();
        Some(sum / self.pixels.len().max(1) as f32)
    }
}

/// Pair `pages` with `candidates` by picture: same size, near-identical
/// thumbnails, clearly better than any other candidate. Returns, per page,
/// the index of its candidate. Each candidate is used at most once; closer
/// pairs are settled first.
pub fn pair_by_picture(pages: &[PairingThumb], candidates: &[PairingThumb]) -> Vec<Option<usize>> {
    let mut scored = Vec::new();
    for (p, page) in pages.iter().enumerate() {
        let mut diffs = candidates
            .iter()
            .enumerate()
            .filter_map(|(c, cand)| page.difference(cand).map(|d| (d, c)))
            .collect::<Vec<_>>();
        diffs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let Some(&(best, c)) = diffs.first() else {
            continue;
        };
        let runner_up = diffs.get(1).map_or(f32::INFINITY, |d| d.0);
        if best <= PAIR_MAX_DIFF && runner_up - best >= PAIR_MIN_MARGIN {
            scored.push((best, p, c));
        }
    }
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = vec![None; pages.len()];
    let mut used = vec![false; candidates.len()];
    for (_, p, c) in scored {
        if !used[c] && out[p].is_none() {
            used[c] = true;
            out[p] = Some(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const W: u32 = 400;
    const H: u32 = 300;

    fn page(color: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(W, H, Rgba([color[0], color[1], color[2], 255]))
    }

    fn fill(img: &mut RgbaImage, [x0, y0, x1, y1]: [u32; 4], color: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }

    fn fill_mask(mask: &mut GrayImage, [x0, y0, x1, y1]: [u32; 4], value: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                mask.put_pixel(x, y, Luma([value]));
            }
        }
    }

    fn boxed([x0, y0, x1, y1]: [u32; 4]) -> Transform {
        Transform {
            x: x0 as f32,
            y: y0 as f32,
            width: (x1 - x0) as f32,
            height: (y1 - y0) as f32,
            rotation_deg: 0.0,
        }
    }

    fn set(mask: &GrayImage, x: u32, y: u32) -> bool {
        mask.get_pixel(x, y).0[0] != 0
    }

    #[test]
    fn identical_pages_have_no_pieces() {
        let raw = page([200, 200, 200]);
        let mask = piece_mask(&raw, &raw.clone(), &[], None, None);
        assert!(mask.pixels().all(|p| p.0[0] == 0));
    }

    #[test]
    fn effect_over_art_comes_over_but_boxed_narration_does_not() {
        let raw = page([200, 200, 200]);
        let mut official = raw.clone();
        // A sound effect on the artwork, far from any box.
        fill(&mut official, [300, 40, 360, 90], [10, 10, 10]);
        // Re-lettered narration spilling past the owner's box.
        fill(&mut official, [40, 200, 180, 216], [10, 10, 10]);
        let boxes = [boxed([40, 180, 140, 214])];
        let mask = piece_mask(&raw, &official, &boxes, None, None);
        assert!(set(&mask, 330, 60), "the effect is a piece");
        assert!(
            !set(&mask, 170, 208),
            "narration past the box stays the owner's"
        );
        assert!(!set(&mask, 60, 208));
    }

    #[test]
    fn typeset_narration_far_from_any_box_stays_but_an_effect_comes_over() {
        let raw = RgbaImage::from_pixel(1200, 600, Rgba([200, 200, 200, 255]));
        let mut official = raw.clone();
        // The release's narration, set in a typeface: 18 even letters on a
        // line, placed far from where the owner's box is.
        for i in 0..18 {
            let x = 100 + i * 30;
            fill(&mut official, [x, 400, x + 18, 430], [10, 10, 10]);
        }
        // A hand-drawn effect: three big uneven letters.
        fill(&mut official, [800, 80, 860, 200], [10, 10, 10]);
        fill(&mut official, [880, 60, 950, 220], [10, 10, 10]);
        fill(&mut official, [970, 100, 1010, 170], [10, 10, 10]);
        let boxes = [boxed([100, 40, 300, 120])];
        let mask = piece_mask(&raw, &official, &boxes, None, None);
        assert!(!set(&mask, 105, 410), "typeset narration is never pasted");
        assert!(set(&mask, 900, 150), "the effect comes over");
    }

    #[test]
    fn tiny_specks_are_ignored() {
        let raw = page([200, 200, 200]);
        let mut official = raw.clone();
        fill(&mut official, [100, 100, 104, 104], [0, 0, 0]);
        let mask = piece_mask(&raw, &official, &[], None, None);
        assert!(!set(&mask, 101, 101));
    }

    #[test]
    fn erasure_near_a_box_claims_its_group_a_far_one_does_not() {
        let raw = page([200, 200, 200]);
        let mut official = raw.clone();
        // Narration re-lettered where the Korean was, the owner's box moved
        // down so it only overlaps the bottom of the Korean block; cleanup
        // still erases the block's original place.
        fill(&mut official, [100, 100, 160, 130], [10, 10, 10]);
        let mut erase = GrayImage::new(W, H);
        fill_mask(&mut erase, [95, 95, 165, 150], 255);
        let boxes = [boxed([100, 145, 170, 190])];
        let mask = piece_mask(&raw, &official, &boxes, Some(&erase), None);
        assert!(!set(&mask, 110, 110), "cleanup erased part of it: owner's");

        // The same erasure with the box far away is a repair stroke over a
        // sound effect: the release's version comes over.
        let big = RgbaImage::from_pixel(W * 2, H * 2, Rgba([200, 200, 200, 255]));
        let mut big_official = big.clone();
        fill(&mut big_official, [100, 100, 160, 130], [10, 10, 10]);
        let mut big_erase = GrayImage::new(W * 2, H * 2);
        fill_mask(&mut big_erase, [150, 120, 158, 128], 255);
        let far = [boxed([600, 500, 640, 540])];
        let mask = piece_mask(&big, &big_official, &far, Some(&big_erase), None);
        assert!(set(&mask, 110, 110));
    }

    #[test]
    fn brushed_out_moan_bubble_gets_the_release() {
        let raw = page([200, 200, 200]);
        let mut official = raw.clone();
        let mut bubbles = GrayImage::new(W, H);
        fill_mask(&mut bubbles, [220, 20, 380, 140], 2);
        fill(&mut official, [260, 60, 340, 100], [0, 0, 0]);
        let mut erase = GrayImage::new(W, H);
        fill_mask(&mut erase, [250, 50, 350, 110], 255);
        let boxes = [boxed([20, 200, 120, 260])];
        let mask = piece_mask(&raw, &official, &boxes, Some(&erase), Some(&bubbles));
        assert!(set(&mask, 300, 80));
    }

    #[test]
    fn bubble_without_a_box_comes_over_bubble_with_a_box_does_not() {
        let raw = page([200, 200, 200]);
        let mut official = raw.clone();
        let mut bubbles = GrayImage::new(W, H);
        // Bubble 1 holds the owner's box; bubble 2 is a moan they skipped.
        fill_mask(&mut bubbles, [20, 20, 180, 140], 1);
        fill_mask(&mut bubbles, [220, 20, 380, 140], 2);
        fill(&mut official, [50, 60, 150, 100], [0, 0, 0]);
        fill(&mut official, [260, 60, 340, 100], [0, 0, 0]);
        let boxes = [boxed([60, 50, 140, 110])];
        let mask = piece_mask(&raw, &official, &boxes, None, Some(&bubbles));
        assert!(!set(&mask, 100, 80), "dialogue bubble stays the owner's");
        assert!(set(&mask, 300, 80), "moan bubble comes over");
    }

    #[test]
    fn pieces_keep_away_from_the_owners_dialogue() {
        let raw = page([200, 200, 200]);
        let mut official = raw.clone();
        let mut bubbles = GrayImage::new(W, H);
        fill_mask(&mut bubbles, [20, 20, 180, 140], 1);
        fill(&mut official, [50, 60, 150, 100], [0, 0, 0]);
        let boxes = [boxed([60, 50, 140, 110])];
        let mask = piece_mask(&raw, &official, &boxes, None, Some(&bubbles));
        assert!(mask.pixels().all(|p| p.0[0] == 0));
    }

    #[test]
    fn apply_keeps_colour_type_and_brush_paint() {
        let raw = page([200, 200, 200]);
        let mut official = raw.clone();
        fill(&mut official, [300, 40, 360, 90], [10, 10, 10]);
        let mask = piece_mask(&raw, &official, &[], None, None);
        let pieces = Pieces {
            pixels: mask.pixels().filter(|p| p.0[0] != 0).count() as u64,
            official: official.clone(),
            mask,
        };
        assert!(!pieces.is_empty());
        let rgb = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(raw.clone()).to_rgb8());
        let applied = pieces.apply(&rgb);
        assert!(!applied.color().has_alpha());
        assert_eq!(applied.get_pixel(330, 60).0[..3], [10, 10, 10]);

        let mut brush = RgbaImage::new(W, H);
        brush.put_pixel(330, 60, Rgba([255, 0, 0, 255]));
        let mut out = raw.clone();
        assert!(pieces.apply_to(&mut out, Some(&brush)));
        assert_eq!(out.get_pixel(330, 60).0, [200, 200, 200, 255]);
        assert_eq!(out.get_pixel(331, 60).0, [10, 10, 10, 255]);
    }

    fn png(img: &RgbaImage) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn textured(seed: u32) -> RgbaImage {
        RgbaImage::from_fn(W, H, |x, y| {
            let v = ((x * 7 + y * 13 + seed * 101) % 200) as u8 + 20;
            Rgba([v, v, v, 255])
        })
    }

    fn image_node(role: ImageRole, blob: BlobRef) -> Node {
        Node {
            id: NodeId::new(),
            transform: Transform::default(),
            visible: role != ImageRole::Rendered,
            kind: NodeKind::Image(ImageData {
                role,
                blob,
                opacity: 1.0,
                natural_width: W,
                natural_height: H,
                name: None,
            }),
        }
    }

    #[test]
    fn plan_pairs_patches_the_cleaned_page_and_drops_its_render() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let raw = textured(1);
        let raw_blob = blobs
            .put_webp(&DynamicImage::ImageRgba8(raw.clone()))
            .unwrap();
        let cleaned = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(raw.clone()).to_rgb8());
        let cleaned_blob = blobs.put_webp(&cleaned).unwrap();
        let rendered_blob = blobs.put_webp(&cleaned).unwrap();

        let mut finished = Page::new("001.jpg", W, H);
        for node in [
            image_node(ImageRole::Source, raw_blob),
            image_node(ImageRole::Inpainted, cleaned_blob.clone()),
            image_node(ImageRole::Rendered, rendered_blob),
        ] {
            finished.nodes.insert(node.id, node);
        }
        let finished_id = finished.id;
        let other_blob = blobs
            .put_webp(&DynamicImage::ImageRgba8(textured(7)))
            .unwrap();
        let mut other = Page::new("002.jpg", W, H);
        let node = image_node(ImageRole::Source, other_blob);
        other.nodes.insert(node.id, node);
        let other_id = other.id;
        let mut scene = Scene::default();
        scene.pages.insert(finished.id, finished);
        scene.pages.insert(other.id, other);

        // The release: the finished page with a sound effect, and a page of
        // another chapter.
        let mut release = raw.clone();
        fill(&mut release, [300, 40, 360, 90], [250, 20, 20]);
        let files = [
            ReleaseFile {
                name: "unrelated.png".into(),
                bytes: png(&textured(50)),
            },
            ReleaseFile {
                name: "001 eng.png".into(),
                bytes: png(&release),
            },
        ];
        let plan = plan_release(&scene, &blobs, &files).unwrap();
        assert_eq!(plan.matched, vec![(finished_id, "001 eng.png".to_string())]);
        assert_eq!(plan.unmatched_pages, vec![other_id]);
        assert_eq!(plan.unused_files, vec!["unrelated.png".to_string()]);
        assert_eq!(plan.rerender, vec![finished_id]);

        let before = scene.clone();
        let mut op = Op::Batch {
            ops: plan.ops,
            label: "Add official pages".into(),
        };
        op.apply(&mut scene).unwrap();
        let page = &scene.pages[&finished_id];
        assert!(page.official_node().is_some());
        let roles = page
            .nodes
            .values()
            .filter_map(|n| match &n.kind {
                NodeKind::Image(img) => Some(img.role),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            roles,
            vec![ImageRole::Source, ImageRole::Official, ImageRole::Inpainted]
        );
        let cleaned = load_role(page, ImageRole::Inpainted, &blobs)
            .unwrap()
            .unwrap();
        assert!(
            !cleaned.color().has_alpha(),
            "keeps the layer's colour type"
        );
        assert_eq!(cleaned.get_pixel(330, 60).0[..3], [250, 20, 20]);
        assert_eq!(
            cleaned.get_pixel(10, 10).0[..3],
            raw.get_pixel(10, 10).0[..3]
        );

        // One undo step restores everything.
        op.inverse().apply(&mut scene).unwrap();
        let restored = &scene.pages[&finished_id];
        assert_eq!(restored.nodes.len(), before.pages[&finished_id].nodes.len());
        assert!(restored.official_node().is_none());
        assert_eq!(
            load_role(restored, ImageRole::Inpainted, &blobs)
                .unwrap()
                .unwrap()
                .get_pixel(330, 60)
                .0[..3],
            raw.get_pixel(330, 60).0[..3]
        );
    }

    fn thumb(seed: u32, lettering: bool) -> PairingThumb {
        let img = RgbaImage::from_fn(300, 400, |x, y| {
            let v = ((x * 7 + y * 13 + seed * 101) % 251) as u8;
            let v = if lettering && (100..140).contains(&x) && (100..120).contains(&y) {
                255
            } else {
                v
            };
            Rgba([v, v, v, 255])
        });
        PairingThumb::new(&DynamicImage::ImageRgba8(img))
    }

    #[test]
    fn pairs_pages_by_picture_not_order() {
        // The release has an extra page 2, so raws 2 and 3 are its 3 and 4.
        let raws = [thumb(1, false), thumb(2, false), thumb(3, false)];
        let release = [
            thumb(1, true),
            thumb(9, true),
            thumb(2, true),
            thumb(3, true),
        ];
        assert_eq!(
            pair_by_picture(&raws, &release),
            vec![Some(0), Some(2), Some(3)]
        );
    }

    #[test]
    fn unrelated_or_differently_sized_pages_stay_unpaired() {
        let raws = [thumb(1, false)];
        let other = [thumb(5, false)];
        assert_eq!(pair_by_picture(&raws, &other), vec![None]);
        let wide = RgbaImage::from_pixel(310, 400, Rgba([0, 0, 0, 255]));
        let wide = [PairingThumb::new(&DynamicImage::ImageRgba8(wide))];
        assert_eq!(pair_by_picture(&raws, &wide), vec![None]);
    }
}
