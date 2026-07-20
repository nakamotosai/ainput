use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig};
use tracing::info;

use crate::config::LocalNonstreamingConfig;

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

pub struct LocalSenseVoiceRecognizer {
    recognizer: OfflineRecognizer,
    model_bundle: SenseVoiceModelBundle,
}

impl LocalSenseVoiceRecognizer {
    pub fn create(
        config: &LocalNonstreamingConfig,
        install_root: impl AsRef<Path>,
    ) -> Result<Self> {
        let model_dir = resolve_model_dir(&config.model_dir, install_root.as_ref());
        let model_bundle = prepare_runtime_bundle(SenseVoiceModelBundle::discover(&model_dir)?)?;
        let tokens_path = path_to_runtime_string(&model_bundle.tokens_file)?;
        let model_path = path_to_runtime_string(&model_bundle.model_file)?;

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

        let recognizer = OfflineRecognizer::create(&recognizer_config)
            .ok_or_else(|| anyhow!("create sherpa-onnx SenseVoice recognizer failed"))?;

        info!(
            model_dir = %model_bundle.root_dir.display(),
            model_file = %model_bundle.model_file.display(),
            tokens_file = %model_bundle.tokens_file.display(),
            provider = %config.provider,
            language = %config.language,
            use_itn = config.use_itn,
            num_threads = config.num_threads,
            "local SenseVoice recognizer created"
        );

        Ok(Self {
            recognizer,
            model_bundle,
        })
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
            .ok_or_else(|| anyhow!("sherpa-onnx returned no local SenseVoice result"))?;
        Ok(LocalTranscription {
            text: result.text,
            model_root: self.model_bundle.root_dir.clone(),
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
        .join("ainput2")
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
