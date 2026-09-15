//! Text styling types shared by scene nodes, the API, and the renderer.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use strum::IntoEnumIterator;
use utoipa::ToSchema;

// ---------------------------------------------------------------------------
// Alignment
// ---------------------------------------------------------------------------

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

// ---------------------------------------------------------------------------
// Shader effect (italic / bold flags)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumIter, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
enum TextShaderEffectFlag {
    Italic,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default, ToSchema, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextShaderEffect {
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub bold: bool,
}

impl TextShaderEffect {
    pub const ITALIC_FLAG: u32 = 1 << 0;
    pub const BOLD_FLAG: u32 = 1 << 1;

    pub fn flags(self) -> u32 {
        let mut flags = 0u32;
        if self.italic {
            flags |= Self::ITALIC_FLAG;
        }
        if self.bold {
            flags |= Self::BOLD_FLAG;
        }
        flags
    }

    pub fn is_empty(self) -> bool {
        self.flags() == 0
    }

    pub fn none() -> Self {
        Self {
            italic: false,
            bold: false,
        }
    }

    fn apply_flag(&mut self, flag: TextShaderEffectFlag) {
        match flag {
            TextShaderEffectFlag::Italic => self.italic = true,
            TextShaderEffectFlag::Bold => self.bold = true,
        }
    }

    fn enabled_flags(self) -> [Option<TextShaderEffectFlag>; 2] {
        [
            self.italic.then_some(TextShaderEffectFlag::Italic),
            self.bold.then_some(TextShaderEffectFlag::Bold),
        ]
    }
}

fn valid_shader_effects() -> String {
    TextShaderEffectFlag::iter()
        .map(|flag| flag.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for TextShaderEffect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts = self
            .enabled_flags()
            .into_iter()
            .flatten()
            .map(|flag| flag.to_string())
            .collect::<Vec<_>>();

        if parts.is_empty() {
            f.write_str("none")
        } else {
            f.write_str(&parts.join(","))
        }
    }
}

impl FromStr for TextShaderEffect {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_lowercase();
        if normalized.is_empty() || normalized == "none" || normalized == "normal" {
            return Ok(Self::none());
        }

        let mut effect = Self::none();
        for token in normalized
            .split(|c: char| c == ',' || c == '|' || c == '+' || c.is_whitespace())
            .filter(|token| !token.is_empty())
        {
            if matches!(token, "normal" | "none") {
                continue;
            }

            let flag = token.parse::<TextShaderEffectFlag>().map_err(|_| {
                anyhow::anyhow!(
                    "Unknown shader effect: {token}. Valid: {}",
                    valid_shader_effects()
                )
            })?;
            effect.apply_flag(flag);
        }

        Ok(effect)
    }
}

impl<'de> Deserialize<'de> for TextShaderEffect {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FlagsRepr {
            italic: Option<bool>,
            bold: Option<bool>,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct BinaryFlagsRepr {
            italic: bool,
            bold: bool,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Flags(FlagsRepr),
            Legacy(String),
        }

        if deserializer.is_human_readable() {
            return match Repr::deserialize(deserializer)? {
                Repr::Flags(FlagsRepr { italic, bold }) => Ok(Self {
                    italic: italic.unwrap_or(false),
                    bold: bold.unwrap_or(false),
                }),
                Repr::Legacy(value) => value.parse().map_err(serde::de::Error::custom),
            };
        }

        let BinaryFlagsRepr { italic, bold } = BinaryFlagsRepr::deserialize(deserializer)?;
        Ok(Self { italic, bold })
    }
}

// ---------------------------------------------------------------------------
// Stroke
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextStrokeStyle {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Explicit outline colour. `None` = automatic: contrast against the
    /// resolved text colour (white text gets a black outline and vice
    /// versa). Became `Option` in scene format v5; earlier formats stored a
    /// required colour that defaulted to white, converted on upgrade in
    /// `session.rs::compat`.
    #[serde(default)]
    pub color: Option<[u8; 4]>,
    #[serde(default)]
    pub width_px: Option<f32>,
}

impl Default for TextStrokeStyle {
    fn default() -> Self {
        Self {
            enabled: true,
            color: None,
            width_px: None,
        }
    }
}

const fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Text style (scene-facing)
// ---------------------------------------------------------------------------

/// Axis of a two-colour fill gradient, in the text's local (unrotated) frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GradientDirection {
    /// Left → right.
    Horizontal,
    /// Top → bottom.
    Vertical,
}

/// Two-colour gradient across the text fill. The fill starts at
/// `TextStyle::color` and ends at `to`; the stroke keeps its own colour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextFillGradient {
    pub to: [u8; 4],
    pub direction: GradientDirection,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextStyle {
    pub font_families: Vec<String>,
    pub font_size: Option<f32>,
    /// Explicit fill colour. `None` = automatic (renderer picks black/white
    /// by background contrast). Any stored value — including pure black or
    /// white — is honoured verbatim. Became `Option` in scene format v4;
    /// earlier formats used sentinel colours (pure black / the predicted
    /// colour) for "auto", converted on upgrade in `session.rs::compat`.
    #[serde(default)]
    pub color: Option<[u8; 4]>,
    pub effect: Option<TextShaderEffect>,
    pub stroke: Option<TextStrokeStyle>,
    #[serde(default)]
    pub text_align: Option<TextAlign>,
    /// NOTE: appended for scene format v3 — any layout change here needs a
    /// SCENE_FORMAT_VERSION bump + a frozen copy in `session.rs::compat`.
    #[serde(default)]
    pub gradient: Option<TextFillGradient>,
}

/// Character-level overrides for a slice of translated text. Each optional
/// field inherits the block's [`TextStyle`] when absent; explicit booleans let
/// a range turn bold/italic off even when the whole block enables it.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TextRangeStyle {
    #[serde(default)]
    pub color: Option<[u8; 4]>,
    #[serde(default)]
    pub bold: Option<bool>,
    #[serde(default)]
    pub italic: Option<bool>,
}

impl TextRangeStyle {
    pub fn is_empty(self) -> bool {
        self.color.is_none() && self.bold.is_none() && self.italic.is_none()
    }
}

/// A half-open UTF-8 byte range (`start..end`) in `TextData::translation`.
/// Ranges may overlap; later entries win per property. Producers should keep
/// them sorted, non-empty, and on character boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextStyleRange {
    pub start: u32,
    pub end: u32,
    pub style: TextRangeStyle,
}

#[cfg(test)]
mod tests {
    use super::{GradientDirection, TextFillGradient, TextShaderEffect, TextStyle};

    #[test]
    fn parse_combined_effects() {
        let effect: TextShaderEffect = "italic,bold".parse().expect("parse");
        assert!(effect.italic);
        assert!(effect.bold);
    }

    #[test]
    fn default_has_no_effects() {
        let effect = TextShaderEffect::default();
        assert!(!effect.italic);
        assert!(!effect.bold);
    }

    #[test]
    fn parse_none_disables_all_effects() {
        let effect: TextShaderEffect = "none".parse().expect("parse");
        assert_eq!(effect.to_string(), "none");
    }

    #[test]
    fn json_legacy_string_deserializes() {
        let effect: TextShaderEffect = serde_json::from_str("\"italic,bold\"").expect("json");
        assert!(effect.italic);
        assert!(effect.bold);
    }

    #[test]
    fn postcard_text_shader_effect_round_trips() {
        let effect = TextShaderEffect {
            italic: true,
            bold: true,
        };
        let bytes = postcard::to_allocvec(&effect).expect("serialize");
        let decoded: TextShaderEffect = postcard::from_bytes(&bytes).expect("deserialize");
        assert!(decoded.italic);
        assert!(decoded.bold);
    }

    #[test]
    fn postcard_text_style_with_effect_round_trips() {
        let style = TextStyle {
            font_families: vec!["Arial".to_string()],
            font_size: Some(18.0),
            color: Some([12, 34, 56, 255]),
            effect: Some(TextShaderEffect {
                italic: true,
                bold: false,
            }),
            stroke: None,
            text_align: None,
            gradient: Some(TextFillGradient {
                to: [200, 40, 90, 255],
                direction: GradientDirection::Vertical,
            }),
        };
        let bytes = postcard::to_allocvec(&style).expect("serialize");
        let decoded: TextStyle = postcard::from_bytes(&bytes).expect("deserialize");
        let effect = decoded.effect.expect("effect");
        assert!(effect.italic);
        assert!(!effect.bold);
    }
}
