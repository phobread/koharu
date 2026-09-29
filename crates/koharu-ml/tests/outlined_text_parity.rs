//! Opt-in check that `outlined_text` reproduces the reference prototype the
//! OCR measurements were made with, pixel for pixel, on real text boxes.
//!
//! Env: KOHARU_OUTLINE_PARITY = dir with manifest.json and, per entry,
//! `<name>_win.png` (the padded window) and `<name>_exp.png` (the reference
//! output; absent when the reference keeps the original). Also checks that
//! `outline_window` derives the same window and box from the page rect.
//! Run: bun cargo test -p koharu-ml --test outlined_text_parity -- --ignored --nocapture

use std::path::PathBuf;

use koharu_ml::outlined_text::{clean_outlined_text, outline_window};
use serde_json::Value;

#[test]
#[ignore]
fn outlined_text_matches_reference() {
    let dir = PathBuf::from(std::env::var("KOHARU_OUTLINE_PARITY").expect("KOHARU_OUTLINE_PARITY"));
    let manifest: Vec<Value> =
        serde_json::from_reader(std::fs::File::open(dir.join("manifest.json")).unwrap()).unwrap();
    let (mut same, mut gate_mismatch, mut pixel_mismatch, mut window_mismatch) = (0, 0, 0, 0);
    for entry in &manifest {
        let name = entry["name"].as_str().unwrap();
        let t = &entry["t"];
        let f = |k: &str| t[k].as_f64().unwrap() as f32;
        let size = entry["page_size"].as_array().unwrap();
        let (window, rect) = outline_window(
            size[0].as_u64().unwrap() as u32,
            size[1].as_u64().unwrap() as u32,
            f("x"),
            f("y"),
            f("x") + f("width"),
            f("y") + f("height"),
        );
        let want_window: Vec<u32> = entry["window"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        let want_rect: Vec<u32> = entry["page_rect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        if window.to_vec() != want_window || rect.to_vec() != want_rect {
            window_mismatch += 1;
            println!("{name}: window {window:?}/{rect:?} vs {want_window:?}/{want_rect:?}");
        }
        let win = image::open(dir.join(format!("{name}_win.png")))
            .unwrap()
            .to_rgb8();
        let b: Vec<u32> = entry["box"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        let got = clean_outlined_text(&win, [b[0], b[1], b[2], b[3]]);
        let expected = entry["expected"].as_bool().unwrap();
        match (got, expected) {
            (None, false) => same += 1,
            (Some(out), true) => {
                let exp = image::open(dir.join(format!("{name}_exp.png")))
                    .unwrap()
                    .to_rgb8();
                let diff = out
                    .pixels()
                    .zip(exp.pixels())
                    .filter(|(a, b)| a != b)
                    .count();
                if diff == 0 {
                    same += 1;
                } else {
                    pixel_mismatch += 1;
                    println!("{name}: {diff} pixels differ");
                }
            }
            (got, _) => {
                gate_mismatch += 1;
                println!(
                    "{name}: rust cleaned={} reference cleaned={expected}",
                    got.is_some()
                );
            }
        }
    }
    println!(
        "{} boxes: {same} identical, {pixel_mismatch} pixel mismatches, {gate_mismatch} gate mismatches, {window_mismatch} window mismatches",
        manifest.len()
    );
    assert_eq!(same, manifest.len());
    assert_eq!(window_mismatch, 0);
}
