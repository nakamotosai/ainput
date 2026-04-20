use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, Stream, StreamConfig, SupportedStreamConfig};

#[derive(Debug, Clone)]
pub struct RecordedAudio {
    pub sample_rate_hz: i32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

pub struct ActiveRecording {
    sample_rate_hz: i32,
    channels: u16,
    shared_samples: Arc<Mutex<Vec<f32>>>,
    shared_level_bits: Arc<AtomicU32>,
    stream: Stream,
}

impl ActiveRecording {
    pub fn start_default_input() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device available"))?;
        let config = device
            .default_input_config()
            .context("read default input device config")?;

        tracing::info!(
            sample_format = ?config.sample_format(),
            channels = config.channels(),
            sample_rate_hz = config.sample_rate(),
            "start microphone recording"
        );

        Self::start_with_config(device, config)
    }

    fn start_with_config(
        device: cpal::Device,
        supported_config: SupportedStreamConfig,
    ) -> Result<Self> {
        let stream_config: StreamConfig = supported_config.clone().into();
        let sample_rate_hz = stream_config.sample_rate as i32;
        let channels = stream_config.channels;
        let shared_samples = Arc::new(Mutex::new(Vec::new()));
        let shared_level_bits = Arc::new(AtomicU32::new(0f32.to_bits()));
        let err_fn = |err| tracing::error!(error = %err, "audio input stream error");

        let stream = build_stream(
            &device,
            &supported_config,
            &stream_config,
            shared_samples.clone(),
            shared_level_bits.clone(),
            err_fn,
        )?;

        stream.play().context("start microphone stream")?;

        Ok(Self {
            sample_rate_hz,
            channels,
            shared_samples,
            shared_level_bits,
            stream,
        })
    }

    pub fn current_level(&self) -> f32 {
        f32::from_bits(self.shared_level_bits.load(Ordering::Relaxed))
    }

    pub fn sample_rate_hz(&self) -> i32 {
        self.sample_rate_hz
    }

    pub fn sample_count(&self) -> usize {
        let Ok(samples) = self.shared_samples.lock() else {
            return 0;
        };

        samples.len()
    }

    pub fn take_new_samples(&self, cursor: &mut usize) -> Vec<f32> {
        let Ok(samples) = self.shared_samples.lock() else {
            return Vec::new();
        };

        if *cursor >= samples.len() {
            return Vec::new();
        }

        let chunk = samples[*cursor..].to_vec();
        *cursor = samples.len();
        chunk
    }

    pub fn stop(self) -> Result<RecordedAudio> {
        drop(self.stream);

        let samples = Arc::try_unwrap(self.shared_samples)
            .map_err(|_| anyhow!("audio samples are still borrowed"))?
            .into_inner()
            .map_err(|_| anyhow!("audio sample buffer was poisoned"))?;

        tracing::info!(
            sample_rate_hz = self.sample_rate_hz,
            channels = self.channels,
            samples = samples.len(),
            "stop microphone recording"
        );

        Ok(RecordedAudio {
            sample_rate_hz: self.sample_rate_hz,
            channels: self.channels,
            samples,
        })
    }
}

fn build_stream(
    device: &cpal::Device,
    supported_config: &SupportedStreamConfig,
    stream_config: &StreamConfig,
    shared_samples: Arc<Mutex<Vec<f32>>>,
    shared_level_bits: Arc<AtomicU32>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream> {
    let channels = stream_config.channels as usize;

    let stream = match supported_config.sample_format() {
        SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _| {
                append_input_data(data, channels, &shared_samples, &shared_level_bits)
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            stream_config,
            move |data: &[u16], _| {
                append_input_data(data, channels, &shared_samples, &shared_level_bits)
            },
            err_fn,
            None,
        ),
        SampleFormat::F32 => device.build_input_stream(
            stream_config,
            move |data: &[f32], _| {
                append_input_data(data, channels, &shared_samples, &shared_level_bits)
            },
            err_fn,
            None,
        ),
        sample_format => {
            return Err(anyhow!(
                "unsupported microphone sample format: {sample_format:?}"
            ));
        }
    }
    .context("build microphone stream")?;

    Ok(stream)
}

fn append_input_data<T>(
    input: &[T],
    channels: usize,
    shared_samples: &Arc<Mutex<Vec<f32>>>,
    shared_level_bits: &Arc<AtomicU32>,
) where
    T: Sample,
    f32: FromSample<T>,
{
    if input.is_empty() || channels == 0 {
        return;
    }

    let mut peak = 0.0f32;

    if let Ok(mut samples) = shared_samples.lock() {
        for frame in input.chunks(channels) {
            let sum = frame
                .iter()
                .fold(0.0f32, |acc, sample| acc + f32::from_sample(*sample));
            let mixed = sum / frame.len() as f32;
            peak = peak.max(mixed.abs());
            samples.push(mixed);
        }
    }

    shared_level_bits.store(peak.to_bits(), Ordering::Relaxed);
}
