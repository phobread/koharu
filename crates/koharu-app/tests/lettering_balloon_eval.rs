//! Opt-in lettering evaluation: renders pages through the real renderer from
//! a job file, so bubble-shaped layout can be compared with other renders.
//!
//! `KOHARU_LETTERING_JOBS` names a JSON array of jobs:
//! `{ "cleaned": path, "bubble": path, "out": path,
//!    "nodes": [{ "id", "transform", "text": TextData }] }`.
//! Renders use the editor defaults (CCWildWordsRoman, box padding 0, 8 px
//! outline) and the fonts in the normal app data folder. Prints, per page,
//! how many blocks were laid out in their bubble rather than their box.

use anyhow::{Context, Result};
use koharu_app::renderer::{PageRenderOptions, RenderBlockInput, Renderer};
use koharu_core::{NodeId, TextData, TextStrokeStyle, Transform};
use serde::Deserialize;

#[derive(Deserialize)]
struct Job {
    cleaned: String,
    bubble: String,
    out: String,
    nodes: Vec<JobNode>,
}

#[derive(Deserialize)]
struct JobNode {
    id: NodeId,
    transform: Transform,
    text: TextData,
}

#[test]
#[ignore = "needs KOHARU_LETTERING_JOBS and local fonts"]
fn lettering_balloon_eval() -> Result<()> {
    let path = std::env::var("KOHARU_LETTERING_JOBS").context("KOHARU_LETTERING_JOBS")?;
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
    let renderer = Renderer::new()?;
    let options = PageRenderOptions {
        shader_stroke: Some(TextStrokeStyle {
            enabled: true,
            color: None,
            width_px: Some(8.0),
        }),
        document_font: Some("CCWildWordsRoman".to_owned()),
        box_padding: 0.0,
        ..Default::default()
    };
    for job in jobs {
        let cleaned = image::open(&job.cleaned)?;
        let bubble = image::open(&job.bubble)?;
        let inputs = job
            .nodes
            .iter()
            .filter_map(|node| {
                let translation = node.text.translation.clone()?;
                (!translation.trim().is_empty()).then(|| RenderBlockInput {
                    node_id: node.id,
                    transform: node.transform,
                    translation,
                    style: node.text.style.clone(),
                    style_ranges: node.text.style_ranges.clone(),
                    font_prediction: node.text.font_prediction.clone(),
                    source_direction: node.text.source_direction,
                    rendered_direction: node.text.rendered_direction,
                    writing_direction: node.text.writing_direction,
                    lock_layout_box: node.text.lock_layout_box,
                })
            })
            .collect::<Vec<_>>();
        // KOHARU_LETTERING_NO_BUBBLES renders without the bubble mask: every
        // block then uses its box, as before bubble-shaped lettering.
        let use_bubbles = std::env::var_os("KOHARU_LETTERING_NO_BUBBLES").is_none();
        let started = std::time::Instant::now();
        let output = renderer.render_page(
            &cleaned,
            None,
            use_bubbles.then_some(&bubble),
            cleaned.width(),
            cleaned.height(),
            &inputs,
            &options,
        )?;
        let elapsed = started.elapsed();
        output.final_render.save(&job.out)?;
        if std::env::var_os("KOHARU_LETTERING_SHAPES").is_some() {
            save_shapes(&job, &cleaned, &bubble, &inputs)?;
        }
        // A box-fitted sprite is centred on its box; a bubble layout isn't.
        let in_bubble = output
            .blocks
            .iter()
            .filter(|block| {
                let input = inputs.iter().find(|i| i.node_id == block.node_id).unwrap();
                block.expanded_transform.is_some_and(|t| {
                    let dx =
                        (t.x + t.width * 0.5) - (input.transform.x + input.transform.width * 0.5);
                    let dy =
                        (t.y + t.height * 0.5) - (input.transform.y + input.transform.height * 0.5);
                    dx.abs() > 1.5 || dy.abs() > 1.5
                })
            })
            .count();
        for block in &output.blocks {
            println!("BLOCK {} {:.1}", block.node_id, block.font_size);
        }
        println!(
            "{} blocks={} in_bubble={} {:.0}ms",
            job.out,
            output.blocks.len(),
            in_bubble,
            elapsed.as_secs_f64() * 1000.0
        );
    }
    Ok(())
}

/// Debug view (`<out>.shapes.png`): each block's balloon rows tinted over
/// the page and its box outlined, grouped the way the renderer groups them.
fn save_shapes(
    job: &Job,
    cleaned: &image::DynamicImage,
    bubble: &image::DynamicImage,
    inputs: &[RenderBlockInput],
) -> Result<()> {
    use koharu_renderer::text::latin::{BubbleIndex, LayoutBox};
    let index = BubbleIndex::new(bubble.to_luma8());
    let boxes = inputs
        .iter()
        .map(|i| LayoutBox {
            x: i.transform.x,
            y: i.transform.y,
            width: i.transform.width.max(1.0),
            height: i.transform.height.max(1.0),
        })
        .collect::<Vec<_>>();
    let mut groups = std::collections::BTreeMap::<u8, Vec<usize>>::new();
    for (i, seed) in boxes.iter().enumerate() {
        if let Some(id) = index.confident_match(*seed) {
            groups.entry(id).or_default().push(i);
        }
    }
    let mut canvas = cleaned.to_rgba8();
    let colours = [
        [255, 60, 60],
        [60, 200, 60],
        [60, 120, 255],
        [230, 180, 0],
        [200, 60, 220],
    ];
    for (id, members) in groups {
        let anchors = members.iter().map(|&i| boxes[i]).collect::<Vec<_>>();
        for (k, shape) in index.balloon_shapes(id, &anchors).into_iter().enumerate() {
            let colour = colours[(members[k] + id as usize) % colours.len()];
            let Some(shape) = shape else { continue };
            for (row, span) in shape.rows.iter().enumerate() {
                let Some((left, right)) = span else { continue };
                let y = (shape.frame.y + row as f32) as u32;
                for x in (shape.frame.x + left) as u32..(shape.frame.x + right) as u32 {
                    if x < canvas.width() && y < canvas.height() {
                        let p = canvas.get_pixel_mut(x, y);
                        for c in 0..3 {
                            p.0[c] = ((p.0[c] as u16 + colour[c] as u16) / 2) as u8;
                        }
                    }
                }
            }
            let b = anchors[k];
            for x in b.x as u32..(b.x + b.width) as u32 {
                for y in [b.y as u32, (b.y + b.height) as u32] {
                    if x < canvas.width() && y < canvas.height() {
                        canvas.put_pixel(x, y, image::Rgba([255, 255, 0, 255]));
                    }
                }
            }
        }
    }
    canvas.save(format!("{}.shapes.png", job.out))?;
    Ok(())
}
