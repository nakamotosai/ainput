use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::config::TermEmbeddingConfig;
use crate::history::{self, HistoryRecord};
use crate::personal_corrections::{self, PersonalCorrectionRule};
use crate::suspect_terms::{self, SuspectTermItem};

#[derive(Clone)]
pub struct TermEmbeddingController {
    cache_path: PathBuf,
    status_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TermEmbeddingCache {
    pub version: u32,
    pub updated_ms: u128,
    pub entries: Vec<TermEmbeddingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TermEmbeddingEntry {
    pub id: String,
    pub canonical: String,
    pub variant: String,
    pub source: String,
    pub input_hash: String,
    pub input_text: String,
    pub model: String,
    pub dimensions: usize,
    pub embedding: Vec<f32>,
    pub created_ms: u128,
    pub updated_ms: u128,
    pub last_error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TermEmbeddingStatus {
    pub ok: bool,
    pub phase: String,
    pub endpoint_url: String,
    pub model: String,
    pub cache_entries: usize,
    pub active_cache_entries: usize,
    pub planned_items: usize,
    pub embedded_items: usize,
    pub pending_items: usize,
    pub pruned_entries: usize,
    pub family_count: usize,
    pub hotword_count: usize,
    pub last_run_ms: u128,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TermFamilyIndex {
    pub version: u32,
    pub updated_ms: u128,
    pub model: String,
    pub review_only: bool,
    pub source_entries: usize,
    pub families: Vec<TermFamily>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TermFamily {
    pub canonical: String,
    pub variants: Vec<TermFamilyVariant>,
    pub sources: Vec<String>,
    pub max_similarity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TermFamilyVariant {
    pub text: String,
    pub sources: Vec<String>,
    pub max_similarity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TermHotwordExport {
    pub version: u32,
    pub updated_ms: u128,
    pub source: String,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmbeddingWorkItem {
    id: String,
    canonical: String,
    variant: String,
    source: String,
    input_hash: String,
    input_text: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

impl Default for TermEmbeddingCache {
    fn default() -> Self {
        Self {
            version: 1,
            updated_ms: now_ms(),
            entries: Vec::new(),
        }
    }
}

impl Default for TermEmbeddingEntry {
    fn default() -> Self {
        let now = now_ms();
        Self {
            id: String::new(),
            canonical: String::new(),
            variant: String::new(),
            source: String::new(),
            input_hash: String::new(),
            input_text: String::new(),
            model: String::new(),
            dimensions: 0,
            embedding: Vec::new(),
            created_ms: now,
            updated_ms: now,
            last_error: String::new(),
        }
    }
}

impl Default for TermEmbeddingStatus {
    fn default() -> Self {
        Self {
            ok: true,
            phase: "not_started".to_string(),
            endpoint_url: String::new(),
            model: String::new(),
            cache_entries: 0,
            active_cache_entries: 0,
            planned_items: 0,
            embedded_items: 0,
            pending_items: 0,
            pruned_entries: 0,
            family_count: 0,
            hotword_count: 0,
            last_run_ms: 0,
            error: None,
        }
    }
}

impl Default for TermFamilyIndex {
    fn default() -> Self {
        Self {
            version: 1,
            updated_ms: now_ms(),
            model: String::new(),
            review_only: true,
            source_entries: 0,
            families: Vec::new(),
        }
    }
}

impl Default for TermFamily {
    fn default() -> Self {
        Self {
            canonical: String::new(),
            variants: Vec::new(),
            sources: Vec::new(),
            max_similarity: 0.0,
        }
    }
}

impl Default for TermFamilyVariant {
    fn default() -> Self {
        Self {
            text: String::new(),
            sources: Vec::new(),
            max_similarity: 0.0,
        }
    }
}

impl Default for TermHotwordExport {
    fn default() -> Self {
        Self {
            version: 1,
            updated_ms: now_ms(),
            source: "personal_corrections".to_string(),
            terms: Vec::new(),
        }
    }
}

impl TermEmbeddingController {
    pub fn start(
        config: TermEmbeddingConfig,
        corrections_path: PathBuf,
        suspect_path: PathBuf,
        history_path: PathBuf,
        cache_path: PathBuf,
        status_path: PathBuf,
        families_path: PathBuf,
        hotwords_path: PathBuf,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create term embedding dir {}", parent.display()))?;
        }
        write_status(
            &status_path,
            &TermEmbeddingStatus {
                ok: true,
                phase: if config.enabled {
                    "waiting".to_string()
                } else {
                    "disabled".to_string()
                },
                endpoint_url: config.endpoint_url.clone(),
                model: config.model.clone(),
                ..Default::default()
            },
        )?;
        let worker = TermEmbeddingWorker::new(
            config,
            corrections_path,
            suspect_path,
            history_path,
            cache_path.clone(),
            status_path.clone(),
            families_path,
            hotwords_path,
        );
        thread::spawn(move || worker.run(shutdown));
        Ok(Self {
            cache_path,
            status_path,
        })
    }

    #[allow(dead_code)]
    pub fn cache_path(&self) -> &Path {
        &self.cache_path
    }

    #[allow(dead_code)]
    pub fn status_path(&self) -> &Path {
        &self.status_path
    }
}

struct TermEmbeddingWorker {
    config: TermEmbeddingConfig,
    corrections_path: PathBuf,
    suspect_path: PathBuf,
    history_path: PathBuf,
    cache_path: PathBuf,
    status_path: PathBuf,
    families_path: PathBuf,
    hotwords_path: PathBuf,
    http: Client,
    api_key: Option<String>,
}

impl TermEmbeddingWorker {
    fn new(
        config: TermEmbeddingConfig,
        corrections_path: PathBuf,
        suspect_path: PathBuf,
        history_path: PathBuf,
        cache_path: PathBuf,
        status_path: PathBuf,
        families_path: PathBuf,
        hotwords_path: PathBuf,
    ) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(1000)))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| Client::new());
        let api_key = read_api_key(config.api_key_env.trim()).or_else(|| {
            let inline = config.api_key.trim().to_string();
            (!inline.is_empty()).then_some(inline)
        });
        Self {
            config,
            corrections_path,
            suspect_path,
            history_path,
            cache_path,
            status_path,
            families_path,
            hotwords_path,
            http,
            api_key,
        }
    }

    fn run(self, shutdown: Arc<AtomicBool>) {
        info!(
            enabled = self.config.enabled,
            endpoint_url = %self.config.endpoint_url,
            model = %self.config.model,
            cache_path = %self.cache_path.display(),
            "term embedding worker started"
        );
        if !self.config.enabled {
            return;
        }
        sleep_until_shutdown(
            Duration::from_millis(self.config.startup_delay_ms),
            &shutdown,
        );
        while !shutdown.load(Ordering::Relaxed) {
            if let Err(error) = self.run_once() {
                warn!(error = %error, "term embedding worker run failed");
                let cache_entries = load_cache(&self.cache_path)
                    .map(|cache| cache.entries.len())
                    .unwrap_or_default();
                let _ = write_status(
                    &self.status_path,
                    &TermEmbeddingStatus {
                        ok: false,
                        phase: "error".to_string(),
                        endpoint_url: self.config.endpoint_url.clone(),
                        model: self.config.model.clone(),
                        cache_entries,
                        last_run_ms: now_ms(),
                        error: Some(error.to_string()),
                        ..Default::default()
                    },
                );
            }
            sleep_until_shutdown(Duration::from_millis(self.config.interval_ms), &shutdown);
        }
        info!("term embedding worker stopped");
    }

    fn run_once(&self) -> Result<()> {
        let corrections = personal_corrections::load_store(&self.corrections_path)?;
        let suspect_book = suspect_terms::load_book(&self.suspect_path)?;
        let history = history::load_recent(&self.history_path, self.config.history_limit)
            .with_context(|| format!("load history {}", self.history_path.display()))?;
        let mut cache = load_cache(&self.cache_path)?;
        let planned = build_work_items(
            &corrections.rules,
            &suspect_book.items,
            &history,
            self.config.max_context_chars,
        );
        let mut pruned_entries = 0usize;
        if self.config.prune_inactive_cache {
            pruned_entries = prune_cache(&mut cache, &planned, &self.config.model);
        }
        let pending = pending_items(&planned, &cache, &self.config.model)
            .into_iter()
            .take(self.config.max_items_per_run.max(1))
            .collect::<Vec<_>>();
        if pending.is_empty() {
            if pruned_entries > 0 {
                save_cache(&self.cache_path, &cache)?;
            }
            let family_index = build_family_index(
                &cache,
                &self.config.model,
                self.config.family_min_variants,
                self.config.family_similarity_threshold,
            );
            let family_count = family_index.families.len();
            save_family_index(&self.families_path, &family_index)?;
            let hotword_export =
                build_hotword_export(&corrections.rules, self.config.max_hotword_terms);
            let hotword_count = hotword_export.terms.len();
            save_hotword_export(&self.hotwords_path, &hotword_export)?;
            write_status(
                &self.status_path,
                &TermEmbeddingStatus {
                    ok: true,
                    phase: "idle".to_string(),
                    endpoint_url: self.config.endpoint_url.clone(),
                    model: self.config.model.clone(),
                    cache_entries: cache.entries.len(),
                    active_cache_entries: active_cache_entries(&cache, &self.config.model),
                    planned_items: planned.len(),
                    pending_items: 0,
                    pruned_entries,
                    family_count,
                    hotword_count,
                    last_run_ms: now_ms(),
                    ..Default::default()
                },
            )?;
            return Ok(());
        }
        let (model, vectors) = self.call_embeddings(&pending)?;
        let embedded_items = pending.len();
        merge_vectors(&mut cache, &pending, &model, vectors)?;
        save_cache(&self.cache_path, &cache)?;
        let family_index = build_family_index(
            &cache,
            &self.config.model,
            self.config.family_min_variants,
            self.config.family_similarity_threshold,
        );
        let family_count = family_index.families.len();
        save_family_index(&self.families_path, &family_index)?;
        let hotword_export =
            build_hotword_export(&corrections.rules, self.config.max_hotword_terms);
        let hotword_count = hotword_export.terms.len();
        save_hotword_export(&self.hotwords_path, &hotword_export)?;
        write_status(
            &self.status_path,
            &TermEmbeddingStatus {
                ok: true,
                phase: "embedded".to_string(),
                endpoint_url: self.config.endpoint_url.clone(),
                model,
                cache_entries: cache.entries.len(),
                active_cache_entries: active_cache_entries(&cache, &self.config.model),
                planned_items: planned.len(),
                embedded_items,
                pending_items: pending_items(&planned, &cache, &self.config.model).len(),
                pruned_entries,
                family_count,
                hotword_count,
                last_run_ms: now_ms(),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    fn call_embeddings(&self, items: &[EmbeddingWorkItem]) -> Result<(String, Vec<Vec<f32>>)> {
        let endpoint = self.config.endpoint_url.trim();
        if endpoint.is_empty() {
            bail!("term_embeddings.endpoint_url is empty");
        }
        let inputs = items
            .iter()
            .map(|item| item.input_text.as_str())
            .collect::<Vec<_>>();
        let mut last_error = None;
        for model in models_to_try(&self.config.model, &self.config.fallback_models) {
            let mut request = self.http.post(endpoint).json(&json!({
                "model": model,
                "input": inputs,
                "input_type": "passage",
            }));
            if let Some(api_key) = &self.api_key {
                request = request.bearer_auth(api_key);
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error.to_string());
                    continue;
                }
            };
            let response = match response.error_for_status() {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error.to_string());
                    continue;
                }
            };
            let decoded = response
                .json::<EmbeddingsResponse>()
                .context("decode embeddings response")?;
            let mut vectors = vec![Vec::<f32>::new(); items.len()];
            for data in decoded.data {
                if data.index < vectors.len() {
                    vectors[data.index] = data.embedding;
                }
            }
            if vectors.iter().any(|vector| vector.is_empty()) {
                last_error = Some("embedding response missing vectors".to_string());
                continue;
            }
            return Ok((decoded.model.unwrap_or(model), vectors));
        }
        Err(anyhow!(
            "embedding model calls failed: {}",
            last_error.unwrap_or_else(|| "unknown error".to_string())
        ))
    }
}

fn build_work_items(
    corrections: &[PersonalCorrectionRule],
    suspects: &[SuspectTermItem],
    history: &[HistoryRecord],
    max_context_chars: usize,
) -> Vec<EmbeddingWorkItem> {
    let mut items = BTreeMap::<String, EmbeddingWorkItem>::new();
    for rule in corrections.iter().filter(|rule| {
        rule.enabled
            && !rule.wrong.trim().is_empty()
            && !rule.correct.trim().is_empty()
            && rule.wrong != rule.correct
    }) {
        let input = trim_chars(
            &format!(
                "标准术语: {}\n误识别: {}\n来源: {}",
                rule.correct, rule.wrong, rule.source
            ),
            max_context_chars,
        );
        add_work_item(
            &mut items,
            rule.correct.trim(),
            rule.wrong.trim(),
            "personal_correction",
            &input,
        );
    }
    for item in suspects.iter().filter(|item| {
        item.status != "dismissed"
            && !item.wrong.trim().is_empty()
            && !item.suggested.trim().is_empty()
            && item.wrong != item.suggested
    }) {
        let examples = item.examples.join(" | ");
        let input = trim_chars(
            &format!(
                "候选术语: {}\n误识别: {}\n原因: {}\n例子: {}",
                item.suggested, item.wrong, item.reason, examples
            ),
            max_context_chars,
        );
        add_work_item(
            &mut items,
            item.suggested.trim(),
            item.wrong.trim(),
            "suspect_term",
            &input,
        );
    }
    let terms = items
        .values()
        .flat_map(|item| [item.canonical.as_str(), item.variant.as_str()])
        .filter(|term| !term.trim().is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    for record in history.iter().rev() {
        let text = record.preview_text().trim();
        if text.is_empty() || !terms.iter().any(|term| text.contains(term)) {
            continue;
        }
        let canonical = terms
            .iter()
            .find(|term| text.contains(term.as_str()))
            .cloned()
            .unwrap_or_default();
        let input = trim_chars(
            &format!(
                "语音上下文: {}\n模式: {}\n动作: {}",
                text, record.mode, record.finalizer_actions
            ),
            max_context_chars,
        );
        add_work_item(&mut items, &canonical, text, "history_context", &input);
    }
    items.into_values().collect()
}

fn add_work_item(
    items: &mut BTreeMap<String, EmbeddingWorkItem>,
    canonical: &str,
    variant: &str,
    source: &str,
    input_text: &str,
) {
    let input_hash = stable_hash_hex(input_text);
    let id = stable_hash_hex(&format!("{source}\0{canonical}\0{variant}\0{input_hash}"));
    items
        .entry(id.clone())
        .or_insert_with(|| EmbeddingWorkItem {
            id,
            canonical: canonical.to_string(),
            variant: variant.to_string(),
            source: source.to_string(),
            input_hash,
            input_text: input_text.to_string(),
        });
}

fn pending_items(
    planned: &[EmbeddingWorkItem],
    cache: &TermEmbeddingCache,
    model: &str,
) -> Vec<EmbeddingWorkItem> {
    planned
        .iter()
        .filter(|item| {
            !cache.entries.iter().any(|entry| {
                entry.id == item.id
                    && entry.input_hash == item.input_hash
                    && entry.model == model
                    && !entry.embedding.is_empty()
            })
        })
        .cloned()
        .collect()
}

fn prune_cache(
    cache: &mut TermEmbeddingCache,
    planned: &[EmbeddingWorkItem],
    model: &str,
) -> usize {
    let planned_ids = planned
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    let before = cache.entries.len();
    cache.entries.retain(|entry| {
        entry.model == model
            && planned_ids.contains(entry.id.as_str())
            && !entry.embedding.is_empty()
            && entry.dimensions == entry.embedding.len()
    });
    let pruned = before.saturating_sub(cache.entries.len());
    if pruned > 0 {
        cache.updated_ms = now_ms();
    }
    pruned
}

fn active_cache_entries(cache: &TermEmbeddingCache, model: &str) -> usize {
    cache
        .entries
        .iter()
        .filter(|entry| entry.model == model && !entry.embedding.is_empty())
        .count()
}

fn merge_vectors(
    cache: &mut TermEmbeddingCache,
    items: &[EmbeddingWorkItem],
    model: &str,
    vectors: Vec<Vec<f32>>,
) -> Result<()> {
    if items.len() != vectors.len() {
        bail!("embedding vector count mismatch");
    }
    let now = now_ms();
    for (item, vector) in items.iter().zip(vectors.into_iter()) {
        let dimensions = vector.len();
        if let Some(entry) = cache.entries.iter_mut().find(|entry| entry.id == item.id) {
            entry.canonical = item.canonical.clone();
            entry.variant = item.variant.clone();
            entry.source = item.source.clone();
            entry.input_hash = item.input_hash.clone();
            entry.input_text = item.input_text.clone();
            entry.model = model.to_string();
            entry.dimensions = dimensions;
            entry.embedding = vector;
            entry.updated_ms = now;
            entry.last_error.clear();
        } else {
            cache.entries.push(TermEmbeddingEntry {
                id: item.id.clone(),
                canonical: item.canonical.clone(),
                variant: item.variant.clone(),
                source: item.source.clone(),
                input_hash: item.input_hash.clone(),
                input_text: item.input_text.clone(),
                model: model.to_string(),
                dimensions,
                embedding: vector,
                created_ms: now,
                updated_ms: now,
                last_error: String::new(),
            });
        }
    }
    cache.updated_ms = now;
    Ok(())
}

fn build_family_index(
    cache: &TermEmbeddingCache,
    model: &str,
    min_variants: usize,
    similarity_threshold: f32,
) -> TermFamilyIndex {
    let mut grouped = BTreeMap::<String, Vec<&TermEmbeddingEntry>>::new();
    for entry in cache.entries.iter().filter(|entry| {
        entry.model == model
            && !entry.embedding.is_empty()
            && !entry.canonical.trim().is_empty()
            && !entry.variant.trim().is_empty()
    }) {
        grouped
            .entry(entry.canonical.trim().to_string())
            .or_default()
            .push(entry);
    }

    let mut families = Vec::new();
    for (canonical, entries) in grouped {
        let mut by_variant = BTreeMap::<String, Vec<&TermEmbeddingEntry>>::new();
        let mut sources = BTreeSet::<String>::new();
        for entry in &entries {
            by_variant
                .entry(entry.variant.trim().to_string())
                .or_default()
                .push(*entry);
            if !entry.source.trim().is_empty() {
                sources.insert(entry.source.clone());
            }
        }
        let variants = by_variant
            .into_iter()
            .filter_map(|(variant, variant_entries)| {
                if variant == canonical {
                    return None;
                }
                let max_similarity = max_similarity_against_family(&variant_entries, &entries);
                let keep = max_similarity >= similarity_threshold
                    || variant_entries.iter().any(|entry| {
                        entry.source == "personal_correction" || entry.source == "suspect_term"
                    });
                if !keep {
                    return None;
                }
                let variant_sources = variant_entries
                    .iter()
                    .filter_map(|entry| {
                        let source = entry.source.trim();
                        (!source.is_empty()).then_some(source.to_string())
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                Some(TermFamilyVariant {
                    text: variant,
                    sources: variant_sources,
                    max_similarity,
                })
            })
            .collect::<Vec<_>>();
        if variants.len() < min_variants.max(1) {
            continue;
        }
        let max_similarity = variants
            .iter()
            .map(|variant| variant.max_similarity)
            .fold(0.0f32, f32::max);
        families.push(TermFamily {
            canonical,
            variants,
            sources: sources.into_iter().collect(),
            max_similarity,
        });
    }
    families.sort_by(|a, b| {
        b.variants
            .len()
            .cmp(&a.variants.len())
            .then_with(|| b.max_similarity.total_cmp(&a.max_similarity))
            .then_with(|| a.canonical.cmp(&b.canonical))
    });
    TermFamilyIndex {
        version: 1,
        updated_ms: now_ms(),
        model: model.to_string(),
        review_only: true,
        source_entries: active_cache_entries(cache, model),
        families,
    }
}

fn max_similarity_against_family(
    variant_entries: &[&TermEmbeddingEntry],
    family_entries: &[&TermEmbeddingEntry],
) -> f32 {
    let mut max_similarity = 0.0f32;
    for left in variant_entries {
        for right in family_entries {
            if left.id == right.id {
                continue;
            }
            max_similarity =
                max_similarity.max(cosine_similarity(&left.embedding, &right.embedding));
        }
    }
    max_similarity
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (a, b) in left.iter().zip(right.iter()) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        return 0.0;
    }
    dot / (left_norm.sqrt() * right_norm.sqrt())
}

fn build_hotword_export(rules: &[PersonalCorrectionRule], max_terms: usize) -> TermHotwordExport {
    let mut terms = BTreeSet::<String>::new();
    for rule in rules.iter().filter(|rule| {
        rule.enabled && !rule.correct.trim().is_empty() && rule.correct.trim().chars().count() >= 2
    }) {
        terms.insert(rule.correct.trim().to_string());
        if terms.len() >= max_terms.max(1) {
            break;
        }
    }
    TermHotwordExport {
        version: 1,
        updated_ms: now_ms(),
        source: "personal_corrections.enabled.correct".to_string(),
        terms: terms.into_iter().collect(),
    }
}

fn load_cache(path: &Path) -> Result<TermEmbeddingCache> {
    if !path.exists() {
        return Ok(TermEmbeddingCache::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read term embeddings {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(TermEmbeddingCache::default());
    }
    let mut cache: TermEmbeddingCache = serde_json::from_str(&raw)
        .with_context(|| format!("parse term embeddings {}", path.display()))?;
    cache
        .entries
        .retain(|entry| !entry.id.trim().is_empty() && !entry.input_hash.trim().is_empty());
    Ok(cache)
}

fn save_cache(path: &Path, cache: &TermEmbeddingCache) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create term embedding dir {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(cache).context("serialize term embeddings")?;
    std::fs::write(path, format!("{raw}\n"))
        .with_context(|| format!("write term embeddings {}", path.display()))
}

fn save_family_index(path: &Path, index: &TermFamilyIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create term family dir {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(index).context("serialize term family index")?;
    std::fs::write(path, format!("{raw}\n"))
        .with_context(|| format!("write term family index {}", path.display()))
}

fn save_hotword_export(path: &Path, export: &TermHotwordExport) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create term hotword dir {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(export).context("serialize term hotwords")?;
    std::fs::write(path, format!("{raw}\n"))
        .with_context(|| format!("write term hotwords {}", path.display()))
}

fn write_status(path: &Path, status: &TermEmbeddingStatus) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create term embedding status dir {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(status).context("serialize term embedding status")?;
    std::fs::write(path, format!("{raw}\n"))
        .with_context(|| format!("write term embedding status {}", path.display()))
}

fn models_to_try(primary: &str, fallback: &[String]) -> Vec<String> {
    let mut models = Vec::new();
    for model in std::iter::once(primary).chain(fallback.iter().map(String::as_str)) {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|existing| existing == model) {
            models.push(model.to_string());
        }
    }
    models
}

fn trim_chars(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(80);
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn stable_hash_hex(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn sleep_until_shutdown(duration: Duration, shutdown: &AtomicBool) {
    let step = Duration::from_millis(250);
    let mut elapsed = Duration::ZERO;
    while elapsed < duration && !shutdown.load(Ordering::Relaxed) {
        let sleep_for = (duration - elapsed).min(step);
        thread::sleep(sleep_for);
        elapsed += sleep_for;
    }
}

fn read_api_key(primary_env: &str) -> Option<String> {
    for name in [
        primary_env,
        "AINPUT_CLIPROXYAPI_KEY",
        "AINPUT_CLIPROXYAPI_8317_KEY",
    ] {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if let Ok(value) = std::env::var(name) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
        #[cfg(windows)]
        if let Some(value) = read_windows_user_env_var(name) {
            return Some(value);
        }
    }
    None
}

#[cfg(windows)]
fn read_windows_user_env_var(name: &str) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};

    if name.trim().is_empty() {
        return None;
    }
    let subkey = HSTRING::from("Environment");
    let value_name = HSTRING::from(name);
    let mut bytes = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer
        .iter()
        .position(|ch| *ch == 0)
        .unwrap_or(buffer.len());
    let value = String::from_utf16_lossy(&buffer[..len]).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::{
        EmbeddingWorkItem, TermEmbeddingCache, build_family_index, build_hotword_export,
        build_work_items, merge_vectors, pending_items, prune_cache,
    };
    use crate::history::HistoryRecord;
    use crate::personal_corrections::PersonalCorrectionRule;
    use crate::suspect_terms::SuspectTermItem;

    #[test]
    fn builds_embedding_items_from_corrections_and_context() {
        let corrections = vec![PersonalCorrectionRule {
            wrong: "军线系统".to_string(),
            correct: "均线系统".to_string(),
            enabled: true,
            source: "test".to_string(),
            ..Default::default()
        }];
        let suspects = vec![SuspectTermItem {
            wrong: "君线系统".to_string(),
            suggested: "均线系统".to_string(),
            reason: "交易术语同音".to_string(),
            examples: vec!["这个君线系统需要优化".to_string()],
            confidence: 0.9,
            ..Default::default()
        }];
        let mut history = HistoryRecord::new("utt", "streaming", "streaming_asr");
        history.pasted_text = "这个军线系统和K线策略有关".to_string();
        let items = build_work_items(&corrections, &suspects, &[history], 240);
        assert!(items.iter().any(|item| item.variant == "军线系统"));
        assert!(items.iter().any(|item| item.variant == "君线系统"));
        assert!(items.iter().any(|item| item.source == "history_context"));
    }

    #[test]
    fn pending_items_skip_cached_same_model_and_hash() {
        let item = EmbeddingWorkItem {
            id: "id1".to_string(),
            canonical: "均线系统".to_string(),
            variant: "军线系统".to_string(),
            source: "test".to_string(),
            input_hash: "hash1".to_string(),
            input_text: "input".to_string(),
        };
        let mut cache = TermEmbeddingCache::default();
        merge_vectors(
            &mut cache,
            std::slice::from_ref(&item),
            "embed-model",
            vec![vec![1.0, 2.0]],
        )
        .expect("merge");
        assert!(pending_items(std::slice::from_ref(&item), &cache, "embed-model").is_empty());
        assert_eq!(
            pending_items(std::slice::from_ref(&item), &cache, "other-model").len(),
            1
        );
    }

    #[test]
    fn prune_cache_removes_old_model_and_unplanned_entries() {
        let item = EmbeddingWorkItem {
            id: "id1".to_string(),
            canonical: "均线系统".to_string(),
            variant: "军线系统".to_string(),
            source: "test".to_string(),
            input_hash: "hash1".to_string(),
            input_text: "input".to_string(),
        };
        let stale = EmbeddingWorkItem {
            id: "stale".to_string(),
            canonical: "旧词".to_string(),
            variant: "旧错词".to_string(),
            source: "test".to_string(),
            input_hash: "hash2".to_string(),
            input_text: "old".to_string(),
        };
        let mut cache = TermEmbeddingCache::default();
        merge_vectors(
            &mut cache,
            &[item.clone(), stale.clone()],
            "active-model",
            vec![vec![1.0, 0.0], vec![0.0, 1.0]],
        )
        .expect("merge active");
        merge_vectors(
            &mut cache,
            std::slice::from_ref(&stale),
            "old-model",
            vec![vec![0.5, 0.5]],
        )
        .expect("merge old");

        let pruned = prune_cache(&mut cache, std::slice::from_ref(&item), "active-model");

        assert_eq!(pruned, 1);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.entries[0].id, item.id);
        assert_eq!(cache.entries[0].model, "active-model");
    }

    #[test]
    fn builds_review_only_family_index_from_active_model() {
        let item1 = EmbeddingWorkItem {
            id: "id1".to_string(),
            canonical: "均线系统".to_string(),
            variant: "军线系统".to_string(),
            source: "personal_correction".to_string(),
            input_hash: "hash1".to_string(),
            input_text: "标准术语: 均线系统\n误识别: 军线系统".to_string(),
        };
        let item2 = EmbeddingWorkItem {
            id: "id2".to_string(),
            canonical: "均线系统".to_string(),
            variant: "君线系统".to_string(),
            source: "suspect_term".to_string(),
            input_hash: "hash2".to_string(),
            input_text: "候选术语: 均线系统\n误识别: 君线系统".to_string(),
        };
        let mut cache = TermEmbeddingCache::default();
        merge_vectors(
            &mut cache,
            &[item1, item2],
            "active-model",
            vec![vec![1.0, 0.0], vec![0.95, 0.05]],
        )
        .expect("merge");

        let index = build_family_index(&cache, "active-model", 2, 0.7);

        assert!(index.review_only);
        assert_eq!(index.families.len(), 1);
        assert_eq!(index.families[0].canonical, "均线系统");
        assert_eq!(index.families[0].variants.len(), 2);
    }

    #[test]
    fn exports_enabled_correction_targets_as_hotwords() {
        let rules = vec![
            PersonalCorrectionRule {
                wrong: "军线系统".to_string(),
                correct: "均线系统".to_string(),
                enabled: true,
                source: "test".to_string(),
                ..Default::default()
            },
            PersonalCorrectionRule {
                wrong: "升".to_string(),
                correct: "生".to_string(),
                enabled: true,
                source: "test".to_string(),
                ..Default::default()
            },
        ];

        let export = build_hotword_export(&rules, 16);

        assert_eq!(export.terms, vec!["均线系统".to_string()]);
    }
}
