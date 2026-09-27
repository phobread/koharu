//! Opt-in check that the shipped Korean repair path (`single_line_ocr_text` →
//! polarity fallback → `repair_hangul` → `space_at_line_breaks` → emoji
//! safeguard) reproduces the offline rules the blind evaluation scored, on the
//! readings recorded by `ocr_line_split_eval`. No models are loaded.
//!
//! Env: KOHARU_PARITY_ROOTS (comma-separated result dirs, each with
//!      results.json and crops/), KOHARU_PARITY_EXPECTED (JSON map
//!      "<dir name>/<id>" -> expected text).
//! Run: bun cargo test -p koharu-app --test ocr_korean_rules_parity -- --ignored --nocapture

use std::path::PathBuf;

use anyhow::{Context, Result};
use koharu_app::pipeline::support::{is_degenerate_ocr_text, single_line_ocr_text};
use koharu_ml::korean_ocr::{
    LineRecognition, hangul_confidence, is_dark_panel, prefer_trusted_polarity, repair_hangul,
    space_at_line_breaks,
};
use serde_json::Value;

const MINIMUM_CONFIDENCE: f32 = 0.77;

fn lines(value: &Value) -> Vec<LineRecognition> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|line| LineRecognition {
            text: line["text"].as_str().unwrap_or_default().to_owned(),
            confidence: line["confidence"].as_f64().unwrap_or_default() as f32,
            char_confidences: line["chars"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|score| score.as_f64().unwrap_or_default() as f32)
                .collect(),
        })
        .collect()
}

#[test]
#[ignore = "needs recorded evaluation outputs"]
fn shipped_korean_rules_match_the_evaluated_rules() -> Result<()> {
    let expected: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("KOHARU_PARITY_EXPECTED")?)?)?;
    let mut checked = 0;
    let mut mismatches = Vec::new();
    for root in std::env::var("KOHARU_PARITY_ROOTS")?.split(',') {
        let root = PathBuf::from(root);
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .context("dir name")?;
        let set = name.rsplit('-').next().context("set name")?;
        let results: Value = serde_json::from_slice(&std::fs::read(root.join("results.json"))?)?;
        for row in results["rows"].as_array().context("rows")? {
            let id = row["id"].as_str().context("id")?;
            let block = image::open(root.join("crops").join(format!("{id}-block.png")))?;
            let dark = is_dark_panel(&block);
            let original = lines(&row["pp_new"]["original"]);
            let inverted = lines(&row["pp_new"]["inverted"]);
            let chosen = original
                .into_iter()
                .zip(inverted)
                .map(|(original, inverted)| {
                    if !dark || hangul_confidence(&original) >= f64::from(MINIMUM_CONFIDENCE) {
                        original
                    } else {
                        prefer_trusted_polarity(original, Some(inverted), MINIMUM_CONFIDENCE)
                    }
                })
                .collect::<Vec<_>>();
            let text = single_line_ocr_text(row["vl"]["raw"].as_str().unwrap_or_default());
            let repaired = repair_hangul(&text, &chosen, MINIMUM_CONFIDENCE);
            let mut out = space_at_line_breaks(&repaired, &chosen);
            if is_degenerate_ocr_text(&out) {
                out.clear();
            }
            let key = format!("{set}/{id}");
            let want = expected[&key].as_str().context("expected entry")?;
            checked += 1;
            if out != want {
                mismatches.push(format!("{key}: rust {out:?} python {want:?}"));
            }
        }
    }
    for mismatch in &mismatches {
        eprintln!("MISMATCH {mismatch}");
    }
    eprintln!("checked {checked}, mismatches {}", mismatches.len());
    anyhow::ensure!(mismatches.is_empty(), "{} mismatches", mismatches.len());
    Ok(())
}
