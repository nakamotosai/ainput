use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

static CORRECTIONS_PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PersonalCorrectionStore {
    pub version: u32,
    pub rules: Vec<PersonalCorrectionRule>,
    pub protected_replacements: Vec<ProtectedReplacementRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PersonalCorrectionRule {
    pub wrong: String,
    pub correct: String,
    pub enabled: bool,
    pub source: String,
    pub created_ms: u128,
    pub updated_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProtectedReplacementRule {
    pub raw: String,
    pub forbidden: String,
    pub enabled: bool,
    pub source: String,
    pub created_ms: u128,
    pub updated_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteGuardDecision {
    pub blocked: bool,
    pub reason: String,
}

impl Default for PersonalCorrectionRule {
    fn default() -> Self {
        Self {
            wrong: String::new(),
            correct: String::new(),
            enabled: true,
            source: String::new(),
            created_ms: now_ms(),
            updated_ms: now_ms(),
        }
    }
}

impl Default for ProtectedReplacementRule {
    fn default() -> Self {
        Self {
            raw: String::new(),
            forbidden: String::new(),
            enabled: true,
            source: String::new(),
            created_ms: now_ms(),
            updated_ms: now_ms(),
        }
    }
}

impl Default for PersonalCorrectionStore {
    fn default() -> Self {
        Self {
            version: 1,
            rules: Vec::new(),
            protected_replacements: vec![ProtectedReplacementRule {
                raw: "搜索".to_string(),
                forbidden: "筛选".to_string(),
                source: "builtin".to_string(),
                ..Default::default()
            }],
        }
    }
}

pub fn init(path: PathBuf) {
    let _ = CORRECTIONS_PATH.set(path);
}

pub fn normalize_text(text: &str) -> String {
    let Some(path) = CORRECTIONS_PATH.get() else {
        return text.to_string();
    };
    match normalize_text_with_path(text, path) {
        Ok(value) => value,
        Err(error) => {
            warn!(error = %error, "personal correction normalization failed");
            text.to_string()
        }
    }
}

pub fn normalize_text_with_path(text: &str, path: &Path) -> Result<String> {
    let store = load_store(path)?;
    Ok(apply_rules(text, &store.rules))
}

pub fn guard_rewrite_output(raw_text: &str, rewrite_text: &str) -> Option<RewriteGuardDecision> {
    let path = CORRECTIONS_PATH.get()?;
    guard_rewrite_output_with_path(raw_text, rewrite_text, path)
        .ok()
        .flatten()
}

pub fn guard_rewrite_output_with_path(
    raw_text: &str,
    rewrite_text: &str,
    path: &Path,
) -> Result<Option<RewriteGuardDecision>> {
    let store = load_store(path)?;
    for rule in store.protected_replacements.iter().filter(|rule| {
        rule.enabled
            && !rule.raw.trim().is_empty()
            && !rule.forbidden.trim().is_empty()
            && rule.raw != rule.forbidden
    }) {
        if raw_text.contains(&rule.raw) && rewrite_text.contains(&rule.forbidden) {
            return Ok(Some(RewriteGuardDecision {
                blocked: true,
                reason: format!("protected_replacement:{}->{}", rule.raw, rule.forbidden),
            }));
        }
    }
    Ok(None)
}

pub fn append_or_update_rule(path: &Path, wrong: &str, correct: &str, source: &str) -> Result<()> {
    let wrong = wrong.trim();
    let correct = correct.trim();
    if wrong.is_empty() || correct.is_empty() || wrong == correct {
        return Ok(());
    }
    let mut store = load_store(path)?;
    let now = now_ms();
    if let Some(rule) = store
        .rules
        .iter_mut()
        .find(|rule| rule.wrong == wrong && rule.correct == correct)
    {
        rule.enabled =
            !conflicts_with_enabled_protection(wrong, correct, &store.protected_replacements);
        rule.updated_ms = now;
        if !source.trim().is_empty() {
            rule.source = source.trim().to_string();
        }
    } else {
        let enabled =
            !conflicts_with_enabled_protection(wrong, correct, &store.protected_replacements);
        store.rules.push(PersonalCorrectionRule {
            wrong: wrong.to_string(),
            correct: correct.to_string(),
            enabled,
            source: source.trim().to_string(),
            created_ms: now,
            updated_ms: now,
        });
    }
    save_store(path, &store)
}

pub fn append_or_update_protected_replacement(
    path: &Path,
    raw: &str,
    forbidden: &str,
    source: &str,
) -> Result<()> {
    let raw = raw.trim();
    let forbidden = forbidden.trim();
    if raw.is_empty() || forbidden.is_empty() || raw == forbidden {
        return Ok(());
    }
    let mut store = load_store(path)?;
    let now = now_ms();
    if let Some(rule) = store
        .protected_replacements
        .iter_mut()
        .find(|rule| rule.raw == raw && rule.forbidden == forbidden)
    {
        rule.enabled = true;
        rule.updated_ms = now;
        if !source.trim().is_empty() {
            rule.source = source.trim().to_string();
        }
    } else {
        store.protected_replacements.push(ProtectedReplacementRule {
            raw: raw.to_string(),
            forbidden: forbidden.to_string(),
            enabled: true,
            source: source.trim().to_string(),
            created_ms: now,
            updated_ms: now,
        });
    }
    disable_conflicting_rules_for_enabled_protections(&mut store);
    save_store(path, &store)
}

pub fn disable_matching_rules(path: &Path, wrong: &str, correct: Option<&str>) -> Result<usize> {
    let wrong = wrong.trim();
    let correct = correct.map(str::trim).unwrap_or_default();
    if wrong.is_empty() {
        return Ok(0);
    }
    let mut store = load_store(path)?;
    let now = now_ms();
    let mut changed = 0usize;
    for rule in &mut store.rules {
        let correct_matches = correct.is_empty() || rule.correct == correct;
        if rule.enabled && rule.wrong == wrong && correct_matches {
            rule.enabled = false;
            rule.updated_ms = now;
            changed += 1;
        }
    }
    if changed > 0 {
        save_store(path, &store)?;
    }
    Ok(changed)
}

pub fn set_rule_enabled(path: &Path, index: usize, enabled: bool) -> Result<bool> {
    let mut store = load_store(path)?;
    let Some(rule) = store.rules.get_mut(index) else {
        return Ok(false);
    };
    rule.enabled = enabled;
    rule.updated_ms = now_ms();
    save_store(path, &store)?;
    Ok(true)
}

pub fn delete_rule(path: &Path, index: usize) -> Result<bool> {
    let mut store = load_store(path)?;
    if index >= store.rules.len() {
        return Ok(false);
    }
    store.rules.remove(index);
    save_store(path, &store)?;
    Ok(true)
}

pub fn set_protected_enabled(path: &Path, index: usize, enabled: bool) -> Result<bool> {
    let mut store = load_store(path)?;
    let Some(rule) = store.protected_replacements.get_mut(index) else {
        return Ok(false);
    };
    rule.enabled = enabled;
    rule.updated_ms = now_ms();
    save_store(path, &store)?;
    Ok(true)
}

pub fn delete_protected(path: &Path, index: usize) -> Result<bool> {
    let mut store = load_store(path)?;
    if index >= store.protected_replacements.len() {
        return Ok(false);
    }
    store.protected_replacements.remove(index);
    save_store(path, &store)?;
    Ok(true)
}

pub fn load_store(path: &Path) -> Result<PersonalCorrectionStore> {
    if !path.exists() {
        return Ok(PersonalCorrectionStore::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read personal corrections {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(PersonalCorrectionStore::default());
    }
    let mut store: PersonalCorrectionStore = serde_json::from_str(&raw)
        .with_context(|| format!("parse personal corrections {}", path.display()))?;
    ensure_builtin_protected_replacements(&mut store);
    disable_conflicting_rules_for_enabled_protections(&mut store);
    Ok(store)
}

pub fn save_store(path: &Path, store: &PersonalCorrectionStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create personal corrections dir {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(store).context("serialize personal corrections")?;
    std::fs::write(path, raw)
        .with_context(|| format!("write personal corrections {}", path.display()))
}

fn apply_rules(text: &str, rules: &[PersonalCorrectionRule]) -> String {
    let mut enabled = rules
        .iter()
        .filter(|rule| {
            rule.enabled
                && !rule.wrong.trim().is_empty()
                && !rule.correct.trim().is_empty()
                && rule.wrong != rule.correct
                && is_safe_global_rule(rule)
        })
        .collect::<Vec<_>>();
    enabled.sort_by_key(|rule| std::cmp::Reverse(rule.wrong.chars().count()));
    let mut output = text.to_string();
    for rule in enabled {
        output = output.replace(&rule.wrong, &rule.correct);
    }
    output
}

fn is_safe_global_rule(rule: &PersonalCorrectionRule) -> bool {
    // One-character substring replacement is too broad for live voice input.
    // Keep the stored rule for review, but require a future context-aware path before applying it.
    rule.wrong.trim().chars().count() >= 2
}

fn ensure_builtin_protected_replacements(store: &mut PersonalCorrectionStore) {
    if !store
        .protected_replacements
        .iter()
        .any(|rule| rule.raw == "搜索" && rule.forbidden == "筛选")
    {
        store.protected_replacements.push(ProtectedReplacementRule {
            raw: "搜索".to_string(),
            forbidden: "筛选".to_string(),
            source: "builtin".to_string(),
            ..Default::default()
        });
    }
}

fn disable_conflicting_rules_for_enabled_protections(store: &mut PersonalCorrectionStore) {
    let protected = store.protected_replacements.clone();
    let now = now_ms();
    for rule in &mut store.rules {
        if rule.enabled && conflicts_with_enabled_protection(&rule.wrong, &rule.correct, &protected)
        {
            rule.enabled = false;
            rule.updated_ms = now;
        }
    }
}

fn conflicts_with_enabled_protection(
    wrong: &str,
    correct: &str,
    protected: &[ProtectedReplacementRule],
) -> bool {
    let wrong = wrong.trim();
    let correct = correct.trim();
    protected.iter().any(|rule| {
        rule.enabled
            && !rule.raw.trim().is_empty()
            && !rule.forbidden.trim().is_empty()
            && rule.raw.trim() == wrong
            && rule.forbidden.trim() == correct
    })
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
        append_or_update_protected_replacement, append_or_update_rule, disable_matching_rules,
        guard_rewrite_output_with_path, load_store, normalize_text_with_path,
    };

    #[test]
    fn applies_persisted_correction_rule() {
        let path = std::env::temp_dir().join(format!(
            "ainput-personal-corrections-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        append_or_update_rule(&path, "扣带", "Codex", "test").expect("append rule");
        assert_eq!(
            normalize_text_with_path("我在用扣带编程", &path).expect("normalize"),
            "我在用Codex编程"
        );
        let store = load_store(&path).expect("load store");
        let _ = std::fs::remove_file(&path);
        assert_eq!(store.rules.len(), 1);
        assert_eq!(store.rules[0].wrong, "扣带");
        assert_eq!(store.rules[0].correct, "Codex");
    }

    #[test]
    fn one_character_correction_is_not_applied_globally() {
        let path = std::env::temp_dir().join(format!(
            "ainput-one-char-correction-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        append_or_update_rule(&path, "升", "生", "test").expect("append rule");
        let normalized = normalize_text_with_path("升级和升黄图", &path).expect("normalize");
        assert_eq!(normalized, "升级和升黄图");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn can_disable_matching_correction_rule() {
        let path = std::env::temp_dir().join(format!(
            "ainput-personal-corrections-disable-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        append_or_update_rule(&path, "收购这个项目", "收录这个项目", "test").expect("append first");
        append_or_update_rule(&path, "收购这个项目", "收口这个项目", "test")
            .expect("append second");
        let disabled =
            disable_matching_rules(&path, "收购这个项目", Some("收录这个项目")).expect("disable");
        let store = load_store(&path).expect("load store");
        let _ = std::fs::remove_file(&path);
        assert_eq!(disabled, 1);
        assert!(!store.rules[0].enabled);
        assert!(store.rules[1].enabled);
    }

    #[test]
    fn protected_replacement_blocks_specific_ai_rewrite() {
        let path = std::env::temp_dir().join(format!(
            "ainput-protected-rewrite-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        append_or_update_protected_replacement(&path, "搜索", "筛选", "test")
            .expect("append protected replacement");
        let decision = guard_rewrite_output_with_path("请搜索这个项目", "请筛选这个项目", &path)
            .expect("guard")
            .expect("blocked");
        let _ = std::fs::remove_file(&path);
        assert!(decision.blocked);
        assert!(decision.reason.contains("搜索->筛选"));
    }

    #[test]
    fn protected_replacement_disables_conflicting_personal_correction() {
        let path = std::env::temp_dir().join(format!(
            "ainput-protected-disables-correction-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        append_or_update_rule(&path, "搜索", "筛选", "test").expect("append rule");
        append_or_update_protected_replacement(&path, "搜索", "筛选", "test")
            .expect("append protected replacement");
        let normalized = normalize_text_with_path("请搜索这个项目", &path).expect("normalize");
        let store = load_store(&path).expect("load store");
        let _ = std::fs::remove_file(&path);
        assert_eq!(normalized, "请搜索这个项目");
        assert!(!store.rules[0].enabled);
    }

    #[test]
    fn cannot_reenable_correction_that_conflicts_with_enabled_protection() {
        let path = std::env::temp_dir().join(format!(
            "ainput-protected-rejects-correction-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        append_or_update_protected_replacement(&path, "搜索", "筛选", "test")
            .expect("append protected replacement");
        append_or_update_rule(&path, "搜索", "筛选", "test").expect("append rule");
        let store = load_store(&path).expect("load store");
        let _ = std::fs::remove_file(&path);
        assert_eq!(store.rules.len(), 1);
        assert!(!store.rules[0].enabled);
    }

    #[test]
    fn protected_replacement_does_not_change_real_forbidden_word() {
        let path = std::env::temp_dir().join(format!(
            "ainput-protected-rewrite-allowed-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        append_or_update_protected_replacement(&path, "搜索", "筛选", "test")
            .expect("append protected replacement");
        let decision = guard_rewrite_output_with_path("请筛选这些内容", "请筛选这些内容。", &path)
            .expect("guard");
        let _ = std::fs::remove_file(&path);
        assert!(decision.is_none());
    }
}
