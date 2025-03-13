use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock},
};

use dashmap::{DashMap, Entry};
use serde::{Deserialize, Serialize};

use crate::utils::spi_get_one;

use super::TokenFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SynonymTokenFilter {
    synonyms: HashMap<String, String>,
}
pub type SynonymTokenFilterPtr = Arc<SynonymTokenFilter>;

impl SynonymTokenFilter {
    // config is a string with multiple lines, each line represents a group of synonyms for a single word, separated by spaces
    pub fn build(config: &str) -> Self {
        let mut synonyms = HashMap::new();
        let mut duplicate_check = HashSet::new();

        for line in config.lines() {
            let mut words = line.split_whitespace();
            if let Some(first) = words.next() {
                if !duplicate_check.insert(first.to_string()) {
                    panic!("Duplicate word defined: {}", first);
                }
                for word in words {
                    if !duplicate_check.insert(word.to_string()) {
                        panic!("Duplicate word defined: {}", word);
                    }
                    synonyms.insert(word.to_string(), first.to_string());
                }
            }
        }

        SynonymTokenFilter { synonyms }
    }
}

impl TokenFilter for SynonymTokenFilter {
    fn apply(&self, token: String) -> Vec<String> {
        if let Some(synonym) = self.synonyms.get(&token) {
            vec![synonym.clone()]
        } else {
            vec![token]
        }
    }
}

pgrx::extension_sql!(
    r#"
CREATE TABLE tokenizer_catalog.synonym (
    name TEXT NOT NULL UNIQUE PRIMARY KEY,
    config TEXT NOT NULL
);
"#,
    name = "synonym_table"
);

type SynonymObjectPool = DashMap<String, SynonymTokenFilterPtr>;
static SYNONYM_OBJECT_POOL: LazyLock<SynonymObjectPool> = LazyLock::new(SynonymObjectPool::default);

pub fn get_synonym_token_filter(name: &str) -> SynonymTokenFilterPtr {
    if let Some(model) = SYNONYM_OBJECT_POOL.get(name) {
        return model.clone();
    }

    match SYNONYM_OBJECT_POOL.entry(name.to_string()) {
        Entry::Occupied(entry) => entry.get().clone(),
        Entry::Vacant(entry) => {
            if let Some(object) = get_synonym_token_filter_from_database(name) {
                entry.insert(object.clone());
                return object;
            }

            panic!("Synonym not found: {}", name);
        }
    }
}

fn get_synonym_token_filter_from_database(name: &str) -> Option<SynonymTokenFilterPtr> {
    let config: &str = spi_get_one(
        "SELECT config FROM tokenizer_catalog.synonym WHERE name = $1",
        &[name.into()],
    )?;

    let synonym = SynonymTokenFilter::build(config);
    Some(Arc::new(synonym))
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn create_synonym(name: &str, config: &str) {
    let synonym = SynonymTokenFilter::build(config);

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(
                r#"
                INSERT INTO tokenizer_catalog.synonym (name, config) VALUES ($1, $2)
                ON CONFLICT (name) DO NOTHING RETURNING 1
                "#,
                Some(1),
                &[name.into(), config.into()],
            )
            .unwrap();

        if tuptable.is_empty() {
            panic!("Synonym already exists: {}", name);
        }

        if SYNONYM_OBJECT_POOL
            .insert(name.to_string(), Arc::new(synonym))
            .is_some()
        {
            panic!("Synonym already exists: {}", name);
        }
    });
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn drop_synonym(name: &str) {
    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(
                "DELETE FROM tokenizer_catalog.synonym WHERE name = $1 RETURNING 1",
                Some(1),
                &[name.into()],
            )
            .unwrap();

        if tuptable.is_empty() {
            pgrx::warning!("Synonym not found: {}", name);
        }
    });

    SYNONYM_OBJECT_POOL.remove(name);
}
