use std::{
    borrow::Cow,
    sync::{Arc, LazyLock},
};

use dashmap::{DashMap, Entry};
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::{
    character_filter::{get_character_filter, CharacterFilterConfig, CharacterFilterPtr},
    pre_tokenizer::{get_pre_tokenizer, PreTokenizerConfig, PreTokenizerPtr},
    token_filter::{get_token_filter, TokenFilterConfig, TokenFilterPtr},
    utils::spi_get_one,
};

#[derive(Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct TextAnalyzerConfig {
    #[serde(default)]
    pub character_filters: Vec<CharacterFilterConfig>,
    #[serde(default)]
    pub pre_tokenizer: Option<PreTokenizerConfig>,
    #[serde(default)]
    pub token_filters: Vec<TokenFilterConfig>,
}

pub struct TextAnalyzer {
    pub character_filters: Vec<CharacterFilterPtr>,
    pub pre_tokenizer: Option<PreTokenizerPtr>,
    pub token_filters: Vec<TokenFilterPtr>,
}
pub type TextAnalyzerPtr = Arc<TextAnalyzer>;

impl TextAnalyzer {
    pub fn build(config: TextAnalyzerConfig) -> Self {
        let character_filters = config
            .character_filters
            .into_iter()
            .map(get_character_filter)
            .collect();
        let pre_tokenizer = config.pre_tokenizer.map(get_pre_tokenizer);
        let token_filters = config
            .token_filters
            .into_iter()
            .map(get_token_filter)
            .collect();

        TextAnalyzer {
            character_filters,
            pre_tokenizer,
            token_filters,
        }
    }

    pub fn apply(&self, text: &str) -> Vec<String> {
        let mut text = Cow::Borrowed(text);
        for filter in &self.character_filters {
            filter.apply(&mut text);
        }

        let text = text.as_ref();
        let pre_tokenized = match &self.pre_tokenizer {
            Some(pre_tokenizer) => pre_tokenizer.pre_tokenize(text),
            None => vec![text],
        };
        let mut tokens = pre_tokenized
            .into_iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();

        for filter in &self.token_filters {
            tokens = filter.apply_batch(tokens);
        }

        tokens
    }
}

pgrx::extension_sql!(
    r#"
CREATE TABLE tokenizer_catalog.text_analyzer (
    name TEXT NOT NULL UNIQUE PRIMARY KEY,
    config TEXT NOT NULL
);
"#,
    name = "text_analyzer_table"
);

type TextAnalyzerObjectPool = DashMap<String, TextAnalyzerPtr>;
static TEXT_ANALYZER_OBJECT_POOL: LazyLock<TextAnalyzerObjectPool> =
    LazyLock::new(TextAnalyzerObjectPool::default);

pub fn get_text_analyzer(name: &str) -> TextAnalyzerPtr {
    if let Some(model) = TEXT_ANALYZER_OBJECT_POOL.get(name) {
        return model.clone();
    }

    match TEXT_ANALYZER_OBJECT_POOL.entry(name.to_string()) {
        Entry::Occupied(entry) => entry.get().clone(),
        Entry::Vacant(entry) => {
            if let Some(object) = get_text_analyzer_from_database(name) {
                entry.insert(object.clone());
                return object;
            }

            panic!("TextAnalyzer not found: {}", name);
        }
    }
}

fn get_text_analyzer_from_database(name: &str) -> Option<TextAnalyzerPtr> {
    let config: &str = spi_get_one(
        "SELECT config FROM tokenizer_catalog.text_analyzer WHERE name = $1",
        &[name.into()],
    )?;

    let config: TextAnalyzerConfig = serde_json::from_str(config).unwrap();
    Some(Arc::new(TextAnalyzer::build(config)))
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn create_text_analyzer(name: &str, config: &str) {
    let config: TextAnalyzerConfig = toml::from_str(config).unwrap();
    config.validate().unwrap();

    let config_str = serde_json::to_string(&config).unwrap();
    let text_analyzer = TextAnalyzer::build(config);

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(
                r#"
                INSERT INTO tokenizer_catalog.text_analyzer (name, config) VALUES ($1, $2)
                ON CONFLICT (name) DO NOTHING RETURNING 1
                "#,
                Some(1),
                &[name.into(), config_str.into()],
            )
            .unwrap();

        if tuptable.is_empty() {
            panic!("Text analyzer already exists: {}", name);
        }

        if TEXT_ANALYZER_OBJECT_POOL
            .insert(name.to_string(), Arc::new(text_analyzer))
            .is_some()
        {
            panic!("TextAnalyzer already exists: {}", name);
        }
    });
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn drop_text_analyzer(name: &str) {
    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(
                "DELETE FROM tokenizer_catalog.text_analyzer WHERE name = $1 RETURNING 1",
                Some(1),
                &[name.into()],
            )
            .unwrap();

        if tuptable.is_empty() {
            pgrx::warning!("TextAnalyzer not found: {}", name);
        }
    });

    TEXT_ANALYZER_OBJECT_POOL.remove(name);
}

#[pgrx::pg_extern(immutable, parallel_safe)]
fn apply_text_analyzer(text: &str, text_analyzer_name: &str) -> Vec<String> {
    let text_analyzer = get_text_analyzer(text_analyzer_name);
    text_analyzer.apply(text)
}
