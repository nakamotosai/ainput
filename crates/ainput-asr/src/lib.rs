use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig};

#[derive(Debug, Clone)]
pub struct SenseVoiceConfig {
    pub model_dir: PathBuf,
    pub provider: String,
    pub sample_rate_hz: i32,
    pub language: String,
    pub use_itn: bool,
    pub num_threads: i32,
}

#[derive(Debug, Clone)]
pub struct SenseVoiceModelBundle {
    pub root_dir: PathBuf,
    pub model_file: PathBuf,
    pub tokens_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Transcription {
    pub text: String,
    pub sample_rate_hz: i32,
    pub source_wav: PathBuf,
    pub model_root: PathBuf,
}

pub struct SenseVoiceRecognizer {
    recognizer: OfflineRecognizer,
    model_bundle: SenseVoiceModelBundle,
}

impl SenseVoiceRecognizer {
    pub fn create(config: &SenseVoiceConfig) -> Result<Self> {
        let model_bundle =
            prepare_runtime_bundle(SenseVoiceModelBundle::discover(&config.model_dir)?)?;
        let tokens_path = path_to_runtime_string(&model_bundle.tokens_file)?;
        let model_path = path_to_runtime_string(&model_bundle.model_file)?;

        let mut recognizer_config = OfflineRecognizerConfig::default();
        recognizer_config.feat_config.sample_rate = config.sample_rate_hz;
        recognizer_config.model_config.tokens = Some(tokens_path);
        recognizer_config.model_config.provider = Some(config.provider.clone());
        recognizer_config.model_config.num_threads = config.num_threads;
        recognizer_config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(model_path),
            language: Some(config.language.clone()),
            use_itn: config.use_itn,
        };

        let recognizer = OfflineRecognizer::create(&recognizer_config)
            .ok_or_else(|| anyhow!("create sherpa-onnx offline recognizer failed"))?;

        tracing::info!(
            model_dir = %model_bundle.root_dir.display(),
            model_file = %model_bundle.model_file.display(),
            tokens_file = %model_bundle.tokens_file.display(),
            language = %config.language,
            use_itn = config.use_itn,
            num_threads = config.num_threads,
            "sense voice recognizer created"
        );

        Ok(Self {
            recognizer,
            model_bundle,
        })
    }

    pub fn transcribe_wav_file(&self, wav_path: impl AsRef<Path>) -> Result<Transcription> {
        let wav_path = wav_path.as_ref();
        let (sample_rate_hz, samples) = read_wav_samples(wav_path)?;

        let stream = self.recognizer.create_stream();
        stream.accept_waveform(sample_rate_hz, &samples);
        self.recognizer.decode(&stream);

        let result = stream
            .get_result()
            .ok_or_else(|| anyhow!("sherpa-onnx returned no transcription result"))?;

        Ok(Transcription {
            text: result.text,
            sample_rate_hz,
            source_wav: wav_path.to_path_buf(),
            model_root: self.model_bundle.root_dir.clone(),
        })
    }

    pub fn transcribe_samples(
        &self,
        sample_rate_hz: i32,
        samples: &[f32],
        source_label: impl Into<PathBuf>,
    ) -> Result<Transcription> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(sample_rate_hz, samples);
        self.recognizer.decode(&stream);

        let result = stream
            .get_result()
            .ok_or_else(|| anyhow!("sherpa-onnx returned no transcription result"))?;

        Ok(Transcription {
            text: result.text,
            sample_rate_hz,
            source_wav: source_label.into(),
            model_root: self.model_bundle.root_dir.clone(),
        })
    }
}

fn path_to_runtime_string(path: &Path) -> Result<String> {
    let absolute_path =
        fs::canonicalize(path).with_context(|| format!("canonicalize path {}", path.display()))?;
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

    tracing::info!(
        source_model_dir = %model_bundle.root_dir.display(),
        cache_dir = %cache_dir.display(),
        "prepared ASCII-safe ASR runtime bundle"
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
            "copy ASR runtime file {} -> {}",
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

fn read_wav_samples(wav_path: &Path) -> Result<(i32, Vec<f32>)> {
    let mut reader = hound::WavReader::open(wav_path)
        .with_context(|| format!("read wav file {}", wav_path.display()))?;
    let spec = reader.spec();
    let sample_rate_hz =
        i32::try_from(spec.sample_rate).context("wav sample rate does not fit in i32")?;

    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("decode wav samples {}", wav_path.display()))?,
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|sample| sample.map(|value| f32::from(value) / f32::from(i8::MAX)))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("decode wav samples {}", wav_path.display()))?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|sample| sample.map(|value| f32::from(value) / f32::from(i16::MAX)))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("decode wav samples {}", wav_path.display()))?,
        (hound::SampleFormat::Int, 24) | (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|sample| sample.map(|value| value as f32 / i32::MAX as f32))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("decode wav samples {}", wav_path.display()))?,
        _ => bail!(
            "unsupported wav format: sample_format={:?}, bits_per_sample={}",
            spec.sample_format,
            spec.bits_per_sample
        ),
    };

    Ok((sample_rate_hz, samples))
}

impl SenseVoiceModelBundle {
    pub fn discover(model_dir: impl AsRef<Path>) -> Result<Self> {
        let model_dir = model_dir.as_ref();
        if !model_dir.exists() {
            bail!("model directory does not exist: {}", model_dir.display());
        }

        if let Some(bundle) = Self::from_dir(model_dir) {
            return Ok(bundle);
        }

        let mut candidates = Vec::new();
        for entry in fs::read_dir(model_dir)
            .with_context(|| format!("read model directory {}", model_dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            if let Some(bundle) = Self::from_dir(&entry.path()) {
                candidates.push(bundle);
            }
        }

        if candidates.is_empty() {
            bail!(
                "no SenseVoice model bundle found under {}",
                model_dir.display()
            );
        }

        candidates.sort_by(|left, right| left.root_dir.cmp(&right.root_dir));
        Ok(candidates.remove(0))
    }

    fn from_dir(dir: &Path) -> Option<Self> {
        let model_int8 = dir.join("model.int8.onnx");
        let model_fp32 = dir.join("model.onnx");
        let tokens_file = dir.join("tokens.txt");

        if !tokens_file.exists() {
            return None;
        }

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
