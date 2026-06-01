use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const LEGACY_AUTO_SECTION_MARKER: &str = "# --- auto learned mappings ---";
const DEFAULT_AUTO_ACTIVATE_THRESHOLD: u32 = 2;

#[derive(Debug, Clone)]
pub struct TermAssetPaths {
    pub builtin_terms_file: PathBuf,
    pub user_terms_file: PathBuf,
    pub learning_state_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct TermCatalog {
    paths: TermAssetPaths,
    builtin: BuiltinTermsFile,
    user: UserTermsFile,
    learning: LearningStateFile,
}

#[derive(Debug, Clone)]
pub struct LearningRecordOutcome {
    pub spoken: String,
    pub canonical: String,
    pub count: u32,
    pub status: LearningStatus,
    pub auto_activated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LearningStatus {
    Candidate,
    Active,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinTermsFile {
    pub version: u32,
    pub description: String,
    pub terms: Vec<TermAliasEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTermsFile {
    pub version: u32,
    pub glossary: Vec<String>,
    pub aliases: Vec<TermAliasEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningStateFile {
    pub version: u32,
    pub entries: Vec<LearningEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermAliasEntry {
    pub spoken: Vec<String>,
    pub canonical: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEntry {
    pub spoken: String,
    pub canonical: String,
    pub count: u32,
    pub status: LearningStatus,
    pub last_updated_ms: u64,
}

impl TermAssetPaths {
    pub fn discover(root_dir: &Path) -> Self {
        let terms_dir = root_dir.join("data").join("terms");
        Self {
            builtin_terms_file: terms_dir.join("base_terms.json"),
            user_terms_file: terms_dir.join("user_terms.json"),
            learning_state_file: terms_dir.join("learned_terms.json"),
        }
    }
}

impl TermCatalog {
    pub fn load(root_dir: &Path) -> Result<Self> {
        let paths = TermAssetPaths::discover(root_dir);
        ensure_builtin_terms_file(&paths.builtin_terms_file)?;
        let user = ensure_user_terms_file(&paths.user_terms_file, root_dir)?;
        let learning = ensure_learning_state_file(&paths.learning_state_file)?;
        let builtin = read_json::<BuiltinTermsFile>(&paths.builtin_terms_file)?;

        Ok(Self {
            paths,
            builtin,
            user,
            learning,
        })
    }

    pub fn builtin_terms_path(&self) -> &Path {
        &self.paths.builtin_terms_file
    }

    pub fn user_terms_path(&self) -> &Path {
        &self.paths.user_terms_file
    }

    pub fn learning_state_path(&self) -> &Path {
        &self.paths.learning_state_file
    }

    pub fn latest_learning_entries(&self, limit: usize) -> Vec<LearningEntry> {
        let mut entries = self.learning.entries.clone();
        entries.sort_by(|left, right| right.last_updated_ms.cmp(&left.last_updated_ms));
        entries.into_iter().take(limit).collect()
    }

    pub fn apply_to_text(&self, text: &str) -> String {
        let exact_aliases = self.build_exact_alias_map();
        let glossary = self.build_glossary();
        correct_ascii_terms(text, &exact_aliases, &glossary)
    }

    pub fn record_recent_correction(
        &mut self,
        original_text: &str,
        corrected_text: &str,
        auto_activate_threshold: Option<u32>,
    ) -> Result<Option<LearningRecordOutcome>> {
        let Some((spoken, canonical)) = infer_single_word_correction(original_text, corrected_text)
        else {
            return Ok(None);
        };

        let threshold = auto_activate_threshold.unwrap_or(DEFAULT_AUTO_ACTIVATE_THRESHOLD);
        ensure_glossary_term(&mut self.user.glossary, &canonical);

        let now_ms = now_unix_ms();
        if let Some(entry) = self
            .learning
            .entries
            .iter_mut()
            .find(|entry| entry.spoken.eq_ignore_ascii_case(&spoken))
        {
            entry.canonical = canonical.clone();
            entry.count += 1;
            entry.last_updated_ms = now_ms;
            let mut auto_activated = false;
            if entry.status == LearningStatus::Candidate && entry.count >= threshold {
                entry.status = LearningStatus::Active;
                auto_activated = true;
            }
            let count = entry.count;
            let status = entry.status;
            self.save()?;
            return Ok(Some(LearningRecordOutcome {
                spoken,
                canonical,
                count,
                status,
                auto_activated,
            }));
        }

        let status = if threshold <= 1 {
            LearningStatus::Active
        } else {
            LearningStatus::Candidate
        };
        self.learning.entries.push(LearningEntry {
            spoken: spoken.clone(),
            canonical: canonical.clone(),
            count: 1,
            status,
            last_updated_ms: now_ms,
        });
        self.save()?;

        Ok(Some(LearningRecordOutcome {
            spoken,
            canonical,
            count: 1,
            status,
            auto_activated: status == LearningStatus::Active,
        }))
    }

    pub fn save(&self) -> Result<()> {
        write_json(self.user_terms_path(), &self.user)?;
        write_json(self.learning_state_path(), &self.learning)?;
        Ok(())
    }

    fn build_exact_alias_map(&self) -> HashMap<String, String> {
        let mut aliases = HashMap::new();

        for entry in &self.builtin.terms {
            for spoken in &entry.spoken {
                aliases.insert(spoken.to_ascii_lowercase(), entry.canonical.clone());
            }
        }

        for entry in &self.user.aliases {
            for spoken in &entry.spoken {
                aliases.insert(spoken.to_ascii_lowercase(), entry.canonical.clone());
            }
        }

        for entry in &self.learning.entries {
            if entry.status == LearningStatus::Active {
                aliases.insert(entry.spoken.to_ascii_lowercase(), entry.canonical.clone());
            }
        }

        aliases
    }

    fn build_glossary(&self) -> Vec<String> {
        let mut glossary = Vec::new();

        for entry in &self.builtin.terms {
            glossary.push(entry.canonical.clone());
        }
        glossary.extend(self.user.glossary.iter().cloned());
        for entry in &self.learning.entries {
            if entry.status == LearningStatus::Active {
                glossary.push(entry.canonical.clone());
            }
        }

        glossary.sort_by_key(|entry| entry.to_ascii_lowercase());
        glossary.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        glossary
    }
}

impl Default for UserTermsFile {
    fn default() -> Self {
        Self {
            version: 1,
            glossary: Vec::new(),
            aliases: Vec::new(),
        }
    }
}

impl Default for LearningStateFile {
    fn default() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
        }
    }
}

fn ensure_builtin_terms_file(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create built-in terms directory {}", parent.display()))?;
    }

    let builtin = default_builtin_terms();
    write_json(path, &builtin)
}

fn ensure_user_terms_file(path: &Path, root_dir: &Path) -> Result<UserTermsFile> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create user terms directory {}", parent.display()))?;
    }

    if path.exists() {
        return read_json(path);
    }

    let document = if legacy_user_terms_json_path(root_dir).exists() {
        migrate_legacy_json_terms(&legacy_user_terms_json_path(root_dir))?
    } else if legacy_user_terms_text_path(root_dir).exists() {
        migrate_legacy_text_terms(&legacy_user_terms_text_path(root_dir))?
    } else {
        UserTermsFile::default()
    };

    write_json(path, &document)?;
    Ok(document)
}

fn ensure_learning_state_file(path: &Path) -> Result<LearningStateFile> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create learning state directory {}", parent.display()))?;
    }

    if path.exists() {
        return read_json(path);
    }

    let learning = LearningStateFile::default();
    write_json(path, &learning)?;
    Ok(learning)
}

fn read_json<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let raw =
        fs::read_to_string(path).with_context(|| format!("read JSON file {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse JSON file {}", path.display()))
}

fn write_json<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    let payload = serde_json::to_string_pretty(value)
        .with_context(|| format!("serialize {}", path.display()))?;
    fs::write(path, payload).with_context(|| format!("write JSON file {}", path.display()))
}

fn legacy_user_terms_json_path(root_dir: &Path) -> PathBuf {
    root_dir.join("data").join("terms").join("user_terms.json")
}

fn legacy_user_terms_text_path(root_dir: &Path) -> PathBuf {
    root_dir.join("data").join("terms").join("user_terms.txt")
}

fn migrate_legacy_json_terms(path: &Path) -> Result<UserTermsFile> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read legacy user terms file {}", path.display()))?;

    if let Ok(document) = serde_json::from_str::<UserTermsFile>(&raw) {
        return Ok(document);
    }

    let mut glossary = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim().trim_end_matches(',');
        if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
            let entry = trimmed.trim_matches('"');
            if !entry.is_empty()
                && entry != "version"
                && entry != "notes"
                && entry != "glossary"
                && entry != "mappings"
            {
                glossary.push(entry.to_string());
            }
        }
    }

    glossary.sort_by_key(|entry| entry.to_ascii_lowercase());
    glossary.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

    Ok(UserTermsFile {
        version: 1,
        glossary,
        aliases: Vec::new(),
    })
}

fn migrate_legacy_text_terms(path: &Path) -> Result<UserTermsFile> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read legacy user terms text file {}", path.display()))?;
    let mut glossary = Vec::new();
    let mut aliases = Vec::new();
    let mut in_auto_section = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == LEGACY_AUTO_SECTION_MARKER {
            in_auto_section = true;
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }

        if in_auto_section {
            if let Some((spoken, canonical)) = parse_legacy_mapping_line(trimmed) {
                aliases.push(TermAliasEntry {
                    spoken: vec![spoken],
                    canonical,
                });
            }
        } else {
            glossary.push(trimmed.to_string());
        }
    }

    glossary.sort_by_key(|entry| entry.to_ascii_lowercase());
    glossary.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

    Ok(UserTermsFile {
        version: 1,
        glossary,
        aliases,
    })
}

fn parse_legacy_mapping_line(line: &str) -> Option<(String, String)> {
    let (left, right) = line.split_once("=>")?;
    let spoken = left.trim().to_ascii_lowercase();
    let canonical = if let Some((canonical, _)) = right.split_once('|') {
        canonical.trim().to_string()
    } else {
        right.trim().to_string()
    };

    if spoken.is_empty() || canonical.is_empty() {
        return None;
    }

    Some((spoken, canonical))
}

fn default_builtin_terms() -> BuiltinTermsFile {
    BuiltinTermsFile {
        version: 2,
        description: "ainput built-in AI coding terms".to_string(),
        terms: vec![
            alias(&["codex", "code x"], "Codex"),
            alias(&["codex cli", "condex cli", "code x cli"], "Codex CLI"),
            alias(&["codex ci", "condex ci", "code x ci"], "Codex CI"),
            alias(&["open ai", "openai"], "OpenAI"),
            alias(&["chat gpt", "chatgpt"], "ChatGPT"),
            alias(&["gpt five", "g p t 5", "gpt 5"], "GPT-5"),
            alias(&["gpt four point one", "gpt 4.1", "g p t 4.1"], "GPT-4.1"),
            alias(&["gpt 4o", "gpt4o", "gpt-4o", "g p t 4 o"], "GPT-4o"),
            alias(&["claude"], "Claude"),
            alias(
                &["claude opus", "claude ops", "cloud ops", "cloud opus"],
                "Claude Opus",
            ),
            alias(&["anthropic"], "Anthropic"),
            alias(&["gemini"], "Gemini"),
            alias(&["google gemini", "google germany"], "Google Gemini"),
            alias(&["cursor"], "Cursor"),
            alias(&["windsurf"], "Windsurf"),
            alias(&["aider"], "Aider"),
            alias(&["copilot", "github copilot"], "Copilot"),
            alias(&["repo", "repository"], "repo"),
            alias(&["pull request", "p r"], "PR"),
            alias(&["issue"], "issue"),
            alias(&["work tree", "worktree"], "worktree"),
            alias(&["branch"], "branch"),
            alias(&["commit"], "commit"),
            alias(&["diff"], "diff"),
            alias(&["refactor"], "refactor"),
            alias(&["lint", "linter"], "lint"),
            alias(&["clippy"], "clippy"),
            alias(&["format", "formatter"], "format"),
            alias(&["cargo"], "cargo"),
            alias(&["rust"], "Rust"),
            alias(&["typescript", "type script"], "TypeScript"),
            alias(&["javascript", "java script"], "JavaScript"),
            alias(&["python"], "Python"),
            alias(&["powershell", "power shell"], "PowerShell"),
            alias(&["terminal"], "terminal"),
            alias(&["cli", "c l i"], "CLI"),
            alias(&["api", "a p i"], "API"),
            alias(&["sdk", "s d k"], "SDK"),
            alias(&["json", "j s o n"], "JSON"),
            alias(&["toml", "t o m l"], "TOML"),
            alias(&["yaml", "y a m l"], "YAML"),
            alias(&["prompt"], "prompt"),
            alias(&["system prompt"], "system prompt"),
            alias(&["context"], "context"),
            alias(&["token"], "token"),
            alias(&["embedding"], "embedding"),
            alias(&["rag", "r a g"], "RAG"),
            alias(&["agent"], "agent"),
            alias(&["workflow"], "workflow"),
            alias(&["spec"], "spec"),
            alias(&["latency"], "latency"),
            alias(&["throughput"], "throughput"),
            alias(&["trace back", "traceback"], "traceback"),
            alias(&["fast api", "fastapi"], "FastAPI"),
            alias(&["tail scale", "tailscale"], "Tailscale"),
            alias(&["windows terminal"], "Windows Terminal"),
            alias(&["visual studio code", "vs code"], "VS Code"),
            alias(&["sherpa onnx", "sherpa-onnx"], "sherpa-onnx"),
            alias(&["sense voice", "sensevoice"], "SenseVoice"),
        ],
    }
}

fn alias(spoken: &[&str], canonical: &str) -> TermAliasEntry {
    TermAliasEntry {
        spoken: spoken.iter().map(|item| item.to_string()).collect(),
        canonical: canonical.to_string(),
    }
}

fn ensure_glossary_term(glossary: &mut Vec<String>, canonical: &str) {
    if !glossary
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(canonical))
    {
        glossary.push(canonical.to_string());
        glossary.sort_by_key(|entry| entry.to_ascii_lowercase());
        glossary.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    }
}

fn infer_single_word_correction(
    original_text: &str,
    corrected_text: &str,
) -> Option<(String, String)> {
    let original_tokens = extract_ascii_word_tokens(original_text);
    let corrected_tokens = extract_ascii_word_tokens(corrected_text);

    if original_tokens.len() != corrected_tokens.len() {
        return None;
    }

    let mut diff: Option<(String, String)> = None;
    for (original, corrected) in original_tokens.iter().zip(corrected_tokens.iter()) {
        if original.eq_ignore_ascii_case(corrected) {
            continue;
        }
        if diff.is_some() {
            return None;
        }
        diff = Some((original.to_ascii_lowercase(), corrected.clone()));
    }

    diff.filter(|(spoken, canonical)| spoken != &canonical.to_ascii_lowercase())
}

fn extract_ascii_word_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if is_term_char(chars[index]) {
            let start = index;
            while index < chars.len() && is_term_char(chars[index]) {
                index += 1;
            }
            let token: String = chars[start..index].iter().collect();
            if token.is_ascii() && token.chars().count() >= 2 {
                tokens.push(token);
            }
        } else {
            index += 1;
        }
    }

    tokens
}

fn correct_ascii_terms(
    text: &str,
    exact_aliases: &HashMap<String, String>,
    glossary: &[String],
) -> String {
    let normalized = apply_phrase_aliases(text, exact_aliases);
    let mut result = String::with_capacity(normalized.len());
    let chars: Vec<char> = normalized.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if is_term_char(chars[index]) {
            let start = index;
            while index < chars.len() && is_term_char(chars[index]) {
                index += 1;
            }

            let token: String = chars[start..index].iter().collect();
            let replacement = correct_ascii_token(&token, exact_aliases, glossary).unwrap_or(token);
            result.push_str(&replacement);
        } else {
            result.push(chars[index]);
            index += 1;
        }
    }

    result
}

fn apply_phrase_aliases(text: &str, exact_aliases: &HashMap<String, String>) -> String {
    let mut current = text.to_string();

    for (spoken, canonical) in exact_aliases
        .iter()
        .filter(|(spoken, _)| spoken.chars().any(|ch| !is_term_char(ch)))
    {
        current = replace_case_insensitive_phrase(&current, spoken, canonical);
    }

    current
}

fn replace_case_insensitive_phrase(text: &str, needle: &str, replacement: &str) -> String {
    let lower_text = text.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut byte_index = 0usize;

    while byte_index < text.len() {
        let remaining = &lower_text[byte_index..];
        if remaining.starts_with(&lower_needle)
            && phrase_boundary_ok(&chars, text, byte_index, needle.len())
        {
            result.push_str(replacement);
            byte_index += needle.len();
            continue;
        }

        let next_char = text[byte_index..]
            .chars()
            .next()
            .expect("current byte index should be on char boundary");
        result.push(next_char);
        byte_index += next_char.len_utf8();
    }

    result
}

fn phrase_boundary_ok(chars: &[char], text: &str, start: usize, len: usize) -> bool {
    let start_char_index = text[..start].chars().count();
    let end_char_index = text[..start + len].chars().count();
    let before = start_char_index
        .checked_sub(1)
        .and_then(|index| chars.get(index))
        .copied();
    let after = chars.get(end_char_index).copied();

    before.map(|ch| !is_term_char(ch)).unwrap_or(true)
        && after.map(|ch| !is_term_char(ch)).unwrap_or(true)
}

fn correct_ascii_token(
    token: &str,
    exact_aliases: &HashMap<String, String>,
    glossary: &[String],
) -> Option<String> {
    if !token.is_ascii() {
        return None;
    }

    let lowered = token.to_ascii_lowercase();
    if let Some(replacement) = exact_aliases.get(&lowered) {
        return Some(replacement.clone());
    }

    if token.chars().count() < 4 {
        return None;
    }

    for canonical in glossary {
        if canonical.eq_ignore_ascii_case(token) {
            return Some(canonical.clone());
        }
    }

    let mut best: Option<(&str, usize)> = None;
    for canonical in glossary {
        if !canonical.is_ascii() || canonical.chars().count() < 4 {
            continue;
        }

        let distance = levenshtein(&lowered, &canonical.to_ascii_lowercase());
        if distance > 2 {
            continue;
        }

        match best {
            Some((_, best_distance)) if distance >= best_distance => {}
            _ => best = Some((canonical.as_str(), distance)),
        }
    }

    best.map(|(canonical, _)| canonical.to_string())
}

fn is_term_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+')
}

fn levenshtein(left: &str, right: &str) -> usize {
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();

    let mut prev: Vec<usize> = (0..=right_chars.len()).collect();
    let mut curr = vec![0usize; right_chars.len() + 1];

    for (i, left_char) in left_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = if left_char == right_char { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1)
                .min(curr[j] + 1)
                .min(prev[j] + substitution_cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[right_chars.len()]
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{
        LearningStateFile, LearningStatus, TermCatalog, UserTermsFile, correct_ascii_terms,
        default_builtin_terms,
    };
    use std::collections::HashMap;

    #[test]
    fn builtin_terms_cover_ai_words() {
        let builtin = default_builtin_terms();
        assert!(builtin.terms.iter().any(|entry| entry.canonical == "Codex"));
        assert!(
            builtin
                .terms
                .iter()
                .any(|entry| entry.canonical == "OpenAI")
        );
    }

    #[test]
    fn exact_aliases_win_first() {
        let mut aliases = HashMap::new();
        aliases.insert("open ai".to_string(), "OpenAI".to_string());
        let glossary = vec!["OpenAI".to_string()];
        assert_eq!(
            correct_ascii_terms("please ask open ai", &aliases, &glossary),
            "please ask OpenAI"
        );
    }

    #[test]
    fn exact_aliases_apply_to_short_ascii_terms_and_brand_phrases() {
        let mut aliases = HashMap::new();
        aliases.insert("cli".to_string(), "CLI".to_string());
        aliases.insert("google germany".to_string(), "Google Gemini".to_string());
        let glossary = vec!["CLI".to_string(), "Google Gemini".to_string()];
        assert_eq!(
            correct_ascii_terms("run cli now", &aliases, &glossary),
            "run CLI now"
        );
        assert_eq!(
            correct_ascii_terms("open google germany please", &aliases, &glossary),
            "open Google Gemini please"
        );
    }

    #[test]
    fn fuzzy_glossary_correction_still_works() {
        let glossary = vec!["Codex".to_string()];
        assert_eq!(
            correct_ascii_terms("launch codax please", &HashMap::new(), &glossary),
            "launch Codex please"
        );
    }

    #[test]
    fn active_learning_entries_apply() {
        let root = std::env::temp_dir().join(format!("ainput-data-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data").join("terms")).unwrap();
        let builtin = default_builtin_terms();
        std::fs::write(
            root.join("data").join("terms").join("base_terms.json"),
            serde_json::to_string_pretty(&builtin).unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("data").join("terms").join("user_terms.json"),
            serde_json::to_string_pretty(&UserTermsFile::default()).unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("data").join("terms").join("learned_terms.json"),
            serde_json::to_string_pretty(&LearningStateFile {
                version: 1,
                entries: vec![super::LearningEntry {
                    spoken: "scale".to_string(),
                    canonical: "skill".to_string(),
                    count: 2,
                    status: LearningStatus::Active,
                    last_updated_ms: 1,
                }],
            })
            .unwrap(),
        )
        .unwrap();

        let catalog = TermCatalog::load(&root).unwrap();
        assert_eq!(catalog.apply_to_text("add scale here"), "add skill here");
        let _ = std::fs::remove_dir_all(&root);
    }
}
