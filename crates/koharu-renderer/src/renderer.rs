use std::{collections::HashMap, ops::Range};

use anyhow::{Context, Result, bail};
use image::{RgbaImage, imageops};
use skrifa::{
    GlyphId, MetadataProvider, OutlineGlyph,
    instance::Size,
    outline::{DrawSettings, OutlinePen},
};
use tiny_skia::{
    Color, FillRule, FilterQuality, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap,
    PixmapPaint, Stroke, Transform,
};

use crate::font::{Font, font_key};
use crate::layout::{LayoutLine, LayoutRun, WritingMode};

pub use crate::types::TextShaderEffect;

#[derive(Debug, Clone, Copy)]
pub struct RenderStrokeOptions {
    pub color: [u8; 4],
    pub width_px: f32,
}

/// Character-level overrides keyed by UTF-8 byte offsets in the laid-out
/// string. Later overlapping ranges win per property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderStyleRange {
    pub range: Range<usize>,
    pub color: Option<[u8; 4]>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DownsampleFilter {
    Nearest,
    Triangle,
    CatmullRom,
    Gaussian,
    #[default]
    Lanczos3,
}

impl From<DownsampleFilter> for imageops::FilterType {
    fn from(value: DownsampleFilter) -> Self {
        match value {
            DownsampleFilter::Nearest => imageops::FilterType::Nearest,
            DownsampleFilter::Triangle => imageops::FilterType::Triangle,
            DownsampleFilter::CatmullRom => imageops::FilterType::CatmullRom,
            DownsampleFilter::Gaussian => imageops::FilterType::Gaussian,
            DownsampleFilter::Lanczos3 => imageops::FilterType::Lanczos3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterOptions {
    pub supersampling_factor: u32,
    pub downsample_filter: DownsampleFilter,
}

impl RasterOptions {
    pub fn supersampled(factor: u32) -> Self {
        Self {
            supersampling_factor: factor,
            ..Default::default()
        }
    }

    fn scale(self) -> u32 {
        self.supersampling_factor.clamp(2, MAX_SUPERSAMPLING_FACTOR)
    }
}

impl Default for RasterOptions {
    fn default() -> Self {
        Self {
            supersampling_factor: 2,
            downsample_filter: DownsampleFilter::Lanczos3,
        }
    }
}

/// Options for rendering text.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub color: [u8; 4],
    pub background: Option<[u8; 4]>,
    pub anti_alias: bool,
    pub padding: f32,
    pub font_size: f32,
    pub effect: TextShaderEffect,
    pub stroke: Option<RenderStrokeOptions>,
    pub style_ranges: Vec<RenderStyleRange>,
    pub raster: RasterOptions,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            color: [0, 0, 0, 255],
            background: None,
            anti_alias: true,
            padding: 0.0,
            font_size: 16.0,
            effect: TextShaderEffect::default(),
            stroke: None,
            style_ranges: Vec::new(),
            raster: RasterOptions::default(),
        }
    }
}

const MAX_SUPERSAMPLING_FACTOR: u32 = 4;

pub struct TinySkiaRenderer;

impl TinySkiaRenderer {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Additional canvas clearance for synthetic effects, beyond caller-supplied
    /// stroke padding. Layout bounds describe the unmodified glyph outlines.
    pub fn effect_padding(&self, layout: &LayoutRun<'_>, opts: &RenderOptions) -> Result<f32> {
        let mut clearance = 0.0f32;
        let mut cache = HashMap::new();
        for glyph in layout.lines.iter().flat_map(|line| &line.glyphs) {
            let effect = resolve_glyph_style(opts, glyph.cluster as usize).effect;
            if !effect.bold && !effect.italic {
                continue;
            }
            let Ok(gid) = u16::try_from(glyph.glyph_id) else {
                continue;
            };
            let key = FontGlyphId {
                font: font_key(glyph.font),
                glyph: gid,
            };
            if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
                entry.insert(load_glyph_source(
                    glyph.font,
                    gid,
                    opts.font_size,
                    opts.anti_alias,
                )?);
            }
            let (width, height) = match &cache[&key] {
                GlyphRenderSource::Outline(data) => (data.bounds.width(), data.bounds.height()),
                GlyphRenderSource::Bitmap(data) => {
                    (data.metrics.width as f32, data.metrics.height as f32)
                }
            };
            let slant = if effect.italic {
                (width.min(height) * 0.22).max(1.0)
            } else {
                0.0
            };
            // Bold expands by one pixel; reserve another for antialiasing.
            clearance = clearance.max(slant + if effect.bold { 1.0 } else { 0.0 } + 1.0);
        }
        Ok(clearance.ceil())
    }

    pub fn render(
        &self,
        layout: &LayoutRun<'_>,
        writing_mode: WritingMode,
        opts: &RenderOptions,
    ) -> Result<RgbaImage> {
        let mut padded_opts = opts.clone();
        padded_opts.padding += self.effect_padding(layout, opts)?;
        let opts = &padded_opts;
        let width = (layout.width + opts.padding * 2.0).ceil() as u32;
        let height = (layout.height + opts.padding * 2.0).ceil() as u32;
        if width == 0 || height == 0 {
            bail!("invalid surface size {width}x{height}");
        }
        let raster_scale = opts.raster.scale();
        let raster_width = width
            .checked_mul(raster_scale)
            .context("supersampled render surface width overflow")?;
        let raster_height = height
            .checked_mul(raster_scale)
            .context("supersampled render surface height overflow")?;
        let raster_scale_f = raster_scale as f32;

        let mut surface = Pixmap::new(raster_width, raster_height)
            .context("failed to allocate render surface")?;
        if let Some(bg) = opts.background {
            surface.fill(color_from_rgba(bg));
        }

        let mut cache: HashMap<FontGlyphId, GlyphRenderSource> = HashMap::new();
        let has_stroke = opts
            .stroke
            .is_some_and(|stroke| stroke.width_px > 0.0 && stroke.color[3] > 0);
        if has_stroke {
            render_pass(
                &mut surface,
                &mut cache,
                layout,
                writing_mode,
                opts,
                RenderPass::Stroke,
                raster_scale_f,
            )?;
        }
        render_pass(
            &mut surface,
            &mut cache,
            layout,
            writing_mode,
            opts,
            RenderPass::Fill,
            raster_scale_f,
        )?;

        surface_to_image(surface, width, height, opts.raster.downsample_filter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FontGlyphId {
    font: usize,
    glyph: u16,
}

#[derive(Clone, Copy, Debug)]
struct GlyphMetrics {
    width: u32,
    height: u32,
    xmin: i32,
    ymin: i32,
}

enum GlyphRenderSource {
    Outline(OutlineGlyphData),
    Bitmap(BitmapGlyphData),
}

struct OutlineGlyphData {
    path: Path,
    bounds: tiny_skia::Rect,
}

struct BitmapGlyphData {
    metrics: GlyphMetrics,
    fill_alpha: Vec<u8>,
}

#[derive(Clone, Copy)]
enum RenderPass {
    Stroke,
    Fill,
}

#[derive(Clone, Copy)]
struct ResolvedGlyphStyle {
    color: [u8; 4],
    effect: TextShaderEffect,
}

fn resolve_glyph_style(opts: &RenderOptions, cluster: usize) -> ResolvedGlyphStyle {
    let mut style = ResolvedGlyphStyle {
        color: opts.color,
        effect: opts.effect,
    };
    for range in &opts.style_ranges {
        if !range.range.contains(&cluster) {
            continue;
        }
        if let Some(color) = range.color {
            style.color = color;
        }
        if let Some(bold) = range.bold {
            style.effect.bold = bold;
        }
        if let Some(italic) = range.italic {
            style.effect.italic = italic;
        }
    }
    style
}

fn render_pass(
    surface: &mut Pixmap,
    cache: &mut HashMap<FontGlyphId, GlyphRenderSource>,
    layout: &LayoutRun<'_>,
    writing_mode: WritingMode,
    opts: &RenderOptions,
    pass: RenderPass,
    raster_scale: f32,
) -> Result<()> {
    for line in &layout.lines {
        let origin = match writing_mode {
            WritingMode::Horizontal | WritingMode::VerticalRl => (
                (opts.padding + line.baseline.0) * raster_scale,
                (opts.padding + line.baseline.1) * raster_scale,
            ),
        };
        render_line(surface, cache, line, origin, opts, pass, raster_scale)?;
    }

    Ok(())
}

fn render_line(
    surface: &mut Pixmap,
    cache: &mut HashMap<FontGlyphId, GlyphRenderSource>,
    line: &LayoutLine<'_>,
    origin: (f32, f32),
    opts: &RenderOptions,
    pass: RenderPass,
    raster_scale: f32,
) -> Result<()> {
    let (origin_x, origin_y) = origin;
    let mut pen_x = 0.0f32;
    let mut pen_y = 0.0f32;

    for glyph in &line.glyphs {
        let glyph_style = resolve_glyph_style(opts, glyph.cluster as usize);
        let Ok(gid) = u16::try_from(glyph.glyph_id) else {
            pen_x += glyph.x_advance;
            pen_y -= glyph.y_advance;
            continue;
        };

        let key = FontGlyphId {
            font: font_key(glyph.font),
            glyph: gid,
        };
        if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(key) {
            let source = load_glyph_source(
                glyph.font,
                gid,
                opts.font_size * raster_scale,
                opts.anti_alias,
            )?;
            e.insert(source);
        }

        let baseline_x = origin_x + (pen_x + glyph.x_offset) * raster_scale;
        let baseline_y = origin_y + (pen_y - glyph.y_offset) * raster_scale;

        if let Some(source) = cache.get(&key) {
            match source {
                GlyphRenderSource::Outline(data) => {
                    draw_outline_glyph(
                        surface,
                        data,
                        baseline_x,
                        baseline_y,
                        opts,
                        glyph_style,
                        pass,
                        raster_scale,
                    );
                }
                GlyphRenderSource::Bitmap(data) => {
                    draw_bitmap_glyph(
                        surface,
                        data,
                        baseline_x,
                        baseline_y,
                        opts,
                        glyph_style,
                        pass,
                        raster_scale,
                    )?;
                }
            }
        }

        pen_x += glyph.x_advance;
        pen_y -= glyph.y_advance;
    }

    Ok(())
}

fn load_glyph_source(
    font: &Font,
    glyph_id: u16,
    font_size: f32,
    anti_alias: bool,
) -> Result<GlyphRenderSource> {
    let font_ref = font.skrifa()?;

    // Support variable font weights by instancing the wght axis
    let mut location = skrifa::instance::Location::default();
    let axes = font_ref.axes();
    if let Some(axis) = axes.iter().find(|a| a.tag() == skrifa::Tag::new(b"wght")) {
        let target = (font.weight as f32).clamp(axis.min_value(), axis.max_value());
        location = axes.location([(skrifa::Tag::new(b"wght"), target)]);
    }

    if let Some(outline) = font_ref.outline_glyphs().get(GlyphId::new(glyph_id as u32))
        && let Some(path) = outline_to_path(&outline, font_size, &location)
    {
        let bounds = path.bounds();
        if bounds.width() > 0.0 && bounds.height() > 0.0 {
            return Ok(GlyphRenderSource::Outline(OutlineGlyphData {
                path,
                bounds,
            }));
        }
    }

    let fontdue = font.fontdue()?;
    let (metrics, mut bitmap) = fontdue.rasterize_indexed(glyph_id, font_size);
    if !anti_alias {
        for px in &mut bitmap {
            *px = if *px >= 128 { 255 } else { 0 };
        }
    }

    Ok(GlyphRenderSource::Bitmap(BitmapGlyphData {
        metrics: GlyphMetrics {
            width: metrics.width as u32,
            height: metrics.height as u32,
            xmin: metrics.xmin,
            ymin: metrics.ymin,
        },
        fill_alpha: bitmap,
    }))
}

fn draw_outline_glyph(
    surface: &mut Pixmap,
    glyph: &OutlineGlyphData,
    baseline_x: f32,
    baseline_y: f32,
    opts: &RenderOptions,
    glyph_style: ResolvedGlyphStyle,
    pass: RenderPass,
    raster_scale: f32,
) {
    let transform = glyph_transform(
        glyph.bounds,
        baseline_x,
        baseline_y,
        glyph_style.effect.italic,
    );

    match pass {
        RenderPass::Stroke => {
            if let Some(stroke) = opts
                .stroke
                .filter(|stroke| stroke.width_px > 0.0 && stroke.color[3] > 0)
            {
                let stroke_paint = paint_from_rgba(stroke.color, opts.anti_alias);
                let stroke_style = Stroke {
                    width: stroke.width_px.max(0.0) * raster_scale * 2.0,
                    line_join: LineJoin::Round,
                    line_cap: LineCap::Round,
                    ..Default::default()
                };
                surface.stroke_path(&glyph.path, &stroke_paint, &stroke_style, transform, None);
            }
        }
        RenderPass::Fill => {
            if glyph_style.effect.bold {
                let bold_paint = paint_from_rgba(glyph_style.color, opts.anti_alias);
                let bold_style = Stroke {
                    width: 2.0 * raster_scale,
                    line_join: LineJoin::Round,
                    line_cap: LineCap::Round,
                    ..Default::default()
                };
                surface.stroke_path(&glyph.path, &bold_paint, &bold_style, transform, None);
            }

            let fill_paint = paint_from_rgba(glyph_style.color, opts.anti_alias);
            surface.fill_path(&glyph.path, &fill_paint, FillRule::Winding, transform, None);
        }
    }
}

fn draw_bitmap_glyph(
    surface: &mut Pixmap,
    glyph: &BitmapGlyphData,
    baseline_x: f32,
    baseline_y: f32,
    opts: &RenderOptions,
    glyph_style: ResolvedGlyphStyle,
    pass: RenderPass,
    raster_scale: f32,
) -> Result<()> {
    if glyph.metrics.width == 0 || glyph.metrics.height == 0 || glyph.fill_alpha.is_empty() {
        return Ok(());
    }

    let width = glyph.metrics.width as usize;
    let height = glyph.metrics.height as usize;
    let mut fill_alpha = glyph.fill_alpha.clone();
    if glyph_style.effect.bold {
        fill_alpha = dilate_alpha(
            &fill_alpha,
            width,
            height,
            scaled_pixel_radius(1.0, raster_scale),
        );
    }

    let x = baseline_x + glyph.metrics.xmin as f32;
    let y = baseline_y - glyph.metrics.ymin as f32 - glyph.metrics.height as f32;
    let transform = bitmap_transform(
        glyph.metrics.width as f32,
        glyph.metrics.height as f32,
        x,
        y,
        glyph_style.effect.italic,
    );
    let paint = pixmap_paint(glyph_style.effect.italic, opts.anti_alias);

    match pass {
        RenderPass::Stroke => {
            if let Some(stroke) = opts
                .stroke
                .filter(|stroke| stroke.width_px > 0.0 && stroke.color[3] > 0)
            {
                let radius = scaled_pixel_radius(stroke.width_px, raster_scale);
                let outer = dilate_alpha(&fill_alpha, width, height, radius);
                let stroke_alpha = outer
                    .into_iter()
                    .zip(&fill_alpha)
                    .map(|(outer_alpha, fill)| outer_alpha.saturating_sub(*fill))
                    .collect::<Vec<_>>();
                if let Some(stroke_pixmap) = alpha_pixmap(
                    glyph.metrics.width,
                    glyph.metrics.height,
                    &stroke_alpha,
                    stroke.color,
                ) {
                    surface.draw_pixmap(0, 0, stroke_pixmap.as_ref(), &paint, transform, None);
                }
            }
        }
        RenderPass::Fill => {
            if let Some(fill_pixmap) = alpha_pixmap(
                glyph.metrics.width,
                glyph.metrics.height,
                &fill_alpha,
                glyph_style.color,
            ) {
                surface.draw_pixmap(0, 0, fill_pixmap.as_ref(), &paint, transform, None);
            }
        }
    }

    Ok(())
}

fn glyph_transform(
    bounds: tiny_skia::Rect,
    baseline_x: f32,
    baseline_y: f32,
    italic: bool,
) -> Transform {
    if !italic {
        return Transform::from_translate(baseline_x, baseline_y);
    }

    let glyph_w = bounds.width().max(1.0);
    let glyph_h = bounds.height().max(1.0);
    let slant = (glyph_w.min(glyph_h) * 0.22).max(1.0);
    let kx = -slant / glyph_h;
    Transform::from_row(
        1.0,
        0.0,
        kx,
        1.0,
        baseline_x - kx * bounds.bottom(),
        baseline_y,
    )
}

fn bitmap_transform(width: f32, height: f32, x: f32, y: f32, italic: bool) -> Transform {
    if !italic {
        return Transform::from_translate(x, y);
    }

    let glyph_w = width.max(1.0);
    let glyph_h = height.max(1.0);
    let slant = (glyph_w.min(glyph_h) * 0.22).max(1.0);
    let kx = -slant / glyph_h;
    Transform::from_row(1.0, 0.0, kx, 1.0, x - kx * glyph_h, y)
}

fn pixmap_paint(italic: bool, anti_alias: bool) -> PixmapPaint {
    PixmapPaint {
        quality: if italic && anti_alias {
            FilterQuality::Bilinear
        } else {
            FilterQuality::Nearest
        },
        ..Default::default()
    }
}

fn surface_to_image(
    surface: Pixmap,
    width: u32,
    height: u32,
    downsample_filter: DownsampleFilter,
) -> Result<RgbaImage> {
    let raster_width = surface.width();
    let raster_height = surface.height();
    let pixels = surface.data().to_vec();

    let raster_img = RgbaImage::from_raw(raster_width, raster_height, pixels)
        .context("failed to build supersampled RgbaImage")?;
    let img = imageops::resize(&raster_img, width, height, downsample_filter.into());
    let mut pixels = img.into_raw();
    unpremultiply_rgba(&mut pixels);
    RgbaImage::from_raw(width, height, pixels).context("failed to build downsampled RgbaImage")
}

fn scaled_pixel_radius(logical_radius: f32, raster_scale: f32) -> usize {
    (logical_radius.max(0.0) * raster_scale).ceil().max(1.0) as usize
}

fn paint_from_rgba(color: [u8; 4], anti_alias: bool) -> Paint<'static> {
    let mut paint = Paint {
        anti_alias,
        ..Default::default()
    };
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint
}

fn color_from_rgba(color: [u8; 4]) -> Color {
    Color::from_rgba8(color[0], color[1], color[2], color[3])
}

fn outline_to_path(
    outline: &OutlineGlyph<'_>,
    font_size: f32,
    location: &skrifa::instance::Location,
) -> Option<Path> {
    let mut pen = TinySkiaPathPen::new();
    let settings = DrawSettings::unhinted(Size::new(font_size), location);
    outline.draw(settings, &mut pen).ok()?;
    pen.finish()
}

fn alpha_pixmap(width: u32, height: u32, alpha: &[u8], color: [u8; 4]) -> Option<Pixmap> {
    if color[3] == 0 || width == 0 || height == 0 || alpha.is_empty() {
        return None;
    }

    let mut pixmap = Pixmap::new(width, height)?;
    let data = pixmap.data_mut();
    for (index, &mask_alpha) in alpha.iter().enumerate() {
        let out_alpha = ((mask_alpha as u32 * color[3] as u32) + 127) / 255;
        let offset = index * 4;
        data[offset] = (((color[0] as u32 * out_alpha) + 127) / 255) as u8;
        data[offset + 1] = (((color[1] as u32 * out_alpha) + 127) / 255) as u8;
        data[offset + 2] = (((color[2] as u32 * out_alpha) + 127) / 255) as u8;
        data[offset + 3] = out_alpha as u8;
    }
    Some(pixmap)
}

fn dilate_alpha(alpha: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    if radius == 0 || alpha.is_empty() {
        return alpha.to_vec();
    }

    let mut out = vec![0u8; alpha.len()];
    for y in 0..height {
        let y0 = y.saturating_sub(radius);
        let y1 = (y + radius).min(height.saturating_sub(1));
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius).min(width.saturating_sub(1));
            let mut max_alpha = 0u8;
            for yy in y0..=y1 {
                let row = yy * width;
                for xx in x0..=x1 {
                    max_alpha = max_alpha.max(alpha[row + xx]);
                }
            }
            out[y * width + x] = max_alpha;
        }
    }
    out
}

fn unpremultiply_rgba(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 || a == 255 {
            continue;
        }
        let alpha = a as u32;
        px[0] = ((px[0] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
        px[1] = ((px[1] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
        px[2] = ((px[2] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
    }
}

struct TinySkiaPathPen {
    builder: PathBuilder,
}

impl TinySkiaPathPen {
    fn new() -> Self {
        Self {
            builder: PathBuilder::new(),
        }
    }

    fn finish(self) -> Option<Path> {
        self.builder.finish()
    }
}

impl OutlinePen for TinySkiaPathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x, -y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(x, -y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder.quad_to(cx0, -cy0, x, -y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder.cubic_to(cx0, -cy0, cx1, -cy1, x, -y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        font::{Font, FontBook},
        layout::TextLayout,
    };

    fn any_system_font() -> Font {
        let mut book = FontBook::new();
        let preferred = [
            "Arial",
            "Segoe UI",
            "Yu Gothic",
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
    fn default_raster_options_use_2x_lanczos_supersampling() {
        let raster = RasterOptions::default();
        assert_eq!(raster.supersampling_factor, 2);
        assert_eq!(raster.downsample_filter, DownsampleFilter::Lanczos3);
        assert_eq!(raster.scale(), 2);
    }

    #[test]
    fn supersampling_factor_is_bounded() {
        assert_eq!(RasterOptions::supersampled(0).scale(), 2);
        assert_eq!(RasterOptions::supersampled(1).scale(), 2);
        assert_eq!(
            RasterOptions::supersampled(99).scale(),
            MAX_SUPERSAMPLING_FACTOR
        );
    }

    #[test]
    fn supersampled_render_keeps_logical_surface_dimensions() -> Result<()> {
        let font = any_system_font();
        let font_size = 24.0;
        let layout = TextLayout::new(&font, Some(font_size)).run("Hello")?;
        let renderer = TinySkiaRenderer::new()?;

        let default = renderer.render(
            &layout,
            WritingMode::Horizontal,
            &RenderOptions {
                font_size,
                ..Default::default()
            },
        )?;
        let higher_scale = renderer.render(
            &layout,
            WritingMode::Horizontal,
            &RenderOptions {
                font_size,
                raster: RasterOptions::supersampled(4),
                ..Default::default()
            },
        )?;

        assert_eq!(higher_scale.dimensions(), default.dimensions());
        assert!(default.pixels().any(|pixel| pixel.0[3] > 0));
        Ok(())
    }

    #[test]
    fn italic_glyphs_do_not_lose_ink_at_surface_edge() -> Result<()> {
        let font = any_system_font();
        let font_size = 64.0;
        let renderer = TinySkiaRenderer::new()?;
        for mode in [WritingMode::Horizontal, WritingMode::VerticalRl] {
            let layout = TextLayout::new(&font, Some(font_size))
                .with_writing_mode(mode)
                .run("H")?;
            for character_override in [false, true] {
                let opts = RenderOptions {
                    font_size,
                    effect: TextShaderEffect {
                        bold: false,
                        italic: !character_override,
                    },
                    style_ranges: if character_override {
                        vec![RenderStyleRange {
                            range: 0..1,
                            color: None,
                            bold: Some(true),
                            italic: Some(true),
                        }]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                };
                let tight = renderer.render(&layout, mode, &opts)?;
                let padded = renderer.render(
                    &layout,
                    mode,
                    &RenderOptions {
                        padding: 32.0,
                        ..opts
                    },
                )?;
                let ink = |img: &RgbaImage| img.pixels().map(|p| u64::from(p[3])).sum::<u64>();
                let (tight_ink, padded_ink) = (ink(&tight), ink(&padded));
                assert!(
                    tight_ink.abs_diff(padded_ink) < padded_ink / 100,
                    "tight={tight_ink}, padded={padded_ink}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn character_range_overrides_only_matching_glyph_clusters() {
        let opts = RenderOptions {
            color: [0, 0, 0, 255],
            effect: TextShaderEffect {
                bold: true,
                italic: false,
            },
            style_ranges: vec![RenderStyleRange {
                range: 1..4,
                color: Some([220, 30, 40, 255]),
                bold: Some(false),
                italic: Some(true),
            }],
            ..Default::default()
        };

        let inherited = resolve_glyph_style(&opts, 0);
        assert_eq!(inherited.color, [0, 0, 0, 255]);
        assert!(inherited.effect.bold);
        assert!(!inherited.effect.italic);

        let overridden = resolve_glyph_style(&opts, 2);
        assert_eq!(overridden.color, [220, 30, 40, 255]);
        assert!(!overridden.effect.bold);
        assert!(overridden.effect.italic);
    }

    #[test]
    fn renderer_paints_selected_character_in_its_own_color() -> Result<()> {
        let font = any_system_font();
        let font_size = 32.0;
        let layout = TextLayout::new(&font, Some(font_size)).run("AB")?;
        let image = TinySkiaRenderer::new()?.render(
            &layout,
            WritingMode::Horizontal,
            &RenderOptions {
                font_size,
                color: [0, 0, 0, 255],
                style_ranges: vec![RenderStyleRange {
                    range: 1..2,
                    color: Some([255, 0, 0, 255]),
                    bold: None,
                    italic: None,
                }],
                ..Default::default()
            },
        )?;

        assert!(
            image
                .pixels()
                .any(|pixel| pixel.0[3] > 0 && pixel.0[0] == 0)
        );
        assert!(
            image
                .pixels()
                .any(|pixel| pixel.0[3] > 0 && pixel.0[0] > 200 && pixel.0[1] < 20)
        );
        Ok(())
    }

    #[test]
    fn vertical_emphasis_pair_keeps_style_range_on_original_character() -> Result<()> {
        // Regression for the vertical punctuation/style cluster mismatch:
        // "!!" collapses into a single "‼" glyph (2 bytes -> 3 bytes), so
        // without cluster remapping "B" would sit at normalized byte 3 while
        // the caller's style range (original coordinates) targets byte 2. The
        // remap keeps clusters in original coordinates, so colouring byte range
        // 2..3 must paint "B" red and leave the rest of the block black.
        let font = any_system_font();
        let font_size = 32.0;
        let layout = TextLayout::new(&font, Some(font_size))
            .with_writing_mode(WritingMode::VerticalRl)
            .run("!!B")?;
        let image = TinySkiaRenderer::new()?.render(
            &layout,
            WritingMode::VerticalRl,
            &RenderOptions {
                font_size,
                color: [0, 0, 0, 255],
                style_ranges: vec![RenderStyleRange {
                    range: 2..3, // "B" in ORIGINAL byte coordinates
                    color: Some([255, 0, 0, 255]),
                    bold: None,
                    italic: None,
                }],
                ..Default::default()
            },
        )?;

        assert!(
            image.pixels().any(|pixel| pixel.0[3] > 0
                && pixel.0[0] > 200
                && pixel.0[1] < 20
                && pixel.0[2] < 20),
            "expected the trailing character to be painted red via its original-coordinate style range"
        );
        Ok(())
    }
}
