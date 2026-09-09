use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sherpa_onnx::{OnlineParaformerModelConfig, OnlineRecognizer, OnlineRecognizerConfig};
use tracing::info;

use crate::config::ParaformerStreamingConfig;

pub struct ParaformerStreamingRecognizer {
    recognizer: OnlineRecognizer,
    root_dir: PathBuf,
    provider_used: String,
}

#[derive(Debug, Clone)]
struct StreamingBundle {
    root_dir: PathBuf,
    encoder_file: PathBuf,
    decoder_file: PathBuf,
    tokens_file: PathBuf,
}

impl StreamingBundle {
    fn from_dir(dir: &Path, prefer_int8: bool) -> Option<Self> {
        let tokens = dir.join("tokens.txt");
        if !tokens.exists() {
            return None;
        }
        let (enc_int8, enc_fp32) = (dir.join("encoder.int8.onnx"), dir.join("encoder.onnx"));
        let (dec_int8, dec_fp32) = (dir.join("decoder.int8.onnx"), dir.join("decoder.onnx"));
        let (encoder, decoder) = if prefer_int8 && enc_int8.exists() && dec_int8.exists() {
            (enc_int8, dec_int8)
        } else if enc_fp32.exists() && dec_fp32.exists() {
            (enc_fp32, dec_fp32)
        } else if enc_int8.exists() && dec_int8.exists() {
            (enc_int8, dec_int8)
        } else {
            return None;
        };
        Some(Self {
            root_dir: dir.to_path_buf(),
            encoder_file: encoder,
            decoder_file: decoder,
            tokens_file: tokens,
        })
    }
}

fn discover_bundle(root_dir: &Path) -> Result<StreamingBundle> {
    if !root_dir.exists() {
        bail!(
            "paraformer-streaming model directory does not exist: {}",
            root_dir.display()
        );
    }
    let mut pending = vec![root_dir.to_path_buf()];
    let mut fallback: Option<StreamingBundle> = None;
    while let Some(dir) = pending.pop() {
        if let Some(bundle) = StreamingBundle::from_dir(&dir, true) {
            return Ok(bundle);
        }
        if fallback.is_none() {
            if let Some(bundle) = StreamingBundle::from_dir(&dir, false) {
                fallback = Some(bundle);
            }
        }
        let entries = fs::read_dir(&dir)
            .with_context(|| format!("read model directory {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    fallback.ok_or_else(|| {
        anyhow!(
            "no paraformer-streaming bundle found under {} (need encoder.onnx + decoder.onnx + tokens.txt)",
            root_dir.display()
        )
    })
}

fn resolve_model_dir(model_dir: &str, install_root: &Path) -> PathBuf {
    let path = PathBuf::from(model_dir);
    if path.is_absolute() {
        path
    } else {
        install_root.join(path)
    }
}

fn path_to_string(path: &Path) -> Result<String> {
    let abs = fs::canonicalize(path)
        .with_context(|| format!("canonicalize path {}", path.display()))?;
    let mut s = abs
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", abs.display()))?;
    #[cfg(windows)]
    {
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            s = stripped.to_string();
        }
        s = s.replace('/', "\\");
    }
    Ok(s)
}

fn build_recognizer(
    bundle: &StreamingBundle,
    config: &ParaformerStreamingConfig,
    provider: &str,
) -> Option<OnlineRecognizer> {
    let mut rec_config = OnlineRecognizerConfig::default();
    rec_config.feat_config.sample_rate = config.sample_rate_hz.max(1) as i32;
    rec_config.model_config.tokens =
        path_to_string(&bundle.tokens_file).ok();
    rec_config.model_config.num_threads = config.num_threads.max(1);
    rec_config.model_config.provider = Some(provider.to_string());
    rec_config.model_config.paraformer = OnlineParaformerModelConfig {
        encoder: path_to_string(&bundle.encoder_file).ok(),
        decoder: path_to_string(&bundle.decoder_file).ok(),
    };
    rec_config.decoding_method = Some("greedy_search".to_string());
    OnlineRecognizer::create(&rec_config)
}

impl ParaformerStreamingRecognizer {
    pub fn create(
        config: &ParaformerStreamingConfig,
        install_root: impl AsRef<Path>,
    ) -> Result<Self> {
        let model_dir = resolve_model_dir(&config.model_dir, install_root.as_ref());
        let bundle = discover_bundle(&model_dir)?;
        let preferred = config.provider.trim().to_ascii_lowercase();
        let preferred = if preferred.is_empty() { "cuda".to_string() } else { preferred };
        let mut tried: Vec<String> = Vec::new();
        for provider in [preferred.clone(), "cpu".to_string()] {
            if tried.contains(&provider) {
                continue;
            }
            tried.push(provider.clone());
            if let Some(recognizer) = build_recognizer(&bundle, config, &provider) {
                info!(
                    provider = %provider,
                    model_dir = %bundle.root_dir.display(),
                    encoder = %bundle.encoder_file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    "paraformer-streaming recognizer created"
                );
                return Ok(Self {
                    recognizer,
                    root_dir: bundle.root_dir.clone(),
                    provider_used: provider,
                });
            }
        }
        Err(anyhow!(
            "create sherpa-onnx streaming paraformer recognizer failed (tried {})",
            tried.join(",")
        ))
    }

    pub fn provider_used(&self) -> &str {
        &self.provider_used
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    ///整段音频模拟流式喂入，按 chunk_ms 切块，返回终句文本。
    pub fn transcribe_streaming(
        &self,
        sample_rate_hz: u32,
        samples: &[f32],
        chunk_ms: u32,
    ) -> Result<String> {
        let stream = self.recognizer.create_stream();
        let chunk_ms = chunk_ms.max(50) as usize;
        let chunk_samples = (sample_rate_hz.max(1) as usize * chunk_ms) / 1000;
        let chunk_samples = chunk_samples.max(800);
        let mut offset = 0usize;
        let mut last_text = String::new();
        while offset < samples.len() {
            let end = (offset + chunk_samples).min(samples.len());
            stream.accept_waveform(sample_rate_hz.max(1) as i32, &samples[offset..end]);
            offset = end;
            while self.recognizer.is_ready(&stream) {
                self.recognizer.decode(&stream);
            }
            if let Some(result) = self.recognizer.get_result(&stream) {
                if !result.text.trim().is_empty() {
                    last_text = result.text;
                }
            }
        }
        stream.input_finished();
        while self.recognizer.is_ready(&stream) {
            self.recognizer.decode(&stream);
        }
        if let Some(result) = self.recognizer.get_result(&stream) {
            if !result.text.trim().is_empty() {
                return Ok(result.text);
            }
        }
        Ok(last_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 重型集成测试：默认忽略，显式跑
    /// `cargo test -p ainput --lib paraformer -- --ignored --nocapture`。
    /// 用自带 1.wav 实测，要求非空文本且 RTF<1。
    #[test]
    #[ignore]
    fn streaming_transcribes_bundled_wav() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("models")
            .join("paraformer-streaming");
        let wav_path = dir
            .join("sherpa-onnx-streaming-paraformer-bilingual-zh-en")
            .join("test_wavs")
            .join("1.wav");
        assert!(wav_path.exists(), "missing test wav {}", wav_path.display());
        let mut config = ParaformerStreamingConfig::default();
        config.model_dir = "models/paraformer-streaming".to_string();
        config.provider = "cpu".to_string();
        config.num_threads = 4;
        let recognizer =
            ParaformerStreamingRecognizer::create(&config, Path::new(env!("CARGO_MANIFEST_DIR")))
                .expect("create paraformer recognizer");
        let (sample_rate_hz, samples) = read_wav_mono16(&wav_path);
        let audio_secs = samples.len() as f32 / sample_rate_hz.max(1) as f32;
        let started = Instant::now();
        let text = recognizer
            .transcribe_streaming(sample_rate_hz, &samples, 200)
            .expect("transcribe");
        let rtf = started.elapsed().as_secs_f32() / audio_secs.max(0.01);
        eprintln!("paraformer text={text:?} rtf={rtf:.3}");
        assert!(!text.trim().is_empty(), "empty transcription");
        assert!(rtf < 1.0, "too slow for input method: rtf={rtf}");
    }

    fn read_wav_mono16(path: &Path) -> (u32, Vec<f32>) {
        let mut reader = hound::WavReader::open(path).expect("open wav");
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => {
                reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect()
            }
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample as u32 - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| (s.unwrap_or(0) as f32) / max)
                    .collect()
            }
        };
        if spec.channels > 1 {
            let ch = spec.channels as usize;
            let mono: Vec<f32> = samples
                .chunks(ch)
                .map(|c| c.iter().sum::<f32>() / ch as f32)
                .collect();
            (spec.sample_rate, mono)
        } else {
            (spec.sample_rate, samples)
        }
    }
}
