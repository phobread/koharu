//! Koharu text renderer.
//!
//! Owns the font book, symbol fallbacks, and Google Fonts service. Exposes
//! [`Renderer::render_page`], which rasterises each text block's translation
//! into an RGBA sprite and composites them onto the inpainted plane.
//!
//! Pure output: the pipeline engine ([`crate::pipeline::engines::renderer`])
//! takes a `RenderOutput` and translates sprites + final composite into ops.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage, RgbaImage, imageops};
use koharu_core::{
    FontFaceInfo, FontPrediction, FontSource, NodeId, TextAlign, TextDirection, TextShaderEffect,
    TextStrokeStyle, TextStyle, Transform,
};

use koharu_renderer::{
    TextAlign as RendererTextAlign, TextShaderEffect as RendererEffect,
    font::{FaceInfo, Font, FontBook},
    layout::{LayoutRun, TextLayout, WritingMode},
    renderer::{RasterOptions, RenderOptions, RenderStrokeOptions, TinySkiaRenderer},
    text::{
        latin::{BubbleIndex, LayoutBox},
        script::{font_families_for_text, writing_mode_for_block},
    },
    types::{RenderBlock, TextDirection as RendererTextDirection},
};

use crate::custom_fonts::{CustomFontFace, CustomFontStore};
use crate::google_fonts::GoogleFontService;

// ---------------------------------------------------------------------------
// Inputs / outputs
// ---------------------------------------------------------------------------

/// Per-block input (immutable snapshot of a scene text node).
#[derive(Debug, Clone)]
pub struct RenderBlockInput {
    pub node_id: NodeId,
    pub transform: Transform,
    pub translation: String,
    pub style: Option<TextStyle>,
    pub font_prediction: Option<FontPrediction>,
    pub source_direction: Option<TextDirection>,
    pub rendered_direction: Option<TextDirection>,
    pub lock_layout_box: bool,
}

/// Document-level render options (shared across all blocks).
#[derive(Debug, Clone, Default)]
pub struct PageRenderOptions {
    pub shader_effect: TextShaderEffect,
    pub shader_stroke: Option<TextStrokeStyle>,
    pub document_font: Option<String>,
    /// Global default text size. Caps the auto-fit search so text is at most
    /// this size but still shrinks to fit its box. A per-node explicit
    /// `style.font_size` overrides it. `None` keeps the box-derived cap.
    pub document_font_size: Option<f32>,
    /// Global default alignment used when a block has no explicit
    /// `style.text_align`. `None` keeps the renderer's centre default.
    pub document_align: Option<TextAlign>,
    /// Pixels to inset text from each edge of its layout box (stops glyphs and
    /// strokes clipping at the box border). `0.0` keeps the original box.
    pub box_padding: f32,
    pub target_language: Option<String>,
    pub raster: RasterOptions,
}

/// Per-block sprite output. `transform` becomes `TextData.sprite_transform`
/// when the renderer expanded the layout beyond the original bubble.
pub struct RenderedBlock {
    pub node_id: NodeId,
    pub sprite: DynamicImage,
    pub rendered_direction: TextDirection,
    pub expanded_transform: Option<Transform>,
    /// Font size the fit actually settled on (auto-fit result or explicit
    /// override) — persisted so the UI can scale text with box resizes.
    pub font_size: f32,
}

/// Result of rendering a whole page.
pub struct RenderOutput {
    pub final_render: DynamicImage,
    pub blocks: Vec<RenderedBlock>,
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

pub struct Renderer {
    fontbook: Arc<Mutex<FontBook>>,
    renderer: TinySkiaRenderer,
    symbol_fallbacks: Vec<Font>,
    pub google_fonts: Arc<GoogleFontService>,
    custom_fonts: Arc<CustomFontStore>,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let mut fontbook = FontBook::new();
        let symbol_fallbacks = load_symbol_fallbacks(&mut fontbook);
        let app_data_root = koharu_runtime::default_app_data_root();
        let google_fonts = Arc::new(
            GoogleFontService::new(&app_data_root)
                .context("failed to initialize Google Fonts service")?,
        );
        let custom_fonts = Arc::new(
            CustomFontStore::new(&app_data_root)
                .context("failed to initialize custom fonts store")?,
        );
        load_custom_fonts(&mut fontbook, &custom_fonts);
        Ok(Self {
            fontbook: Arc::new(Mutex::new(fontbook)),
            renderer: TinySkiaRenderer::new()?,
            symbol_fallbacks,
            google_fonts,
            custom_fonts,
        })
    }

    /// Import a font file the user uploaded: validate it parses, register it in
    /// the font book, and cache it under `fonts/custom` so it survives restart.
    /// Returns the added face(s) for the API to hand back to the picker.
    pub fn import_custom_font(&self, filename: &str, bytes: Vec<u8>) -> Result<Vec<FontFaceInfo>> {
        let face = {
            let mut fontbook = self
                .fontbook
                .lock()
                .map_err(|_| anyhow::anyhow!("failed to lock fontbook"))?;
            let font = fontbook
                .load_from_bytes(bytes.clone())
                .context("file is not a valid font")?;
            font_to_custom_face(&font)
        };
        if face.post_script_name.is_empty() {
            anyhow::bail!("font has no PostScript name");
        }
        self.custom_fonts
            .store_bytes(filename, &bytes)
            .context("failed to save custom font")?;
        self.custom_fonts.record(face.clone());
        Ok(vec![FontFaceInfo {
            family_name: face.family_name,
            post_script_name: face.post_script_name,
            source: FontSource::Custom,
            category: None,
            cached: true,
        }])
    }

    /// List system + cached Google Fonts for the API.
    pub fn available_fonts(&self) -> Result<Vec<FontFaceInfo>> {
        let fontbook = self
            .fontbook
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to lock fontbook"))?;
        let mut fonts = fontbook
            .all_families()
            .into_iter()
            .filter(|face| !face.post_script_name.is_empty())
            .map(|face| {
                let family_name = face
                    .families
                    .first()
                    .map(|(family, _)| family.clone())
                    .unwrap_or_else(|| face.post_script_name.clone());
                let source = if self.custom_fonts.is_custom(&face.post_script_name) {
                    FontSource::Custom
                } else {
                    FontSource::System
                };
                FontFaceInfo {
                    family_name,
                    post_script_name: face.post_script_name,
                    source,
                    category: None,
                    cached: true,
                }
            })
            .collect::<Vec<_>>();
        let catalog = self.google_fonts.catalog();
        for entry in &catalog.fonts {
            for variant in &entry.variants {
                // Unique PS name for Google Fonts to identify the specific weight/style
                let post_script_name = format!(
                    "{}:{}{}",
                    entry.family,
                    variant.weight,
                    if variant.style == "italic" { "i" } else { "" }
                );

                fonts.push(FontFaceInfo {
                    family_name: entry.family.clone(),
                    post_script_name,
                    source: FontSource::Google,
                    category: Some(entry.category.clone()),
                    cached: self.google_fonts.is_variant_cached(&entry.family, variant),
                });
            }
        }
        fonts.sort();
        Ok(fonts)
    }

    /// Render every block's translation, composite onto `inpainted`, return
    /// the full page + per-block sprites. Blocks with an empty translation
    /// are skipped (they appear as holes in the composite, falling through to
    /// the inpainted plane).
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(level = "info", skip_all, fields(blocks = blocks.len()))]
    pub fn render_page(
        &self,
        inpainted: &DynamicImage,
        brush_layer: Option<&DynamicImage>,
        bubble_mask: Option<&DynamicImage>,
        image_width: u32,
        image_height: u32,
        blocks: &[RenderBlockInput],
        opts: &PageRenderOptions,
    ) -> Result<RenderOutput> {
        let min_font = min_font_size_for_image(image_width, image_height);
        // Build the bubble index once per page. The mask encodes each
        // detected bubble as a distinct grayscale ID; the index scans
        // once to record per-ID bboxes and then answers seed→bbox
        // lookups in O(seed_area).
        let bubble_index: Option<BubbleIndex> = bubble_mask.map(|m| BubbleIndex::new(m.to_luma8()));
        let layout_boxes = resolve_layout_boxes(blocks, bubble_index.as_ref());
        let bubble_mask = bubble_index.as_ref().map(BubbleIndex::mask);

        let mut background = inpainted.to_rgba8();
        if let Some(brush) = brush_layer {
            imageops::overlay(&mut background, &brush.to_rgba8(), 0, 0);
        }

        let mut rendered_blocks = Vec::with_capacity(blocks.len());
        for (block, layout_box) in blocks.iter().zip(layout_boxes.iter().copied()) {
            match self.render_one(
                block,
                layout_box,
                &background,
                bubble_mask,
                &opts.shader_effect,
                &opts.shader_stroke,
                opts.document_font.as_deref(),
                opts.document_font_size,
                opts.document_align,
                opts.box_padding,
                opts.target_language.as_deref(),
                opts.raster,
                min_font,
            ) {
                Ok(Some(out)) => rendered_blocks.push(out),
                Ok(None) => {}
                Err(e) => tracing::warn!(node = %block.node_id, "render failed: {e:#}"),
            }
        }

        // Compose the final page: inpainted → brush → per-block sprites.
        let mut canvas = background;
        for out in &rendered_blocks {
            let (x, y) = placement_origin(find_input(blocks, out.node_id), &out.expanded_transform);
            imageops::overlay(&mut canvas, &out.sprite.to_rgba8(), x as i64, y as i64);
        }
        Ok(RenderOutput {
            final_render: DynamicImage::ImageRgba8(canvas),
            blocks: rendered_blocks,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_one(
        &self,
        block: &RenderBlockInput,
        resolved_box: ResolvedLayoutBox,
        background: &RgbaImage,
        bubble_mask: Option<&GrayImage>,
        effect: &TextShaderEffect,
        global_stroke: &Option<TextStrokeStyle>,
        document_font: Option<&str>,
        document_font_size: Option<f32>,
        document_align: Option<TextAlign>,
        box_padding: f32,
        _target_language: Option<&str>,
        raster: RasterOptions,
        min_font_size: f32,
    ) -> Result<Option<RenderedBlock>> {
        let translation = block.translation.trim();
        if translation.is_empty() {
            return Ok(None);
        }

        let layout_source = layout_source_from_input(block, translation);

        let mut style = block.style.clone().unwrap_or_else(|| TextStyle {
            font_families: Vec::new(),
            font_size: None,
            color: [0, 0, 0, 255],
            effect: None,
            stroke: None,
            text_align: None,
        });
        if style.font_families.is_empty()
            && let Some(font) = document_font
        {
            style.font_families.push(font.to_string());
        }
        apply_default_font_families(&mut style.font_families, translation);

        let font = self.select_font(&style)?;
        let block_effect = style.effect.unwrap_or(*effect);
        let color = resolve_text_color(
            &style,
            block.font_prediction.as_ref(),
            background,
            resolved_box.layout_box,
            bubble_mask,
            resolved_box.bubble_id,
        );

        let writing_mode = writing_mode_for_block(&layout_source);
        // Translations default to centre alignment inside a bubble — each
        // line sits centred above/below the others, matching manga
        // typesetting convention. Explicit `style.text_align` wins; otherwise
        // the document-level default applies, falling back to centre.
        let align = style
            .text_align
            .or(document_align)
            .map(core_align_to_renderer)
            .unwrap_or(RendererTextAlign::Center);
        // Inset the layout box so glyphs/strokes don't clip at the box edge.
        // Symmetric inset preserves the box centre (and thus sprite centring).
        let layout_box = inset_layout_box(resolved_box.layout_box, box_padding);
        // A manually drawn / resized box (`lock_layout_box`) is an explicit size
        // choice, so let text shrink to fit it rather than bottoming out at the
        // image-wide readability floor and overflowing the box — the centred
        // sprite would otherwise spill past both edges and look de-centred
        // (issue #223). Auto-detected boxes keep the floor so OCR text stays
        // legible.
        let min_font_size = effective_min_font_size(min_font_size, block.lock_layout_box);

        // Never break words with a hyphen — the auto-fit search shrinks the
        // font until the longest word fits on a line instead. The core
        // layouter hyphenates by default (English), so opt out explicitly.
        // `target_language` stays plumbed in case hyphenation returns as an
        // opt-in setting.
        let layout_builder = TextLayout::new(&font, None)
            .with_fallback_fonts(&self.symbol_fallbacks)
            .with_writing_mode(writing_mode)
            .with_alignment(align)
            .without_hyphenation();
        // A document default size caps the auto-fit search (text still shrinks
        // to fit a tight box); otherwise the cap is derived from the box.
        let max_font = match document_font_size {
            Some(size) => size.max(min_font_size + 1.0),
            None => max_font_size_for_box(layout_box, min_font_size),
        };
        // Reserve clearance for the outline. The sprite canvas is sized to the
        // glyph fill and the stroke paints *outward* beyond it, so without
        // canvas padding the outline clips at the sprite edge no matter how
        // far the layout box is inset. Resolve the stroke at the largest
        // candidate size — stroke width never shrinks as fonts grow, so the
        // bound holds for the final fit — shrink the fit constraint by that
        // clearance, and pad the canvas by the same amount per candidate in
        // `render_candidate`. Fill + clearance then always fits `layout_box`,
        // and the symmetric inset keeps the sprite centred.
        let fit_clearance = stroke_clearance(
            resolve_stroke_style(
                block.font_prediction.as_ref(),
                style.stroke.as_ref(),
                global_stroke.as_ref(),
                max_font,
                color,
            )
            .as_ref(),
        );
        let fit_box = inset_layout_box(layout_box, fit_clearance);
        let mut render_candidate = |layout: &LayoutRun<'_>| -> Result<RenderedTextCandidate> {
            let resolved_stroke = resolve_stroke_style(
                block.font_prediction.as_ref(),
                style.stroke.as_ref(),
                global_stroke.as_ref(),
                layout.font_size,
                color,
            );

            let rendered = self.renderer.render(
                layout,
                writing_mode,
                &RenderOptions {
                    font_size: layout.font_size,
                    color,
                    effect: shader_core_to_renderer(block_effect),
                    padding: stroke_clearance(resolved_stroke.as_ref()),
                    stroke: resolved_stroke,
                    raster,
                    ..Default::default()
                },
            )?;
            let transform = centred_sprite_transform(
                layout_box,
                rendered.width(),
                rendered.height(),
                block.transform.rotation_deg,
            );
            Ok(RenderedTextCandidate {
                image: rendered,
                transform,
                font_size: layout.font_size,
            })
        };

        if let Some((mask, bubble_id)) = bubble_mask.zip(resolved_box.bubble_id) {
            let candidate = fit_rendered_with_mask_collision(
                &layout_builder,
                translation,
                fit_box,
                style.font_size,
                min_font_size,
                max_font,
                mask,
                bubble_id,
                &mut render_candidate,
            )?;
            return Ok(Some(RenderedBlock {
                node_id: block.node_id,
                sprite: DynamicImage::ImageRgba8(candidate.image),
                rendered_direction: rendered_direction_for_writing_mode(writing_mode),
                expanded_transform: Some(candidate.transform),
                font_size: candidate.font_size,
            }));
        }

        let layout = fit_font_size(
            &layout_builder,
            translation,
            fit_box.width,
            fit_box.height,
            style.font_size,
            min_font_size,
            max_font,
        )?;

        let candidate = render_candidate(&layout)?;

        Ok(Some(RenderedBlock {
            node_id: block.node_id,
            sprite: DynamicImage::ImageRgba8(candidate.image),
            rendered_direction: rendered_direction_for_writing_mode(writing_mode),
            expanded_transform: Some(candidate.transform),
            font_size: candidate.font_size,
        }))
    }

    /// Resolve a set of font family candidates into a single PostScript name.
    pub fn resolve_post_script_name(
        &self,
        style: &TextStyle,
        text: Option<&str>,
    ) -> Result<String> {
        let fontbook = self
            .fontbook
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to lock fontbook"))?;
        let faces = fontbook.all_families();

        let mut families = style.font_families.clone();
        if families.is_empty()
            && let Some(text) = text
        {
            tracing::debug!(
                "Families empty, applying script-based default font families for text: {}",
                text
            );
            apply_default_font_families(&mut families, text);
        }
        if families.is_empty() {
            families.push("ArialMT".to_string());
        }

        for candidate in &families {
            tracing::debug!("Attempting to resolve font candidate: {}", candidate);
            // 1. Exact PS name
            if let Some(face) = faces.iter().find(|f| f.post_script_name == *candidate) {
                tracing::debug!("Resolved via exact PS name: {}", face.post_script_name);
                return Ok(face.post_script_name.clone());
            }

            // 2. Google Font variant
            let (family, weight, style_str) = crate::google_fonts::parse_variant_query(candidate);
            if candidate.contains(':')
                && self
                    .google_fonts
                    .read_cached_variant(family, weight, style_str)
                    .map(|opt| opt.is_some())
                    .unwrap_or(false)
            {
                tracing::debug!("Resolved via Google Font variant: {}", candidate);
                return Ok(candidate.clone());
            }

            // 3. Fuzzy family name
            if let Some(psn) = face_post_script_name(&faces, candidate) {
                tracing::debug!("Resolved via fuzzy family name: {}", psn);
                return Ok(psn);
            }

            // 4. Base Google Font
            if self
                .google_fonts
                .read_cached_file(candidate)
                .map(|opt| opt.is_some())
                .unwrap_or(false)
            {
                tracing::debug!("Resolved via base Google Font: {}", candidate);
                return Ok(candidate.clone());
            }
        }

        tracing::warn!(?families, "font resolution failed, falling back to ArialMT");
        Ok("ArialMT".to_string())
    }

    fn select_font(&self, style: &TextStyle) -> Result<Font> {
        let mut fontbook = self
            .fontbook
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to lock fontbook"))?;
        for candidate in &style.font_families {
            let faces = fontbook.all_families();

            // 1. Try exact PostScript name match first (most reliable for variants)
            if let Some(face) = faces.iter().find(|f| f.post_script_name == *candidate) {
                return fontbook.load_font(face.id);
            }

            // 2. Check if it's a Google Font variant (Family:WeightStyle)
            let (family, weight, style_str) = crate::google_fonts::parse_variant_query(candidate);
            if candidate.contains(':')
                && let Some(data) = self
                    .google_fonts
                    .read_cached_variant(family, weight, style_str)?
            {
                let mut font = fontbook.load_from_bytes(data)?;

                // Explicitly set the weight and style for variable font instancing
                font.weight = weight;
                font.style = style_str.to_string();

                return Ok(font);
            }

            // 3. Try fuzzy family name match
            if let Some(psn) = face_post_script_name(&faces, candidate) {
                return fontbook.query(&psn);
            }

            // 4. Try base Google Font file
            if let Some(data) = self.google_fonts.read_cached_file(candidate)? {
                return fontbook.load_from_bytes(data);
            }
        }
        Err(anyhow::anyhow!(
            "no font found for candidates: {:?}",
            style.font_families
        ))
    }
}

// ---------------------------------------------------------------------------
// Helpers: font sizing
// ---------------------------------------------------------------------------

const MASK_COLLISION_ALPHA_THRESHOLD: u8 = 8;
const FIT_EPSILON: f32 = 0.5;

struct RenderedTextCandidate {
    image: RgbaImage,
    transform: Transform,
    font_size: f32,
}

struct MaskCollisionAttempt {
    candidate: RenderedTextCandidate,
    valid: bool,
}

fn min_font_size_for_image(image_width: u32, image_height: u32) -> f32 {
    let max_dim = image_width.max(image_height) as f32;
    (max_dim / 90.0).clamp(12.0, 28.0)
}

/// Floor for text in a manually drawn / resized box. Such a box is an explicit
/// size choice, so text may shrink well below the auto readability floor to
/// stay inside it (issue #223).
const MANUAL_MIN_FONT_SIZE: f32 = 6.0;

/// Effective minimum font size for a block. Manually-sized (locked) boxes are
/// allowed down to `MANUAL_MIN_FONT_SIZE` so text fits the box the user drew;
/// auto-detected boxes keep the image-derived readability floor. Never raises
/// the floor above the image minimum.
fn effective_min_font_size(image_min: f32, lock_layout_box: bool) -> f32 {
    if lock_layout_box {
        MANUAL_MIN_FONT_SIZE.min(image_min)
    } else {
        image_min
    }
}

/// Maximum font size for the given layout box, derived from its dimensions.
/// Caps extreme cases (huge empty bubble + short text → giant glyphs).
fn max_font_size_for_box(layout_box: LayoutBox, min_size: f32) -> f32 {
    const GLOBAL_CAP_PX: f32 = 72.0;
    let by_height = layout_box.height * 0.45;
    let by_width = layout_box.width * 0.9;
    by_height.min(by_width).clamp(min_size + 1.0, GLOBAL_CAP_PX)
}

/// Binary-search the largest integer font size in `[min_size, max_size]`
/// whose shaped layout still fits inside the constraint box. An
/// `explicit_size` override (user-set per-block font size) bypasses the
/// search.
fn fit_font_size<'a>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    constraint_width: f32,
    constraint_height: f32,
    explicit_size: Option<f32>,
    min_size: f32,
    max_size: f32,
) -> Result<LayoutRun<'a>> {
    let run_at = |size: f32| -> Result<LayoutRun<'a>> {
        layout_builder
            .clone()
            .with_font_size(size.max(1.0))
            .with_max_width(constraint_width)
            .with_max_height(constraint_height)
            .run(text)
    };
    if let Some(s) = explicit_size {
        return run_at(s);
    }

    let fits =
        |run: &LayoutRun<'a>| run.width <= constraint_width && run.height <= constraint_height;

    let min_size = min_size.max(1.0).round() as i32;
    let max_size = (max_size.round() as i32).max(min_size);

    let at_max = run_at(max_size as f32)?;
    if fits(&at_max) {
        return Ok(at_max);
    }
    // Binary-search [min, max) for the largest fitting size.
    let mut lo = min_size;
    let mut hi = max_size - 1;
    let mut best = run_at(min_size as f32)?;
    if !fits(&best) {
        // The readability floor is a preference, not a licence to overflow:
        // auto-fit text must always stay inside its box, so keep shrinking
        // below the floor until it fits. Only an explicit user-set size may
        // exceed the box.
        let mut lo = 1;
        let mut hi = min_size - 1;
        while lo <= hi {
            let mid = lo + (hi - lo) / 2;
            let candidate = run_at(mid as f32)?;
            if fits(&candidate) {
                best = candidate;
                lo = mid + 1;
            } else {
                hi = mid - 1;
            }
        }
        if !fits(&best) {
            // Nothing fits at any size — overflow as little as possible.
            return run_at(1.0);
        }
        return Ok(best);
    }
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let candidate = run_at(mid as f32)?;
        if fits(&candidate) {
            best = candidate;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    Ok(best)
}

#[allow(clippy::too_many_arguments)]
fn fit_rendered_with_mask_collision<'a, F>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    explicit_size: Option<f32>,
    min_size: f32,
    max_size: f32,
    mask: &GrayImage,
    bubble_id: u8,
    render_candidate: &mut F,
) -> Result<RenderedTextCandidate>
where
    F: FnMut(&LayoutRun<'a>) -> Result<RenderedTextCandidate>,
{
    if let Some(size) = explicit_size {
        let attempt = render_mask_collision_attempt(
            layout_builder,
            text,
            layout_box,
            size.max(1.0),
            mask,
            bubble_id,
            render_candidate,
        )?;
        return Ok(attempt.candidate);
    }

    let min_size = min_size.max(1.0).round() as i32;
    let max_size = (max_size.max(1.0).round() as i32).max(min_size);

    if let Some(candidate) = try_mask_collision_size(
        layout_builder,
        text,
        layout_box,
        max_size as f32,
        mask,
        bubble_id,
        render_candidate,
    )? {
        return Ok(candidate);
    }

    let min_attempt = render_mask_collision_attempt(
        layout_builder,
        text,
        layout_box,
        min_size as f32,
        mask,
        bubble_id,
        render_candidate,
    )?;
    if !min_attempt.valid {
        // Same soft floor as `fit_font_size`: prefer shrinking below the
        // readability minimum over spilling outside the box/bubble.
        let mut lo = 1;
        let mut hi = min_size - 1;
        let mut below_floor_best: Option<RenderedTextCandidate> = None;
        while lo <= hi {
            let mid = lo + (hi - lo) / 2;
            if let Some(candidate) = try_mask_collision_size(
                layout_builder,
                text,
                layout_box,
                mid as f32,
                mask,
                bubble_id,
                render_candidate,
            )? {
                below_floor_best = Some(candidate);
                lo = mid + 1;
            } else {
                hi = mid - 1;
            }
        }
        return Ok(below_floor_best.unwrap_or(min_attempt.candidate));
    }
    let mut best = min_attempt.candidate;

    let mut lo = min_size + 1;
    let mut hi = max_size - 1;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        if let Some(candidate) = try_mask_collision_size(
            layout_builder,
            text,
            layout_box,
            mid as f32,
            mask,
            bubble_id,
            render_candidate,
        )? {
            best = candidate;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }

    Ok(best)
}

#[allow(clippy::too_many_arguments)]
fn try_mask_collision_size<'a, F>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    font_size: f32,
    mask: &GrayImage,
    bubble_id: u8,
    render_candidate: &mut F,
) -> Result<Option<RenderedTextCandidate>>
where
    F: FnMut(&LayoutRun<'a>) -> Result<RenderedTextCandidate>,
{
    let layout = run_collision_layout_at(layout_builder, text, layout_box, font_size)?;
    let fits_layout_box = layout_fits_collision_attempt(&layout, layout_box);
    if !fits_layout_box {
        return Ok(None);
    }

    let candidate = render_candidate(&layout)?;
    if sprite_collides_with_bubble_mask(&candidate.image, &candidate.transform, mask, bubble_id) {
        return Ok(None);
    }
    Ok(Some(candidate))
}

#[allow(clippy::too_many_arguments)]
fn render_mask_collision_attempt<'a, F>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    font_size: f32,
    mask: &GrayImage,
    bubble_id: u8,
    render_candidate: &mut F,
) -> Result<MaskCollisionAttempt>
where
    F: FnMut(&LayoutRun<'a>) -> Result<RenderedTextCandidate>,
{
    let layout = run_collision_layout_at(layout_builder, text, layout_box, font_size)?;
    let fits_layout_box = layout_fits_collision_attempt(&layout, layout_box);
    let candidate = render_candidate(&layout)?;
    let valid = fits_layout_box
        && !sprite_collides_with_bubble_mask(
            &candidate.image,
            &candidate.transform,
            mask,
            bubble_id,
        );
    Ok(MaskCollisionAttempt { candidate, valid })
}

fn run_collision_layout_at<'a>(
    layout_builder: &TextLayout<'a>,
    text: &str,
    layout_box: LayoutBox,
    font_size: f32,
) -> Result<LayoutRun<'a>> {
    layout_builder
        .clone()
        .with_font_size(font_size.max(1.0))
        .with_max_width(layout_box.width.max(1.0))
        .with_max_height(layout_box.height.max(1.0))
        .run(text)
}

fn layout_fits_collision_attempt(layout: &LayoutRun<'_>, layout_box: LayoutBox) -> bool {
    layout.width <= layout_box.width + FIT_EPSILON
        && layout.height <= layout_box.height + FIT_EPSILON
}

fn sprite_collides_with_bubble_mask(
    sprite: &RgbaImage,
    transform: &Transform,
    mask: &GrayImage,
    bubble_id: u8,
) -> bool {
    let origin_x = transform.x.round() as i32;
    let origin_y = transform.y.round() as i32;
    let mask_w = mask.width() as i32;
    let mask_h = mask.height() as i32;

    for (x, y, pixel) in sprite.enumerate_pixels() {
        if pixel.0[3] <= MASK_COLLISION_ALPHA_THRESHOLD {
            continue;
        }
        let mask_x = origin_x + x as i32;
        let mask_y = origin_y + y as i32;
        if mask_x < 0 || mask_y < 0 || mask_x >= mask_w || mask_y >= mask_h {
            return true;
        }
        if mask.get_pixel(mask_x as u32, mask_y as u32).0[0] != bubble_id {
            return true;
        }
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResolvedLayoutBox {
    seed_box: LayoutBox,
    layout_box: LayoutBox,
    bubble_id: Option<u8>,
}

fn resolve_layout_boxes(
    blocks: &[RenderBlockInput],
    bubble_index: Option<&BubbleIndex>,
) -> Vec<ResolvedLayoutBox> {
    let Some(bubble_index) = bubble_index else {
        return blocks
            .iter()
            .map(|block| {
                let seed_box = seed_layout_box(block);
                ResolvedLayoutBox {
                    seed_box,
                    layout_box: seed_box,
                    bubble_id: None,
                }
            })
            .collect();
    };

    let mut counts: HashMap<u8, usize> = HashMap::new();
    let mut matches = Vec::with_capacity(blocks.len());

    for block in blocks {
        let seed_box = seed_layout_box(block);
        let translation = block.translation.trim();
        // Locked (manually sized/split) boxes never expand to the bubble's
        // safe area, but they must still *occupy* it: without counting them,
        // resizing one block in a shared bubble would leave its neighbour as
        // the sole occupant, blowing it up to the whole bubble and painting
        // over the locked box.
        let occupied = if translation.is_empty() {
            None
        } else {
            let layout_source = layout_source_from_input(block, translation);
            let writing_mode = writing_mode_for_block(&layout_source);
            bubble_index.lookup_match(seed_box, writing_mode)
        };
        if let Some(matched) = occupied {
            *counts.entry(matched.id).or_insert(0) += 1;
        }
        let bubble_match = if block.lock_layout_box {
            None
        } else {
            occupied
        };
        matches.push((seed_box, bubble_match));
    }

    matches
        .into_iter()
        .map(|(seed_box, bubble_match)| match bubble_match {
            // Connected bubbles can contain multiple independently detected
            // text blocks. Expanding all of them to the same safe area makes
            // their layouts collide, so shared bubbles keep each block's
            // original detector box.
            Some(matched) if counts.get(&matched.id).copied().unwrap_or(0) == 1 => {
                ResolvedLayoutBox {
                    seed_box,
                    layout_box: matched.layout_box,
                    bubble_id: Some(matched.id),
                }
            }
            Some(matched) => ResolvedLayoutBox {
                seed_box,
                layout_box: seed_box,
                bubble_id: Some(matched.id),
            },
            None => ResolvedLayoutBox {
                seed_box,
                layout_box: seed_box,
                bubble_id: None,
            },
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers: font families, fallbacks
// ---------------------------------------------------------------------------

fn apply_default_font_families(font_families: &mut Vec<String>, text: &str) {
    if font_families.is_empty() {
        *font_families = font_families_for_text(text);
    }
}

/// Register every cached custom font file into the book at startup, recording
/// each face so `available_fonts` can label it. Failures are logged and
/// skipped — one bad file shouldn't block the rest.
fn load_custom_fonts(fontbook: &mut FontBook, store: &CustomFontStore) {
    for path in store.files() {
        match std::fs::read(path.as_std_path()) {
            Ok(bytes) => match fontbook.load_from_bytes(bytes) {
                Ok(font) => store.record(font_to_custom_face(&font)),
                Err(e) => tracing::warn!(%path, "skipping invalid custom font: {e:#}"),
            },
            Err(e) => tracing::warn!(%path, "failed to read custom font: {e:#}"),
        }
    }
}

fn font_to_custom_face(font: &Font) -> CustomFontFace {
    let face = font.face_info();
    let family_name = face
        .families
        .first()
        .map(|(family, _)| family.clone())
        .unwrap_or_else(|| face.post_script_name.clone());
    CustomFontFace {
        post_script_name: face.post_script_name.clone(),
        family_name,
    }
}

fn load_symbol_fallbacks(fontbook: &mut FontBook) -> Vec<Font> {
    let candidates = [
        "Segoe UI Symbol",
        "Segoe UI Emoji",
        "Noto Sans Symbols",
        "Noto Sans Symbols2",
        "Noto Color Emoji",
        "Apple Color Emoji",
        "Apple Symbols",
        "Symbola",
        "Arial Unicode MS",
    ];
    let faces = fontbook.all_families();
    candidates
        .iter()
        .filter_map(|candidate| face_post_script_name(&faces, candidate))
        .filter_map(|post_script_name| fontbook.query(&post_script_name).ok())
        .collect()
}

fn face_post_script_name(faces: &[FaceInfo], candidate: &str) -> Option<String> {
    let candidate_lower = candidate.trim().to_lowercase();
    faces
        .iter()
        .find(|face| {
            face.post_script_name.to_lowercase() == candidate_lower
                || face
                    .families
                    .iter()
                    .any(|(family, _)| family.to_lowercase() == candidate_lower)
        })
        .map(|face| face.post_script_name.clone())
        .filter(|post_script_name| !post_script_name.is_empty())
}

fn layout_source_from_input(block: &RenderBlockInput, translation: &str) -> RenderBlock {
    RenderBlock {
        x: block.transform.x,
        y: block.transform.y,
        width: block.transform.width.max(1.0),
        height: block.transform.height.max(1.0),
        text: translation.to_string(),
        source_direction: block.source_direction.map(core_direction_to_renderer),
    }
}

fn seed_layout_box(block: &RenderBlockInput) -> LayoutBox {
    LayoutBox {
        x: block.transform.x,
        y: block.transform.y,
        width: block.transform.width.max(1.0),
        height: block.transform.height.max(1.0),
    }
}

/// Symmetrically shrink a layout box by `padding` px on every side, keeping a
/// positive size and the original centre. `padding <= 0` returns the box
/// unchanged. The inset is capped so it never collapses the box below ~2px.
fn inset_layout_box(layout_box: LayoutBox, padding: f32) -> LayoutBox {
    if padding <= 0.0 {
        return layout_box;
    }
    let max_w_inset = (layout_box.width - 2.0) * 0.5;
    let max_h_inset = (layout_box.height - 2.0) * 0.5;
    let inset = padding.min(max_w_inset).min(max_h_inset).max(0.0);
    LayoutBox {
        x: layout_box.x + inset,
        y: layout_box.y + inset,
        width: (layout_box.width - 2.0 * inset).max(1.0),
        height: (layout_box.height - 2.0 * inset).max(1.0),
    }
}

// ---------------------------------------------------------------------------
// Helpers: stroke resolution
// ---------------------------------------------------------------------------

fn default_stroke_width(font_size: f32) -> f32 {
    (font_size * 0.10).clamp(1.2, 8.0)
}

/// Clearance the sprite needs around the glyph fill for an outline to render
/// fully: the core stroke pass paints outward ~`width_px` beyond the fill,
/// plus 1px for anti-aliasing. Used both as canvas padding and as the fit
/// constraint inset so the padded sprite still fits its layout box.
fn stroke_clearance(stroke: Option<&RenderStrokeOptions>) -> f32 {
    stroke
        .map(|s| s.width_px.max(0.0).ceil() + 1.0)
        .unwrap_or(0.0)
}

fn contrasting_stroke_color(text_color: [u8; 4]) -> [u8; 4] {
    let luminance =
        0.299 * text_color[0] as f32 + 0.587 * text_color[1] as f32 + 0.114 * text_color[2] as f32;
    if luminance > 128.0 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    }
}

fn resolve_stroke_style(
    font_prediction: Option<&FontPrediction>,
    block_stroke: Option<&TextStrokeStyle>,
    global_stroke: Option<&TextStrokeStyle>,
    font_size: f32,
    text_color: [u8; 4],
) -> Option<RenderStrokeOptions> {
    if let Some(stroke) = block_stroke {
        if !stroke.enabled {
            return None;
        }
        return Some(RenderStrokeOptions {
            color: stroke.color,
            width_px: stroke
                .width_px
                .unwrap_or_else(|| default_stroke_width(font_size)),
        });
    }
    if let Some(stroke) = global_stroke {
        if !stroke.enabled {
            return None;
        }
        return Some(RenderStrokeOptions {
            color: stroke.color,
            width_px: stroke
                .width_px
                .unwrap_or_else(|| default_stroke_width(font_size)),
        });
    }
    Some(RenderStrokeOptions {
        color: contrasting_stroke_color(text_color),
        width_px: font_prediction
            .filter(|pred| pred.stroke_width_px > 0.0)
            .map(|pred| pred.stroke_width_px)
            .unwrap_or_else(|| default_stroke_width(font_size)),
    })
}

fn resolve_text_color(
    derived_style: &TextStyle,
    font_prediction: Option<&FontPrediction>,
    background: &RgbaImage,
    layout_box: LayoutBox,
    bubble_mask: Option<&GrayImage>,
    bubble_id: Option<u8>,
) -> [u8; 4] {
    if is_manual_text_color(derived_style.color, font_prediction) {
        return derived_style.color;
    }

    contrast_text_color(background, layout_box, bubble_mask, bubble_id)
}

fn is_manual_text_color(color: [u8; 4], font_prediction: Option<&FontPrediction>) -> bool {
    if color[3] != 255 {
        return true;
    }
    if color == [0, 0, 0, 255] {
        return false;
    }
    if let Some(pred) = font_prediction
        && color[0] == pred.text_color[0]
        && color[1] == pred.text_color[1]
        && color[2] == pred.text_color[2]
    {
        return false;
    }
    true
}

fn contrast_text_color(
    background: &RgbaImage,
    layout_box: LayoutBox,
    bubble_mask: Option<&GrayImage>,
    bubble_id: Option<u8>,
) -> [u8; 4] {
    let luminance =
        median_background_luminance(background, layout_box, bubble_mask, bubble_id).unwrap_or(1.0);
    let black_contrast = contrast_ratio(luminance, 0.0);
    let white_contrast = contrast_ratio(luminance, 1.0);
    if black_contrast >= white_contrast {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    }
}

fn median_background_luminance(
    background: &RgbaImage,
    layout_box: LayoutBox,
    bubble_mask: Option<&GrayImage>,
    bubble_id: Option<u8>,
) -> Option<f32> {
    let width = background.width();
    let height = background.height();
    if width == 0 || height == 0 {
        return None;
    }

    let left = layout_box.x.floor().max(0.0).min(width as f32) as u32;
    let top = layout_box.y.floor().max(0.0).min(height as f32) as u32;
    let right = (layout_box.x + layout_box.width)
        .ceil()
        .max(0.0)
        .min(width as f32) as u32;
    let bottom = (layout_box.y + layout_box.height)
        .ceil()
        .max(0.0)
        .min(height as f32) as u32;
    if right <= left || bottom <= top {
        return None;
    }

    let sample_area = (right - left).saturating_mul(bottom - top).max(1);
    let stride = ((sample_area as f32 / 10_000.0).sqrt().ceil() as u32).max(1);
    let mut samples = Vec::new();
    let mask_and_id = bubble_mask.zip(bubble_id);

    let mut y = top;
    while y < bottom {
        let mut x = left;
        while x < right {
            if let Some((mask, id)) = mask_and_id
                && (x >= mask.width() || y >= mask.height() || mask.get_pixel(x, y).0[0] != id)
            {
                x = x.saturating_add(stride);
                continue;
            }
            let pixel = background.get_pixel(x, y).0;
            samples.push(relative_luminance(pixel[0], pixel[1], pixel[2]));
            x = x.saturating_add(stride);
        }
        y = y.saturating_add(stride);
    }

    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    Some(samples[samples.len() / 2])
}

fn contrast_ratio(a: f32, b: f32) -> f32 {
    let lighter = a.max(b);
    let darker = a.min(b);
    (lighter + 0.05) / (darker + 0.05)
}

fn relative_luminance(r: u8, g: u8, b: u8) -> f32 {
    fn channel(v: u8) -> f32 {
        let normalized = v as f32 / 255.0;
        if normalized <= 0.03928 {
            normalized / 12.92
        } else {
            ((normalized + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

// ---------------------------------------------------------------------------
// Helpers: type conversions
// ---------------------------------------------------------------------------

fn shader_core_to_renderer(e: TextShaderEffect) -> RendererEffect {
    RendererEffect {
        italic: e.italic,
        bold: e.bold,
    }
}

fn core_align_to_renderer(a: koharu_core::TextAlign) -> RendererTextAlign {
    match a {
        koharu_core::TextAlign::Left => RendererTextAlign::Left,
        koharu_core::TextAlign::Center => RendererTextAlign::Center,
        koharu_core::TextAlign::Right => RendererTextAlign::Right,
    }
}

fn core_direction_to_renderer(d: TextDirection) -> RendererTextDirection {
    match d {
        TextDirection::Horizontal => RendererTextDirection::Horizontal,
        TextDirection::Vertical => RendererTextDirection::Vertical,
    }
}

fn rendered_direction_for_writing_mode(writing_mode: WritingMode) -> TextDirection {
    match writing_mode {
        WritingMode::Horizontal => TextDirection::Horizontal,
        WritingMode::VerticalRl => TextDirection::Vertical,
    }
}

// ---------------------------------------------------------------------------
// Helpers: placement
// ---------------------------------------------------------------------------

fn centred_sprite_transform(
    anchor_box: LayoutBox,
    sprite_width: u32,
    sprite_height: u32,
    rotation_deg: f32,
) -> Transform {
    let sprite_w = sprite_width as f32;
    let sprite_h = sprite_height as f32;
    let cx = anchor_box.x + anchor_box.width * 0.5;
    let cy = anchor_box.y + anchor_box.height * 0.5;
    Transform {
        x: (cx - sprite_w * 0.5).round(),
        y: (cy - sprite_h * 0.5).round(),
        width: sprite_w,
        height: sprite_h,
        rotation_deg,
    }
}

fn find_input(blocks: &[RenderBlockInput], id: NodeId) -> &RenderBlockInput {
    blocks
        .iter()
        .find(|b| b.node_id == id)
        .expect("rendered_block must have matching input")
}

fn placement_origin(input: &RenderBlockInput, expanded: &Option<Transform>) -> (f32, f32) {
    if let Some(t) = expanded {
        (t.x.round(), t.y.round())
    } else {
        (input.transform.x, input.transform.y)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma, Rgba, RgbaImage};
    use koharu_core::NodeId;

    #[test]
    fn default_font_families_should_fill_empty_list() {
        let mut font_families = Vec::new();
        apply_default_font_families(&mut font_families, "hello");
        assert!(!font_families.is_empty());
    }

    #[test]
    fn default_stroke_color_uses_black_for_light_text() {
        let stroke = resolve_stroke_style(None, None, None, 16.0, [255, 255, 255, 255])
            .expect("default stroke should be present");
        assert_eq!(stroke.color, [0, 0, 0, 255]);
        assert_eq!(stroke.width_px, 1.6);
    }

    #[test]
    fn predicted_stroke_keeps_width_but_uses_contrast_color() {
        let prediction = FontPrediction {
            stroke_color: [12, 34, 56],
            stroke_width_px: 3.0,
            ..Default::default()
        };
        let stroke =
            resolve_stroke_style(Some(&prediction), None, None, 18.0, [255, 255, 255, 255])
                .expect("predicted stroke should be present");
        assert_eq!(stroke.color, [0, 0, 0, 255]);
        assert_eq!(stroke.width_px, 3.0);
    }

    #[test]
    fn explicit_block_stroke_color_is_preserved_even_if_it_matches_text() {
        let stroke = resolve_stroke_style(
            None,
            Some(&TextStrokeStyle {
                enabled: true,
                color: [255, 255, 255, 255],
                width_px: Some(2.0),
            }),
            None,
            18.0,
            [255, 255, 255, 255],
        )
        .expect("explicit stroke should be present");
        assert_eq!(stroke.color, [255, 255, 255, 255]);
        assert_eq!(stroke.width_px, 2.0);
    }

    #[test]
    fn auto_text_color_ignores_prediction_and_picks_black_on_light_background() {
        let derived = TextStyle {
            font_families: Vec::new(),
            font_size: None,
            color: [0, 0, 0, 255],
            effect: None,
            stroke: None,
            text_align: None,
        };
        let prediction = FontPrediction {
            text_color: [12, 34, 56],
            ..Default::default()
        };
        let background = RgbaImage::from_pixel(32, 32, Rgba([245, 245, 245, 255]));
        assert_eq!(
            resolve_text_color(
                &derived,
                Some(&prediction),
                &background,
                LayoutBox {
                    x: 0.0,
                    y: 0.0,
                    width: 32.0,
                    height: 32.0
                },
                None,
                None,
            ),
            [0, 0, 0, 255]
        );
    }

    #[test]
    fn auto_text_color_picks_white_on_dark_background() {
        let derived = TextStyle {
            font_families: Vec::new(),
            font_size: None,
            color: [0, 0, 0, 255],
            effect: None,
            stroke: None,
            text_align: None,
        };
        let background = RgbaImage::from_pixel(32, 32, Rgba([24, 24, 24, 255]));
        assert_eq!(
            resolve_text_color(
                &derived,
                None,
                &background,
                LayoutBox {
                    x: 0.0,
                    y: 0.0,
                    width: 32.0,
                    height: 32.0
                },
                None,
                None,
            ),
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn manual_colored_text_wins_over_auto_contrast() {
        let explicit = TextStyle {
            font_families: Vec::new(),
            font_size: None,
            color: [200, 100, 50, 255],
            effect: None,
            stroke: None,
            text_align: None,
        };
        let prediction = FontPrediction {
            text_color: [12, 34, 56],
            ..Default::default()
        };
        let background = RgbaImage::from_pixel(32, 32, Rgba([255, 255, 255, 255]));
        assert_eq!(
            resolve_text_color(
                &explicit,
                Some(&prediction),
                &background,
                LayoutBox {
                    x: 0.0,
                    y: 0.0,
                    width: 32.0,
                    height: 32.0
                },
                None,
                None,
            ),
            [200, 100, 50, 255]
        );
    }

    #[test]
    fn stale_predicted_style_color_is_treated_as_auto() {
        let style = TextStyle {
            font_families: Vec::new(),
            font_size: Some(24.0),
            color: [12, 34, 56, 255],
            effect: None,
            stroke: None,
            text_align: None,
        };
        let prediction = FontPrediction {
            text_color: [12, 34, 56],
            ..Default::default()
        };
        let background = RgbaImage::from_pixel(32, 32, Rgba([20, 20, 20, 255]));
        assert_eq!(
            resolve_text_color(
                &style,
                Some(&prediction),
                &background,
                LayoutBox {
                    x: 0.0,
                    y: 0.0,
                    width: 32.0,
                    height: 32.0
                },
                None,
                None,
            ),
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn mask_collision_fit_renders_min_size_when_no_safe_size_exists() -> Result<()> {
        let font = any_system_font();
        let layout_builder = TextLayout::new(&font, None);
        let layout_box = LayoutBox {
            x: 0.0,
            y: 0.0,
            width: 24.0,
            height: 12.0,
        };
        let mask = GrayImage::from_pixel(64, 64, Luma([0u8]));
        let mut rendered_sizes = Vec::new();
        let mut render_candidate = |layout: &LayoutRun<'_>| -> Result<RenderedTextCandidate> {
            rendered_sizes.push(layout.font_size);
            let width = layout.width.ceil().max(1.0) as u32;
            let height = layout.height.ceil().max(1.0) as u32;
            Ok(RenderedTextCandidate {
                image: RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 255])),
                font_size: layout.font_size,
                transform: Transform {
                    x: 0.0,
                    y: 0.0,
                    width: width as f32,
                    height: height as f32,
                    rotation_deg: 0.0,
                },
            })
        };

        let candidate = fit_rendered_with_mask_collision(
            &layout_builder,
            "overflowing text",
            layout_box,
            None,
            12.0,
            18.0,
            &mask,
            1,
            &mut render_candidate,
        )?;

        // Every size collides (the mask has no bubble-1 pixels), so the
        // readability-floor candidate is returned as the least-bad option.
        assert!(rendered_sizes.contains(&12.0));
        assert_eq!(candidate.font_size, 12.0);
        assert!(candidate.image.width() >= 1);
        assert!(candidate.image.height() >= 1);
        Ok(())
    }

    #[test]
    fn auto_fit_shrinks_below_readability_floor_instead_of_overflowing() -> Result<()> {
        let font = any_system_font();
        let layout_builder = TextLayout::new(&font, None).without_hyphenation();
        // A box far too small for the floor size: the fit must drop below the
        // floor until the text fits rather than spilling out of the box.
        let (constraint_w, constraint_h) = (48.0, 30.0);
        let layout = fit_font_size(
            &layout_builder,
            "overflowing text",
            constraint_w,
            constraint_h,
            None,
            12.0,
            18.0,
        )?;
        assert!(
            layout.font_size < 12.0,
            "expected a below-floor font size, got {}",
            layout.font_size
        );
        assert!(layout.width <= constraint_w && layout.height <= constraint_h);
        Ok(())
    }

    #[test]
    fn shared_bubble_keeps_seed_boxes_to_avoid_overlap() {
        let mut mask = GrayImage::from_pixel(200, 200, Luma([0u8]));
        paint_rect(&mut mask, 10, 10, 190, 190, 1);
        let index = BubbleIndex::new(mask);
        let blocks = vec![
            block(30.0, 30.0, 40.0, 80.0, "hello"),
            block(120.0, 30.0, 40.0, 80.0, "world"),
        ];

        let layout_boxes = resolve_layout_boxes(&blocks, Some(&index));

        assert_eq!(layout_boxes[0].layout_box, seed_layout_box(&blocks[0]));
        assert_eq!(layout_boxes[0].bubble_id, Some(1));
        assert_eq!(layout_boxes[1].layout_box, seed_layout_box(&blocks[1]));
        assert_eq!(layout_boxes[1].bubble_id, Some(1));
    }

    #[test]
    fn single_block_can_still_expand_into_its_bubble() {
        let mut mask = GrayImage::from_pixel(200, 200, Luma([0u8]));
        paint_rect(&mut mask, 20, 20, 180, 180, 1);
        let index = BubbleIndex::new(mask);
        let blocks = vec![block(70.0, 70.0, 20.0, 30.0, "hello")];

        let layout_boxes = resolve_layout_boxes(&blocks, Some(&index));

        assert!(layout_boxes[0].layout_box.width > blocks[0].transform.width);
        assert!(layout_boxes[0].layout_box.height > blocks[0].transform.height);
        assert_eq!(layout_boxes[0].bubble_id, Some(1));
    }

    #[test]
    fn locked_block_keeps_manual_layout_box_inside_bubble() {
        let mut mask = GrayImage::from_pixel(200, 200, Luma([0u8]));
        paint_rect(&mut mask, 20, 20, 180, 180, 1);
        let index = BubbleIndex::new(mask);
        let mut locked = block(70.0, 70.0, 20.0, 30.0, "hello");
        locked.lock_layout_box = true;
        let blocks = vec![locked];

        let layout_boxes = resolve_layout_boxes(&blocks, Some(&index));

        assert_eq!(layout_boxes[0].layout_box, seed_layout_box(&blocks[0]));
        assert_eq!(layout_boxes[0].bubble_id, None);
    }

    #[test]
    fn locked_block_still_occupies_shared_bubble_so_neighbour_keeps_seed_box() {
        // Resizing a block locks it; the unlocked neighbour in the same
        // bubble must NOT become the "sole occupant" and expand over it.
        let mut mask = GrayImage::from_pixel(200, 200, Luma([0u8]));
        paint_rect(&mut mask, 10, 10, 190, 190, 1);
        let mut locked = block(30.0, 30.0, 40.0, 80.0, "hello");
        locked.lock_layout_box = true;
        let neighbour = block(120.0, 30.0, 40.0, 80.0, "world");
        let index = BubbleIndex::new(mask);
        let blocks = vec![locked, neighbour];

        let layout_boxes = resolve_layout_boxes(&blocks, Some(&index));

        // Locked box: unchanged, opts out of bubble handling entirely.
        assert_eq!(layout_boxes[0].layout_box, seed_layout_box(&blocks[0]));
        assert_eq!(layout_boxes[0].bubble_id, None);
        // Neighbour: keeps its own detector box instead of the bubble area.
        assert_eq!(layout_boxes[1].layout_box, seed_layout_box(&blocks[1]));
        assert_eq!(layout_boxes[1].bubble_id, Some(1));
    }

    #[test]
    fn mask_collision_detects_alpha_outside_matched_bubble() {
        let mut mask = GrayImage::from_pixel(10, 10, Luma([0u8]));
        paint_rect(&mut mask, 2, 2, 8, 8, 1);
        let sprite = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));

        let inside = Transform {
            x: 3.0,
            y: 3.0,
            width: 4.0,
            height: 4.0,
            rotation_deg: 0.0,
        };
        assert!(!sprite_collides_with_bubble_mask(
            &sprite, &inside, &mask, 1
        ));

        let outside = Transform {
            x: 0.0,
            y: 0.0,
            width: 4.0,
            height: 4.0,
            rotation_deg: 0.0,
        };
        assert!(sprite_collides_with_bubble_mask(
            &sprite, &outside, &mask, 1
        ));
    }

    #[test]
    fn mask_collision_ignores_transparent_sprite_pixels() {
        let mask = GrayImage::from_pixel(4, 4, Luma([0u8]));
        let sprite = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
        let transform = Transform {
            x: 0.0,
            y: 0.0,
            width: 4.0,
            height: 4.0,
            rotation_deg: 0.0,
        };

        assert!(!sprite_collides_with_bubble_mask(
            &sprite, &transform, &mask, 1
        ));
    }

    fn block(x: f32, y: f32, width: f32, height: f32, translation: &str) -> RenderBlockInput {
        RenderBlockInput {
            node_id: NodeId::new(),
            transform: Transform {
                x,
                y,
                width,
                height,
                rotation_deg: 0.0,
            },
            translation: translation.to_string(),
            style: None,
            font_prediction: None,
            source_direction: None,
            rendered_direction: None,
            lock_layout_box: false,
        }
    }

    fn paint_rect(img: &mut GrayImage, x0: u32, y0: u32, x1: u32, y1: u32, value: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, Luma([value]));
            }
        }
    }

    fn any_system_font() -> Font {
        let mut book = FontBook::new();
        let preferred = [
            "Yu Gothic",
            "MS Gothic",
            "Noto Sans CJK JP",
            "Noto Sans",
            "Arial",
            "DejaVu Sans",
            "Liberation Sans",
        ];

        for name in preferred {
            if let Some(post_script_name) = book
                .all_families()
                .into_iter()
                .find(|face| {
                    face.post_script_name == name
                        || face
                            .families
                            .iter()
                            .any(|(family, _)| family.as_str() == name)
                })
                .map(|face| face.post_script_name)
                .filter(|post_script_name| !post_script_name.is_empty())
                && let Ok(font) = book.query(&post_script_name)
            {
                return font;
            }
        }

        if let Some(face) = book
            .all_families()
            .into_iter()
            .find(|face| !face.post_script_name.is_empty())
        {
            return book
                .query(&face.post_script_name)
                .expect("failed to load first system font");
        }

        panic!("no system font available for tests");
    }

    #[test]
    fn inset_layout_box_shrinks_symmetrically_and_keeps_centre() {
        let b = LayoutBox {
            x: 100.0,
            y: 100.0,
            width: 200.0,
            height: 100.0,
        };
        let inset = inset_layout_box(b, 10.0);
        assert_eq!(inset.x, 110.0);
        assert_eq!(inset.y, 110.0);
        assert_eq!(inset.width, 180.0);
        assert_eq!(inset.height, 80.0);
        // Centre is preserved so sprite centring is unaffected.
        assert_eq!(b.x + b.width * 0.5, inset.x + inset.width * 0.5);
        assert_eq!(b.y + b.height * 0.5, inset.y + inset.height * 0.5);
    }

    #[test]
    fn inset_layout_box_never_collapses_below_minimum() {
        let b = LayoutBox {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 6.0,
        };
        let inset = inset_layout_box(b, 1000.0);
        assert!(inset.width >= 1.0);
        assert!(inset.height >= 1.0);
    }

    #[test]
    fn inset_layout_box_is_noop_for_zero_padding() {
        let b = LayoutBox {
            x: 5.0,
            y: 7.0,
            width: 20.0,
            height: 30.0,
        };
        assert_eq!(inset_layout_box(b, 0.0), b);
    }

    #[test]
    fn stroke_clearance_reserves_full_width_plus_antialias() {
        let stroke = RenderStrokeOptions {
            color: [255, 255, 255, 255],
            width_px: 4.2,
        };
        // Full stroke width (ceiled) + 1px AA: outline paints outward past the
        // glyph fill, so the sprite canvas needs at least this much padding.
        assert_eq!(stroke_clearance(Some(&stroke)), 6.0);
    }

    #[test]
    fn stroke_clearance_is_zero_without_stroke() {
        assert_eq!(stroke_clearance(None), 0.0);
        // Degenerate widths never produce negative clearance.
        let stroke = RenderStrokeOptions {
            color: [0, 0, 0, 255],
            width_px: -3.0,
        };
        assert_eq!(stroke_clearance(Some(&stroke)), 1.0);
    }

    #[test]
    fn stroke_clearance_fits_padded_sprite_inside_layout_box() {
        // Fill constrained to the inset box + canvas padded by the clearance
        // must never exceed the original layout box on either axis.
        let layout_box = LayoutBox {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 120.0,
        };
        let stroke = RenderStrokeOptions {
            color: [255, 255, 255, 255],
            width_px: 5.0,
        };
        let clearance = stroke_clearance(Some(&stroke));
        let fit_box = inset_layout_box(layout_box, clearance);
        assert!(fit_box.width + 2.0 * clearance <= layout_box.width);
        assert!(fit_box.height + 2.0 * clearance <= layout_box.height);
        // Symmetric inset keeps the centre (and thus sprite centring).
        assert_eq!(
            fit_box.x + fit_box.width / 2.0,
            layout_box.x + layout_box.width / 2.0
        );
        assert_eq!(
            fit_box.y + fit_box.height / 2.0,
            layout_box.y + layout_box.height / 2.0
        );
    }

    #[test]
    fn manual_boxes_allow_smaller_font_than_auto_floor() {
        // Auto-detected boxes keep the image readability floor.
        assert_eq!(effective_min_font_size(18.0, false), 18.0);
        // Manually drawn / resized boxes may shrink text below it so it fits
        // the box the user drew (issue #223).
        assert_eq!(effective_min_font_size(18.0, true), MANUAL_MIN_FONT_SIZE);
        // Never raises the floor above the image minimum.
        assert_eq!(effective_min_font_size(4.0, true), 4.0);
    }

    #[test]
    fn centred_sprite_transform_anchors_to_provided_box_center() {
        let anchor = LayoutBox {
            x: 100.0,
            y: 100.0,
            width: 200.0,
            height: 100.0,
        };
        let sprite_w = 100;
        let sprite_h = 50;

        let transform = centred_sprite_transform(anchor, sprite_w, sprite_h, 0.0);

        // Center of anchor is (200, 150).
        // Sprite (100x50) centered on (200, 150) starts at (150, 125).
        assert_eq!(transform.x, 150.0);
        assert_eq!(transform.y, 125.0);
    }
}
