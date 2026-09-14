use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sherpa_onnx::{
    OfflinePunctuation, OfflinePunctuationConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSenseVoiceModelConfig,
};
use tracing::{info, warn};

use crate::config::LocalNonstreamingConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEngine {
    SenseVoice,
}

impl LocalEngine {
    fn parse(engine: &str) -> Result<Self> {
        match engine.trim().to_ascii_lowercase().as_str() {
            "sense-voice" | "sensevoice" | "" => Ok(Self::SenseVoice),
            other => Err(anyhow!(
                "unsupported local ASR engine '{}': expected 'sense-voice'",
                other
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::SenseVoice => "sense-voice",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SenseVoice => "SenseVoice（默认·最快）",
        }
    }
}

#[derive(Debug, Clone)]
struct SenseVoiceModelBundle {
    root_dir: PathBuf,
    model_file: PathBuf,
    tokens_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LocalTranscription {
    pub text: String,
    pub model_root: PathBuf,
}

/// R23 decode-side threads: configured > 0 clamps to [1, 8]; 0/negative means
/// auto (available parallelism clamped to [1, 8]).
pub fn effective_decoder_threads(configured: i32) -> i32 {
    if configured > 0 {
        configured.clamp(1, 8)
    } else {
        std::thread::available_parallelism()
            .map(|cores| cores.get() as i32)
            .unwrap_or(4)
            .clamp(1, 8)
    }
}

pub struct LocalSenseVoiceRecognizer {
    recognizer: OfflineRecognizer,
    engine: LocalEngine,
    root_dir: PathBuf,
    /// 2026-09-03: use_itn=false 后 SenseVoice 输出裸文本（标点本就靠 ITN 产出），
    /// 独立 ct-transformer 标点模型负责补回标点，数字转写仍由 pipeline 层兜底。
    punctuator: Option<OfflinePunctuation>,
}

impl LocalSenseVoiceRecognizer {
    pub fn create(
        config: &LocalNonstreamingConfig,
        install_root: impl AsRef<Path>,
    ) -> Result<Self> {
        let engine = LocalEngine::parse(&config.engine)?;
        let model_dir = resolve_model_dir(&config.model_dir, install_root.as_ref());
        let effective_threads = effective_decoder_threads(config.num_threads);
        let (recognizer_config, root_dir, punctuator) = match engine {
            LocalEngine::SenseVoice => {
                let model_bundle = prepare_runtime_bundle(SenseVoiceModelBundle::discover(
                    &model_dir,
                )?)?;
                let tokens_path = path_to_runtime_string(&model_bundle.tokens_file)?;
                let model_path = path_to_runtime_string(&model_bundle.model_file)?;
                let punctuator = if config.punct_enabled && !config.use_itn {
                    match create_punctuator(config, install_root.as_ref()) {
                        Ok(punct) => Some(punct),
                        Err(err) => {
                            warn!(error = %err, "punctuation model unavailable; output stays unpunctuated");
                            None
                        }
                    }
                } else {
                    None
                };
                let mut recognizer_config = OfflineRecognizerConfig::default();
                recognizer_config.feat_config.sample_rate = config.sample_rate_hz.max(1) as i32;
                recognizer_config.model_config.tokens = Some(tokens_path);
                recognizer_config.model_config.provider = Some(config.provider.clone());
                recognizer_config.model_config.num_threads = effective_threads;
                recognizer_config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                    model: Some(model_path),
                    language: Some(config.language.clone()),
                    use_itn: config.use_itn,
                };
                (recognizer_config, model_bundle.root_dir.clone(), punctuator)
            }
        };

        let recognizer = OfflineRecognizer::create(&recognizer_config)
            .ok_or_else(|| anyhow!("create sherpa-onnx local recognizer failed (engine={})", engine.name()))?;

        info!(
            engine = engine.name(),
            model_dir = %root_dir.display(),
            provider = %config.provider,
            language = %config.language,
            use_itn = config.use_itn,
            num_threads = effective_threads,
            configured_num_threads = config.num_threads,
            punctuation = punctuator.is_some(),
            "local ASR recognizer created"
        );

        Ok(Self {
            recognizer,
            engine,
            root_dir,
            punctuator,
        })
    }

    /// 给无标点的识别结果补标点（use_itn=false 的 SenseVoice 场景）。
    /// 无标点器或输入为空时返回 None，调用方保持原文。
    pub fn punctuate(&self, text: &str) -> Option<String> {
        let punctuator = self.punctuator.as_ref()?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        punctuator.add_punctuation(trimmed)
    }

    pub fn transcribe_samples(
        &self,
        sample_rate_hz: u32,
        samples: &[f32],
    ) -> Result<LocalTranscription> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(sample_rate_hz.max(1) as i32, samples);
        self.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| anyhow!("sherpa-onnx returned no local ASR result"))?;
        Ok(LocalTranscription {
            text: result.text,
            model_root: self.root_dir.clone(),
        })
    }

    /// R23 decode warmup: one tiny silence decode so the first real utterance
    /// does not pay one-time init cost. Best-effort; result discarded.
    pub fn warmup_once(&self, sample_rate_hz: u32) {
        let stream = self.recognizer.create_stream();
        let silence = vec![0.0f32; 1600];
        stream.accept_waveform(sample_rate_hz.max(1) as i32, &silence);
        self.recognizer.decode(&stream);
    }
}

fn resolve_model_dir(model_dir: &str, install_root: &Path) -> PathBuf {
    let path = PathBuf::from(model_dir);
    if path.is_absolute() {
        path
    } else {
        install_root.join(path)
    }
}

/// 2026-09-03: 创建离线标点器（ct-transformer zh-en）。模型缺失/加载失败只返回 Err，
/// 调用方降级为「无标点」而不阻塞识别器创建——识别永远优先于标点。
fn create_punctuator(
    config: &LocalNonstreamingConfig,
    install_root: &Path,
) -> Result<OfflinePunctuation> {
    let dir = resolve_model_dir(&config.punct_model_dir, install_root);
    let mut model_file = find_punct_model(&dir).ok_or_else(|| {
        anyhow!(
            "no punctuation model (model.onnx / model.int8.onnx) found under {}",
            dir.display()
        )
    })?;

    // sherpa-onnx 加载原生模型要求路径为 ASCII（与 sense-voice bundle 同纪律），
    // 非 ASCII 时复制进 LOCALAPPDATA 缓存目录。
    if contains_non_ascii(&model_file) {
        let cache_root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("ainput")
            .join("asr-cache")
            .join("punct");
        fs::create_dir_all(&cache_root)
            .with_context(|| format!("create punct cache directory {}", cache_root.display()))?;
        let dest = cache_root.join(
            model_file
                .file_name()
                .ok_or_else(|| anyhow!("invalid punct model name: {}", model_file.display()))?,
        );
        copy_if_stale(&model_file, &dest)?;
        model_file = dest;
    }

    let mut punct_config = OfflinePunctuationConfig::default();
    punct_config.model.ct_transformer = Some(path_to_runtime_string(&model_file)?);
    punct_config.model.num_threads = config.num_threads.clamp(1, 4);
    OfflinePunctuation::create(&punct_config)
        .ok_or_else(|| anyhow!("create sherpa-onnx offline punctuator failed"))
}

/// 标点模型目录允许留一层打包子目录（官方 tar 包带一层壳），优先 int8。
fn find_punct_model(dir: &Path) -> Option<PathBuf> {
    let mut queue = vec![dir.to_path_buf()];
    let mut fp32: Option<PathBuf> = None;
    while let Some(current) = queue.pop() {
        for entry in fs::read_dir(&current).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                queue.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            match name.as_str() {
                "model.int8.onnx" => return Some(path),
                "model.onnx" => fp32 = Some(path),
                _ => {}
            }
        }
    }
    fp32
}

impl SenseVoiceModelBundle {
    fn discover(model_dir: &Path) -> Result<Self> {
        if !model_dir.exists() {
            bail!(
                "local SenseVoice model directory does not exist: {}",
                model_dir.display()
            );
        }
        let candidates = discover_model_bundles(model_dir)?;
        if candidates.is_empty() {
            bail!(
                "no local SenseVoice model bundle found under {}",
                model_dir.display()
            );
        }
        Ok(select_first_bundle(candidates))
    }

    fn from_dir(dir: &Path) -> Option<Self> {
        let tokens_file = dir.join("tokens.txt");
        if !tokens_file.exists() {
            return None;
        }
        let model_int8 = dir.join("model.int8.onnx");
        let model_fp32 = dir.join("model.onnx");
        let model_file = if model_int8.exists() {
            model_int8
        } else if model_fp32.exists() {
            model_fp32
        } else {
            return None;
        };
        Some(Self {
            root_dir: dir.to_path_buf(),
            model_file,
            tokens_file,
        })
    }
}

fn discover_model_bundles(root_dir: &Path) -> Result<Vec<SenseVoiceModelBundle>> {
    let mut candidates = Vec::new();
    let mut pending_dirs = vec![root_dir.to_path_buf()];
    while let Some(dir) = pending_dirs.pop() {
        if let Some(bundle) = SenseVoiceModelBundle::from_dir(&dir) {
            candidates.push(bundle);
            continue;
        }
        for entry in
            fs::read_dir(&dir).with_context(|| format!("read model directory {}", dir.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending_dirs.push(entry.path());
            }
        }
    }
    Ok(candidates)
}

fn select_first_bundle(mut candidates: Vec<SenseVoiceModelBundle>) -> SenseVoiceModelBundle {
    candidates.sort_by(|left, right| left.root_dir.cmp(&right.root_dir));
    candidates.remove(0)
}

fn path_to_runtime_string(path: &Path) -> Result<String> {
    let absolute_path =
        fs::canonicalize(path).with_context(|| format!("canonicalize path {}", path.display()))?;
    #[allow(unused_mut)]
    let mut absolute_string = absolute_path
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", absolute_path.display()))?;
    #[cfg(windows)]
    {
        if let Some(stripped) = absolute_string.strip_prefix(r"\\?\") {
            absolute_string = stripped.to_string();
        }
        absolute_string = absolute_string.replace('/', "\\");
    }
    Ok(absolute_string)
}

fn prepare_runtime_bundle(model_bundle: SenseVoiceModelBundle) -> Result<SenseVoiceModelBundle> {
    if !contains_non_ascii(&model_bundle.root_dir) {
        return Ok(model_bundle);
    }

    let cache_root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ainput")
        .join("asr-cache");
    let bundle_name = model_bundle
        .root_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "sense-voice".to_string());
    let cache_dir = cache_root.join(bundle_name);
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create ASR cache directory {}", cache_dir.display()))?;

    let cached_model = cache_dir.join(model_bundle.model_file.file_name().ok_or_else(|| {
        anyhow!(
            "invalid model file name: {}",
            model_bundle.model_file.display()
        )
    })?);
    let cached_tokens = cache_dir.join(model_bundle.tokens_file.file_name().ok_or_else(|| {
        anyhow!(
            "invalid tokens file name: {}",
            model_bundle.tokens_file.display()
        )
    })?);

    copy_if_stale(&model_bundle.model_file, &cached_model)?;
    copy_if_stale(&model_bundle.tokens_file, &cached_tokens)?;

    info!(
        source_model_dir = %model_bundle.root_dir.display(),
        cache_dir = %cache_dir.display(),
        "prepared ASCII-safe local SenseVoice runtime bundle"
    );

    Ok(SenseVoiceModelBundle {
        root_dir: cache_dir,
        model_file: cached_model,
        tokens_file: cached_tokens,
    })
}

fn contains_non_ascii(path: &Path) -> bool {
    !path.as_os_str().to_string_lossy().is_ascii()
}

fn copy_if_stale(source: &Path, destination: &Path) -> Result<()> {
    if !needs_refresh(source, destination)? {
        return Ok(());
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "copy local ASR runtime file {} -> {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn needs_refresh(source: &Path, destination: &Path) -> Result<bool> {
    if !destination.exists() {
        return Ok(true);
    }
    let source_meta =
        fs::metadata(source).with_context(|| format!("read metadata {}", source.display()))?;
    let destination_meta = fs::metadata(destination)
        .with_context(|| format!("read metadata {}", destination.display()))?;
    if source_meta.len() != destination_meta.len() {
        return Ok(true);
    }
    let source_modified = source_meta.modified().ok();
    let destination_modified = destination_meta.modified().ok();
    Ok(matches!(
        (source_modified, destination_modified),
        (Some(source_time), Some(destination_time)) if source_time > destination_time
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(engine: &str, model_dir: &str) -> LocalNonstreamingConfig {
        LocalNonstreamingConfig {
            engine: engine.to_string(),
            model_dir: model_dir.to_string(),
            provider: "cpu".to_string(),
            sample_rate_hz: 16_000,
            language: "auto".to_string(),
            use_itn: true,
            punct_enabled: false,
            punct_model_dir: "models/punct".to_string(),
            num_threads: 4,
            release_grace_ms: 80,
            min_audio_ms: 800,
            min_rms_dbfs: -56.0,
        }
    }

    #[test]
    fn parse_engine_accepts_known_aliases() {
        assert!(matches!(LocalEngine::parse("sense-voice"), Ok(LocalEngine::SenseVoice)));
        assert!(matches!(LocalEngine::parse(""), Ok(LocalEngine::SenseVoice)));
        assert!(matches!(LocalEngine::parse("sensevoice"), Ok(LocalEngine::SenseVoice)));
        assert!(LocalEngine::parse("whisper").is_err());
        assert!(LocalEngine::parse("unknown-engine").is_err());
    }

    #[test]
    fn effective_decoder_threads_mapping() {
        // Explicit values clamp to [1, 8].
        assert_eq!(effective_decoder_threads(4), 4);
        assert_eq!(effective_decoder_threads(100), 8);
        // 0 / negative means auto: available parallelism clamped to [1, 8].
        let cores = std::thread::available_parallelism()
            .map(|cores| cores.get() as i32)
            .unwrap_or(4)
            .clamp(1, 8);
        assert_eq!(effective_decoder_threads(0), cores);
        assert_eq!(effective_decoder_threads(-1), cores);
        assert!(effective_decoder_threads(0) >= 1 && effective_decoder_threads(0) <= 8);
    }

    /// 2026-09-03: 用真实 75MB int8 标点模型验证「裸识别文本 → 带标点」契约，
    /// 同时确认标点模型不会把中文数字改写成阿拉伯数字（那条归 pipeline normalize 层）。
    #[test]
    #[ignore = "loads the real 75MB punct model; run with --release -- --ignored"]
    fn punctuator_restores_chinese_punctuation() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut config = test_config("sense-voice", "models/sense-voice");
        config.num_threads = 2;
        config.punct_enabled = true;
        config.use_itn = false;
        // 与运行态一致：识别器先建（app 实测识别器 env 初始化后标点器才能安全推理）
        let recognizer = LocalSenseVoiceRecognizer::create(&config, &manifest_dir)
            .expect("create recognizer with punctuator");
        let punctuated = recognizer
            .punctuate("我现在语音识别之后发给你都没有任何标点符号我不知道是怎么回事")
            .expect("punctuate");
        assert!(
            punctuated.contains('，') || punctuated.contains('。'),
            "no punctuation added: {punctuated}"
        );

        let punct = recognizer.punctuator.as_ref().expect("punctuator present");
        let text = punct
            .add_punctuation("我现在语音识别之后发给你都没有任何标点符号我不知道是怎么回事")
            .expect("punctuate");
        println!("punctuated: {text}");
        assert!(
            text.contains('，') || text.contains('。'),
            "no punctuation added: {text}"
        );

        let digit_text = punct
            .add_punctuation("我今年三十岁身高一百七十五")
            .expect("punctuate digits");
        println!("digits: {digit_text}");
        assert!(
            digit_text.contains("三十"),
            "punct model must not rewrite Chinese numerals: {digit_text}"
        );
    }

}
