use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use hound::{SampleFormat, WavSpec, WavWriter};
use wasapi::{
    DeviceCollection, Direction, SampleType, StreamMode, WaveFormat, get_default_device,
    initialize_mta,
};

const SAMPLE_RATE: usize = 48_000;
const CHANNELS: usize = 2;
const CHUNK_FRAMES: usize = 2048;

pub struct ActiveAudioCapture {
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<Result<PathBuf>>>,
}

impl ActiveAudioCapture {
    pub fn start_loopback(output_path: PathBuf) -> Result<Self> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let thread_stop = stop_flag.clone();
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        let join_handle = thread::Builder::new()
            .name("ainput-recording-audio".to_string())
            .spawn(move || {
                let result = capture_loopback_audio(output_path, thread_stop, startup_tx.clone());
                if let Err(error) = &result {
                    let _ = startup_tx.send(Err(anyhow!(error.to_string())));
                }
                result
            })
            .context("启动系统音频采集线程失败")?;

        startup_rx
            .recv()
            .map_err(|_| anyhow!("系统音频线程启动握手失败"))??;

        Ok(Self {
            stop_flag,
            join_handle: Some(join_handle),
        })
    }

    pub fn stop(mut self) -> Result<PathBuf> {
        self.stop_flag.store(true, Ordering::SeqCst);
        let join_handle = self
            .join_handle
            .take()
            .ok_or_else(|| anyhow!("系统音频线程句柄丢失"))?;
        join_handle
            .join()
            .map_err(|_| anyhow!("系统音频线程异常退出"))?
    }
}

fn capture_loopback_audio(
    output_path: PathBuf,
    stop_flag: Arc<AtomicBool>,
    startup_tx: mpsc::SyncSender<Result<()>>,
) -> Result<PathBuf> {
    initialize_mta().ok().context("初始化音频 COM 失败")?;

    let device = get_default_render_device().context("读取默认播放设备失败")?;
    let mut audio_client = device.get_iaudioclient().context("创建音频客户端失败")?;

    let desired_format = WaveFormat::new(32, 32, &SampleType::Float, SAMPLE_RATE, CHANNELS, None);
    let block_align = desired_format.get_blockalign() as usize;
    let (_, min_time) = audio_client
        .get_device_period()
        .context("读取音频设备周期失败")?;
    let stream_mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: min_time,
    };
    audio_client
        .initialize_client(&desired_format, &Direction::Capture, &stream_mode)
        .context("初始化系统音频 loopback 失败")?;

    let capture_client = audio_client
        .get_audiocaptureclient()
        .context("创建音频捕获客户端失败")?;
    let event_handle = audio_client
        .set_get_eventhandle()
        .context("创建音频事件句柄失败")?;

    let spec = WavSpec {
        channels: CHANNELS as u16,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(&output_path, spec)
        .with_context(|| format!("创建 {:?} 失败", output_path))?;

    let mut queued = VecDeque::<u8>::with_capacity(block_align * CHUNK_FRAMES * 8);
    audio_client.start_stream().context("启动系统音频流失败")?;
    let _ = startup_tx.send(Ok(()));

    while !stop_flag.load(Ordering::SeqCst) {
        capture_client
            .read_from_device_to_deque(&mut queued)
            .context("读取系统音频数据失败")?;
        flush_complete_chunks(&mut queued, &mut writer, block_align)?;
        let _ = event_handle.wait_for_event(200);
    }

    capture_client
        .read_from_device_to_deque(&mut queued)
        .context("读取剩余系统音频数据失败")?;
    flush_all_samples(&mut queued, &mut writer)?;
    audio_client.stop_stream().context("停止系统音频流失败")?;
    writer.finalize().context("写入系统音频 WAV 失败")?;

    Ok(output_path)
}

fn flush_complete_chunks(
    queued: &mut VecDeque<u8>,
    writer: &mut WavWriter<std::io::BufWriter<std::fs::File>>,
    block_align: usize,
) -> Result<()> {
    let chunk_bytes = block_align * CHUNK_FRAMES;
    while queued.len() >= chunk_bytes {
        for _ in 0..(chunk_bytes / 4) {
            writer
                .write_sample(pop_f32(queued)?)
                .context("写入系统音频样本失败")?;
        }
    }
    Ok(())
}

fn flush_all_samples(
    queued: &mut VecDeque<u8>,
    writer: &mut WavWriter<std::io::BufWriter<std::fs::File>>,
) -> Result<()> {
    while queued.len() >= 4 {
        writer
            .write_sample(pop_f32(queued)?)
            .context("写入剩余系统音频样本失败")?;
    }
    Ok(())
}

fn pop_f32(queued: &mut VecDeque<u8>) -> Result<f32> {
    let bytes = [
        queued
            .pop_front()
            .ok_or_else(|| anyhow!("系统音频数据长度不足"))?,
        queued
            .pop_front()
            .ok_or_else(|| anyhow!("系统音频数据长度不足"))?,
        queued
            .pop_front()
            .ok_or_else(|| anyhow!("系统音频数据长度不足"))?,
        queued
            .pop_front()
            .ok_or_else(|| anyhow!("系统音频数据长度不足"))?,
    ];
    Ok(f32::from_le_bytes(bytes))
}

fn get_default_render_device() -> Result<wasapi::Device> {
    let mut last_error = None;
    for _ in 0..20 {
        match get_default_device(&Direction::Render) {
            Ok(device) => return Ok(device),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(200));
            }
        }
    }

    if let Ok(collection) = DeviceCollection::new(&Direction::Render) {
        if collection.get_nbr_devices().unwrap_or(0) > 0
            && let Ok(device) = collection.get_device_at_index(0)
        {
            return Ok(device);
        }
    }

    Err(anyhow!(
        "{}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "未知错误".to_string())
    ))
}
