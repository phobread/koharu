//! Opt-in evaluation of the official-release piece rule on real pages.
//!
//! `KOHARU_OFFICIAL_JOBS` names a JSON array of jobs:
//! `{ "raw": path, "official": path, "bubble": path?, "segment": path?,
//!    "boxes": [Transform], "out": path }`.
//! Writes each page's piece mask (white = copied from the release) to `out`
//! (and the owner's bubble parts next to it, `_own.png`), and prints how
//! many pixels come over and how long the rule took.

use anyhow::{Context, Result};
use koharu_app::official::piece_mask;
use koharu_core::Transform;
use serde::Deserialize;

#[derive(Deserialize)]
struct Job {
    raw: String,
    official: String,
    bubble: Option<String>,
    segment: Option<String>,
    boxes: Vec<Transform>,
    out: String,
}

#[test]
#[ignore = "needs KOHARU_OFFICIAL_JOBS"]
fn official_pieces_eval() -> Result<()> {
    let path = std::env::var("KOHARU_OFFICIAL_JOBS").context("KOHARU_OFFICIAL_JOBS")?;
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    for job in jobs {
        let raw = image::open(&job.raw)?.to_rgba8();
        let official = image::open(&job.official)?.to_rgba8();
        let bubble = job
            .bubble
            .as_deref()
            .map(image::open)
            .transpose()?
            .map(|m| m.to_luma8());
        let segment = job
            .segment
            .as_deref()
            .map(image::open)
            .transpose()?
            .map(|m| m.to_luma8());
        let started = std::time::Instant::now();
        let mask = piece_mask(
            &raw,
            &official,
            &job.boxes,
            segment.as_ref(),
            bubble.as_ref(),
        );
        let elapsed = started.elapsed();
        let pixels = mask.pixels().filter(|p| p.0[0] != 0).count();
        mask.save(&job.out)?;
        if let Some(bubble) = bubble.as_ref() {
            let own = koharu_app::official::owned_bubble_parts(&job.boxes, bubble);
            own.save(job.out.replace("_mask.png", "_own.png"))?;
        }
        println!(
            "{}: {pixels} px from the release, {} ms",
            job.out,
            elapsed.as_millis()
        );
    }
    Ok(())
}
