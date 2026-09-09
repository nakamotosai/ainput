use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sherpa_onnx::{
    OfflineFunASRNanoModelConfig, OfflinePunctuation, OfflinePunctuationConfig,
    OfflineQwen3ASRModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSenseVoiceModelConfig,
};
use tracing::{info, warn};

use crate::config::LocalNonstreamingConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEngine {
    SenseVoice,
    FunAsrNano,
    Qwen3Asr,
}

impl LocalEngine {
    fn parse(engine: &str) -> Result<Self> {
        match engine.trim().to_ascii_lowercase().as_str() {
            "sense-voice" | "sensevoice" | "" => Ok(Self::SenseVoice),
            "funasr-nano" | "fun-asr-nano" | "funasr_nano" => Ok(Self::FunAsrNano),
            "qwen3-asr" | "qwen3_asr" => Ok(Self::Qwen3Asr),
            other => Err(anyhow!(
                "unsupported local ASR engine '{}': expected 'sense-voice', 'qwen3-asr' or 'funasr-nano'",
                other
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::SenseVoice => "sense-voice",
            Self::FunAsrNano => "funasr-nano",
            Self::Qwen3Asr => "qwen3-asr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SenseVoice => "SenseVoice（默认·最快）",
            Self::Qwen3Asr => "Qwen3-ASR 0.6B（更准·较慢）",
            Self::FunAsrNano => "FunASR-Nano（本机兼容问题）",
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
struct FunAsrNanoModelBundle {
    root_dir: PathBuf,
    encoder_adaptor_file: PathBuf,
    llm_file: PathBuf,
    embedding_file: PathBuf,
    tokenizer_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct Qwen3AsrModelBundle {
    root_dir: PathBuf,
    conv_frontend_file: PathBuf,
    encoder_file: PathBuf,
    decoder_file: PathBuf,
    tokenizer_dir: PathBuf,
}

impl Qwen3AsrModelBundle {
    fn from_dir(dir: &Path) -> Option<Self> {
        let conv = dir.join("conv_frontend.onnx");
        let encoder = dir.join("encoder.int8.onnx");
        let decoder = dir.join("decoder.int8.onnx");
        if !(conv.exists() && encoder.exists() && decoder.exists()) {
            return None;
        }
        let candidates = ["tokenizer", "tokenizer/"];
        let tokenizer_dir = candidates
            .iter()
            .map(|name| dir.join(name))
            .find(|path| path.is_dir())?;
        Some(Self {
            root_dir: dir.to_path_buf(),
            conv_frontend_file: conv,
            encoder_file: encoder,
            decoder_file: decoder,
            tokenizer_dir,
        })
    }
}

impl FunAsrNanoModelBundle {
    fn from_dir(dir: &Path) -> Option<Self> {
        let encoder_int8 = dir.join("encoder_adaptor.int8.onnx");
        let embedding_int8 = dir.join("embedding.int8.onnx");
        let tokenizer_dir = dir.join("Qwen3-0.6B");
        // 2026-09-09: sherpa 官方新增 fp16 包（llm.fp16.onnx），精度与 int8 同源、
        // 在部分 AMD 机器上可绕开 llm.int8.onnx 的静默空输出 bug（sherpa#828）。
        // 优先 int8（更快），其次 fp16。
        let llm_int8 = dir.join("llm.int8.onnx");
        let llm_fp16 = dir.join("llm.fp16.onnx");
        let llm_file = if llm_int8.exists() {
            llm_int8
        } else if llm_fp16.exists() {
            llm_fp16
        } else {
            return None;
        };
        if !(encoder_int8.exists() && embedding_int8.exists()) {
            return None;
        }
        if !tokenizer_dir.is_dir() {
            return None;
        }
        Some(Self {
            root_dir: dir.to_path_buf(),
            encoder_adaptor_file: encoder_int8,
            llm_file,
            embedding_file: embedding_int8,
            tokenizer_dir,
        })
    }
}

fn discover_funasr_nano_bundle(root_dir: &Path) -> Result<FunAsrNanoModelBundle> {
    if !root_dir.exists() {
        bail!(
            "local funasr-nano model directory does not exist: {}",
            root_dir.display()
        );
    }
    let mut pending_dirs = vec![root_dir.to_path_buf()];
    while let Some(dir) = pending_dirs.pop() {
        if let Some(bundle) = FunAsrNanoModelBundle::from_dir(&dir) {
            return Ok(bundle);
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
    bail!(
        "no funasr-nano model bundle found under {} (need encoder_adaptor.int8.onnx + embedding.int8.onnx + llm.int8.onnx/llm.fp16.onnx + Qwen3-0.6B/)",
        root_dir.display()
    );
}

fn discover_qwen3_bundle(root_dir: &Path) -> Result<Qwen3AsrModelBundle> {
    if !root_dir.exists() {
        bail!(
            "local qwen3-asr model directory does not exist: {}",
            root_dir.display()
        );
    }
    let mut pending_dirs = vec![root_dir.to_path_buf()];
    while let Some(dir) = pending_dirs.pop() {
        if let Some(bundle) = Qwen3AsrModelBundle::from_dir(&dir) {
            return Ok(bundle);
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
    bail!(
        "no qwen3-asr model bundle found under {} (need conv_frontend.onnx + encoder.int8.onnx + decoder.int8.onnx + tokenizer/)",
        root_dir.display()
    );
}

#[derive(Debug, Clone)]
pub struct LocalTranscription {
    pub text: String,
    pub model_root: PathBuf,
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
                recognizer_config.model_config.num_threads = config.num_threads.max(1);
                recognizer_config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                    model: Some(model_path),
                    language: Some(config.language.clone()),
                    use_itn: config.use_itn,
                };
                (recognizer_config, model_bundle.root_dir.clone(), punctuator)
            }
            LocalEngine::FunAsrNano => {
                let raw_bundle = discover_funasr_nano_bundle(&model_dir)?;
                let prepared = prepare_funasr_nano_runtime_files(&raw_bundle)?;
                let tokenizer_dir =
                    path_to_runtime_string(&prepared.tokenizer_dir)?;

                let mut recognizer_config = OfflineRecognizerConfig::default();
                recognizer_config.feat_config.sample_rate = config.sample_rate_hz.max(1) as i32;
                recognizer_config.model_config.provider = Some(config.provider.clone());
                recognizer_config.model_config.num_threads = config.num_threads.max(1);
                recognizer_config.model_config.funasr_nano = OfflineFunASRNanoModelConfig {
                    encoder_adaptor: Some(path_to_runtime_string(
                        &prepared.encoder_adaptor_file,
                    )?),
                    llm: Some(path_to_runtime_string(&prepared.llm_file)?),
                    embedding: Some(path_to_runtime_string(&prepared.embedding_file)?),
                    tokenizer: Some(tokenizer_dir),
                    system_prompt: Some("You are a helpful assistant.".to_string()),
                    user_prompt: Some("语音转写：".to_string()),
                    max_new_tokens: 512,
                    temperature: 1e-6,
                    top_p: 0.8,
                    seed: 42,
                    language: if config.language.eq_ignore_ascii_case("auto") {
                        None
                    } else {
                        Some(config.language.clone())
                    },
                    itn: if config.use_itn { 1 } else { 0 },
                    hotwords: None,
                };
                (recognizer_config, raw_bundle.root_dir.clone(), None)
            }
            LocalEngine::Qwen3Asr => {
                let raw_bundle = discover_qwen3_bundle(&model_dir)?;
                let prepared = prepare_qwen3_runtime_bundle(&raw_bundle)?;
                let mut recognizer_config = OfflineRecognizerConfig::default();
                recognizer_config.feat_config.sample_rate = config.sample_rate_hz.max(1) as i32;
                recognizer_config.model_config.provider = Some(config.provider.clone());
                recognizer_config.model_config.num_threads = config.num_threads.max(1);
                recognizer_config.model_config.qwen3_asr = OfflineQwen3ASRModelConfig {
                    conv_frontend: Some(path_to_runtime_string(&prepared.conv_frontend_file)?),
                    encoder: Some(path_to_runtime_string(&prepared.encoder_file)?),
                    decoder: Some(path_to_runtime_string(&prepared.decoder_file)?),
                    tokenizer: Some(path_to_runtime_string(&prepared.tokenizer_dir)?),
                    max_new_tokens: 512,
                    max_total_len: 4096,
                    temperature: 1e-6,
                    top_p: 0.8,
                    seed: 42,
                    hotwords: None,
                    ..OfflineQwen3ASRModelConfig::default()
                };
                (recognizer_config, raw_bundle.root_dir.clone(), None)
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
            num_threads = config.num_threads,
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

fn prepare_qwen3_runtime_bundle(
    bundle: &Qwen3AsrModelBundle,
) -> Result<Qwen3AsrModelBundle> {
    let ascii_safe = [&bundle.conv_frontend_file, &bundle.encoder_file, &bundle.decoder_file]
        .iter()
        .all(|path| !contains_non_ascii(path))
        && !contains_non_ascii(&bundle.tokenizer_dir);
    if ascii_safe {
        return Ok(bundle.clone());
    }

    let cache_root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ainput")
        .join("asr-cache");
    let cache_dir = cache_root.join(
        bundle
            .root_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "qwen3-asr".to_string()),
    );
    fs::create_dir_all(cache_dir.join("tokenizer"))
        .with_context(|| format!("create ASR cache directory {}", cache_dir.display()))?;

    let copy_file = |source: &Path, dest_name: &str| -> Result<PathBuf> {
        let destination = cache_dir.join(dest_name);
        copy_if_stale(source, &destination)?;
        Ok(destination)
    };

    Ok(Qwen3AsrModelBundle {
        root_dir: cache_dir.clone(),
        conv_frontend_file: copy_file(&bundle.conv_frontend_file, "conv_frontend.onnx")?,
        encoder_file: copy_file(&bundle.encoder_file, "encoder.int8.onnx")?,
        decoder_file: copy_file(&bundle.decoder_file, "decoder.int8.onnx")?,
        tokenizer_dir: {
            for file in ["merges.txt", "tokenizer.json", "vocab.json", "tokens.txt", "tokenizer_config.json"] {
                let source = bundle.tokenizer_dir.join(file);
                if source.exists() {
                    copy_if_stale(&source, &cache_dir.join("tokenizer").join(file))?;
                }
            }
            cache_dir.join("tokenizer")
        },
    })
}

fn prepare_funasr_nano_runtime_files(
    bundle: &FunAsrNanoModelBundle,
) -> Result<FunAsrNanoModelBundle> {
    let ascii_safe = [
        &bundle.encoder_adaptor_file,
        &bundle.llm_file,
        &bundle.embedding_file,
    ]
    .iter()
    .all(|path| !contains_non_ascii(path))
        && !contains_non_ascii(&bundle.tokenizer_dir);
    if ascii_safe {
        return Ok(bundle.clone());
    }

    let cache_root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ainput")
        .join("asr-cache");
    let cache_dir = cache_root.join(
        bundle
            .root_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "funasr-nano".to_string()),
    );
    fs::create_dir_all(cache_dir.join("Qwen3-0.6B")).with_context(|| {
        format!("create ASR cache directory {}", cache_dir.display())
    })?;

    let copy_file = |source: &Path, dest_name: &str| -> Result<PathBuf> {
        let destination = cache_dir.join(dest_name);
        copy_if_stale(source, &destination)?;
        Ok(destination)
    };

    Ok(FunAsrNanoModelBundle {
        root_dir: cache_dir.clone(),
        encoder_adaptor_file: copy_file(&bundle.encoder_adaptor_file, "encoder_adaptor.int8.onnx")?,
        llm_file: copy_file(&bundle.llm_file, "llm.int8.onnx")?,
        embedding_file: copy_file(&bundle.embedding_file, "embedding.int8.onnx")?,
        tokenizer_dir: {
            for file in ["merges.txt", "tokenizer.json", "vocab.json", "tokens.txt", "tokenizer_config.json"] {
                let source = bundle.tokenizer_dir.join(file);
                if source.exists() {
                    copy_if_stale(&source, &cache_dir.join("Qwen3-0.6B").join(file))?;
                }
            }
            cache_dir.join("Qwen3-0.6B")
        },
    })
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
        assert!(matches!(LocalEngine::parse("funasr-nano"), Ok(LocalEngine::FunAsrNano)));
        assert!(LocalEngine::parse("whisper").is_err());
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

    #[test]
    #[ignore = "loads the real 950MB funasr-nano bundle; run with --release -- --ignored"]
    fn funasr_nano_transcribes_hunan_dialect_wav() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let config = test_config("funasr-nano", "models/funasr-nano");
        let recognizer =
            LocalSenseVoiceRecognizer::create(&config, &manifest_dir).expect("create recognizer");

        let wav_path = manifest_dir.join("models/funasr-nano/test_wavs/dia_hunan.wav");
        let mut reader = hound::WavReader::open(&wav_path).expect("open wav");
        let sample_rate = reader.spec().sample_rate;
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.expect("read sample") as f32 / 32768.0)
            .collect();
        assert!(!samples.is_empty());

        let started = std::time::Instant::now();
        let transcription = recognizer
            .transcribe_samples(sample_rate, &samples)
            .expect("transcribe");
        let elapsed = started.elapsed();

        println!("transcript: {}", transcription.text);
        println!("audio_s={:.2} decode_ms={}", samples.len() as f32 / sample_rate as f32, elapsed.as_millis());
        assert!(transcription.text.contains("孙膑"), "unexpected transcript: {}", transcription.text);
        assert!(transcription.text.contains("庞涓"), "unexpected transcript: {}", transcription.text);
    }
}
