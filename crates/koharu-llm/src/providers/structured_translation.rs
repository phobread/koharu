//! OpenRouter translation contract. Required ID keys express exact coverage
//! without relying on provider-specific array length or numeric constraints.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, de};
use serde_json::{Value, json};

use crate::{Language, prompt};

pub(super) fn prompts(
    sources: &[String],
    language: Language,
    custom: Option<&str>,
) -> (String, String) {
    let system = prompt::system_prompt_with_output_instructions(
        language,
        custom,
        "The input is a JSON array of blocks with id and text fields, in block order. Return only a JSON object with a translations object mapping every input ID to its translated text. Copy every ID exactly once. Do not merge, split, omit, or add blocks. Keep empty input blocks empty.",
    );
    let input: Vec<Value> = sources
        .iter()
        .enumerate()
        .map(|(index, source)| json!({"id": (index + 1).to_string(), "text": source}))
        .collect();
    (
        system,
        serde_json::to_string(&input).expect("source blocks serialize"),
    )
}

pub(super) fn response_format(count: usize) -> Value {
    let properties: serde_json::Map<String, Value> = (1..=count)
        .map(|id| (id.to_string(), json!({"type": "string"})))
        .collect();
    let required: Vec<String> = (1..=count).map(|id| id.to_string()).collect();
    json!({"type": "json_schema", "json_schema": {
        "name": "manga_translation", "strict": true,
        "schema": {"type": "object", "properties": {
            "translations": {"type": "object", "properties": properties,
                "required": required, "additionalProperties": false}
        }, "required": ["translations"], "additionalProperties": false}
    }})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    #[serde(deserialize_with = "unique_translations")]
    translations: BTreeMap<String, String>,
}

fn unique_translations<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    struct Unique;
    impl<'de> de::Visitor<'de> for Unique {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a translation object with unique block IDs")
        }
        fn visit_map<M: de::MapAccess<'de>>(self, mut access: M) -> Result<Self::Value, M::Error> {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = access.next_entry::<String, String>()? {
                if values.insert(key, value).is_some() {
                    return Err(de::Error::custom("duplicate translation block ID"));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(Unique)
}

pub(super) fn parse(text: &str, sources: &[String]) -> Result<Vec<String>> {
    let mut output: Output = serde_json::from_str(text).context(
        "OpenRouter returned invalid structured translation JSON; no translations were applied",
    )?;
    ensure!(
        output.translations.len() == sources.len(),
        "OpenRouter translation block count mismatch; no translations were applied"
    );
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let id = (index + 1).to_string();
            let text = output.translations.remove(&id).with_context(|| {
                format!(
                    "OpenRouter translation is missing block {id}; no translations were applied"
                )
            })?;
            ensure!(
                !source.trim().is_empty() || text.trim().is_empty(),
                "OpenRouter translated an empty block; no translations were applied"
            );
            // JSON already unescapes quotes. Quoted dialogue is content, not a wrapper.
            Ok(text.trim().to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_preserves_block_order_past_nine() {
        let sources = (1..=12).map(|id| format!("block {id}")).collect::<Vec<_>>();
        let (_, input) = prompts(&sources, Language::English, None);
        let input: Value = serde_json::from_str(&input).unwrap();
        for (index, source) in sources.iter().enumerate() {
            assert_eq!(input[index]["id"], (index + 1).to_string());
            assert_eq!(input[index]["text"], *source);
        }
    }

    #[test]
    fn restores_numeric_order_and_preserves_text() {
        let sources = vec!["source".into(); 12];
        let values: BTreeMap<_, _> = (1..=12)
            .rev()
            .map(|id| (id.to_string(), format!("\"안녕 {id}\"\n[2] literal")))
            .collect();
        let output = parse(&json!({"translations": values}).to_string(), &sources).unwrap();
        assert_eq!(output[1], "\"안녕 2\"\n[2] literal");
        assert_eq!(output[11], "\"안녕 12\"\n[2] literal");
    }

    #[test]
    fn rejects_incomplete_duplicate_extra_and_malformed_results() {
        for text in [
            r#"{"translations":{"1":"ok"}}"#,
            r#"{"translations":{"1":"ok","1":"duplicate","2":"ok"}}"#,
            r#"{"translations":{"1":"ok","3":"wrong ID"}}"#,
            r#"{"translations":{"1":"ok","2":"ok","3":"extra"}}"#,
            r#"{"translations":{"1":"ok","2":null}}"#,
            r#"{"translations":{"1":"ok","2":"ok"},"notes":"extra"}"#,
            r#"{"translations":{"1":"ok","2":"cut off"#,
            "```json\n{\"translations\":{\"1\":\"ok\",\"2\":\"ok\"}}\n```",
        ] {
            assert!(parse(text, &["a".into(), "b".into()]).is_err(), "{text}");
        }
    }

    #[test]
    fn empty_blocks_and_custom_prompt_are_preserved() {
        assert_eq!(
            parse(
                r#"{"translations":{"1":"","2":"hello"}}"#,
                &[" ".into(), "hi".into()]
            )
            .unwrap(),
            ["", "hello"]
        );
        assert!(parse(r#"{"translations":{"1":"invented"}}"#, &["".into()]).is_err());
        let (system, input) = prompts(
            &["[2] literal\n\"quote\"".into(), "".into()],
            Language::English,
            Some("Keep honorifics."),
        );
        assert!(system.contains("Keep honorifics."));
        assert!(system.contains("natural English"));
        assert!(!system.contains(prompt::BLOCK_TAG_INSTRUCTIONS));
        assert_eq!(
            serde_json::from_str::<Value>(&input).unwrap()[0]["text"],
            "[2] literal\n\"quote\""
        );
        let schema = response_format(2);
        assert_eq!(
            schema["json_schema"]["schema"]["properties"]["translations"]["required"],
            json!(["1", "2"])
        );
    }
}
