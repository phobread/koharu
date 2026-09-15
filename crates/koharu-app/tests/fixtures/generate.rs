#![allow(dead_code, unused_imports)]
mod blob;
mod font;
mod op;
mod scene;
mod style;
pub use blob::*;
pub use font::*;
pub use op::*;
pub use scene::*;
pub use style::*;

#[derive(serde::Serialize)]
struct Snapshot {
    epoch: u64,
    scene: Scene,
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let version: u16 = args[1].parse().unwrap();
    let output = std::path::Path::new(&args[2]);
    // JSON is only construction input; the historical types below perform the
    // postcard serialization. Stable UUIDs/timestamps make fixtures reproducible.
    let scene: Scene = serde_json::from_value(serde_json::json!({
        "project": {
            "name": "Historical compatibility fixture",
            "createdAt": "2026-07-01T00:00:00Z",
            "updatedAt": "2026-07-02T00:00:00Z",
            "style": { "defaultFont": "Fixture Sans" }
        },
        "pages": {
            "00000000-0000-0000-0000-000000000001": {
                "id": "00000000-0000-0000-0000-000000000001",
                "name": "Mixed nodes", "width": 800, "height": 600,
                "nodes": {
                    "00000000-0000-0000-0000-000000000002": {
                        "id": "00000000-0000-0000-0000-000000000002", "visible": true,
                        "transform": { "x": 1.0, "y": 2.0, "width": 800.0, "height": 600.0, "rotationDeg": 12.0 },
                        "kind": { "image": { "role": "source", "blob": "fixture-source", "opacity": 0.75,
                            "naturalWidth": 800, "naturalHeight": 600, "name": "source.png" } }
                    },
                    "00000000-0000-0000-0000-000000000003": {
                        "id": "00000000-0000-0000-0000-000000000003", "visible": true,
                        "transform": { "x": 10.0, "y": 20.0, "width": 200.0, "height": 100.0, "rotationDeg": -15.0 },
                        "kind": { "text": {
                            "confidence": 0.875, "sourceLang": "ko", "text": "한글", "translation": "Hello 猫🙂",
                            "sourceDirection": "vertical", "renderedDirection": "horizontal",
                            "linePolygons": [[[1.0, 2.0], [3.0, 4.0], [5.0, 6.0], [7.0, 8.0]]],
                            "rotationDeg": 7.0, "detectedFontSizePx": 18.0, "detector": "fixture-detector",
                            "style": { "fontFamilies": ["Fixture Serif"], "fontSize": 22.0,
                                "color": [12, 34, 56, 255], "effect": { "bold": true, "italic": true },
                                "stroke": { "enabled": true, "color": [210, 211, 212, 255], "widthPx": 2.0 },
                                "textAlign": "center" },
                            "fontPrediction": { "topFonts": [{ "index": 2, "score": 0.75 }],
                                "namedFonts": [{ "index": 2, "name": "Fixture Serif", "language": "ko", "probability": 0.75, "serif": true }],
                                "direction": "vertical", "textColor": [40, 41, 42], "strokeColor": [210, 211, 212],
                                "fontSizePx": 18.0, "strokeWidthPx": 2.0, "lineHeight": 1.25, "angleDeg": 7.0 },
                            "sprite": "fixture-sprite", "spriteTransform": { "x": 11.0, "y": 21.0, "width": 201.0, "height": 101.0, "rotationDeg": -14.0 },
                            "renderedFontSizePx": 22.0, "renderedTextColor": [12, 34, 56, 255], "lockLayoutBox": true
                        } }
                    },
                    "00000000-0000-0000-0000-000000000004": {
                        "id": "00000000-0000-0000-0000-000000000004", "visible": false,
                        "transform": { "x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0, "rotationDeg": 0.0 },
                        "kind": { "mask": { "role": "bubble", "blob": "fixture-mask" } }
                    }
                }
            }
        }
    })).unwrap();
    let snapshot = Snapshot { epoch: 42, scene };
    let payload = postcard::to_allocvec(&snapshot).unwrap();
    let mut bytes = Vec::new();
    if version != 1 {
        bytes.extend_from_slice(b"KSCN");
        bytes.extend_from_slice(&version.to_le_bytes());
    }
    bytes.extend_from_slice(&payload);
    std::fs::write(output.join(format!("scene-v{version}.bin")), bytes).unwrap();
    std::fs::write(
        output.join(format!("scene-v{version}.json")),
        serde_json::to_vec_pretty(&snapshot.scene).unwrap(),
    )
    .unwrap();
}
