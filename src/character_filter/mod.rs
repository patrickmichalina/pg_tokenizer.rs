mod to_lowercase;
mod unicode_normalization;

use std::{borrow::Cow, sync::Arc};

use serde::{Deserialize, Serialize};
use to_lowercase::ToLowercase;
use unicode_normalization::UnicodeNormalizationConfig;

pub trait CharacterFilter {
    fn apply(&self, text: &mut Cow<str>);
}
pub type CharacterFilterPtr = Arc<dyn CharacterFilter + Send + Sync>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CharacterFilterConfig {
    ToLowercase,
    UnicodeNormalization(UnicodeNormalizationConfig),
}

pub fn get_character_filter(config: CharacterFilterConfig) -> CharacterFilterPtr {
    match config {
        CharacterFilterConfig::ToLowercase => Arc::new(ToLowercase),
        CharacterFilterConfig::UnicodeNormalization(config) => {
            unicode_normalization::get_unicode_normalization(config)
        }
    }
}
