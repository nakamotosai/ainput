use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, Wave};

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
        let model_bundle = SenseVoiceModelBundle::discover(&config.model_dir)?;

        let mut recognizer_config = OfflineRecognizerConfig::default();
        recognizer_config.feat_config.sample_rate = config.sample_rate_hz;
        recognizer_config.model_config.tokens =
            Some(model_bundle.tokens_file.display().to_string());
        recognizer_config.model_config.provider = Some(config.provider.clone());
        recognizer_config.model_config.num_threads = config.num_threads;
        recognizer_config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(model_bundle.model_file.display().to_string()),
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
        let wav_path_str = wav_path
            .to_str()
            .ok_or_else(|| anyhow!("wav path is not valid UTF-8: {}", wav_path.display()))?;
        let wave = Wave::read(wav_path_str)
            .ok_or_else(|| anyhow!("read wav file {}", wav_path.display()))?;

        let stream = self.recognizer.create_stream();
        stream.accept_waveform(wave.sample_rate(), wave.samples());
        self.recognizer.decode(&stream);

        let result = stream
            .get_result()
            .ok_or_else(|| anyhow!("sherpa-onnx returned no transcription result"))?;

        Ok(Transcription {
            text: result.text,
            sample_rate_hz: wave.sample_rate(),
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
