mod audio;
mod ffmpeg;
mod selection;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use audio::ActiveAudioCapture;
use ffmpeg::{
    ActiveVideoCapture, mux_audio_video, mux_video_only, probe_media, render_with_watermark,
    resolve_ffmpeg_path,
};
use selection::{CaptureRegion, RecordingFrame, choose_region_interactive};
use serde::{Deserialize, Serialize};

pub use selection::configure_dpi_awareness;

pub const START_HOTKEY: &str = "F1";
pub const STOP_HOTKEY: &str = "F2";
pub const FPS_PRESETS: [u32; 3] = [30, 60, 90];
pub const WATERMARK_POSITION_PRESETS: [WatermarkPosition; 6] = [
    WatermarkPosition::LeftTop,
    WatermarkPosition::RightTop,
    WatermarkPosition::LeftBottom,
    WatermarkPosition::RightBottom,
    WatermarkPosition::MovingFlash,
    WatermarkPosition::RandomWalk,
];
pub const QUALITY_PRESETS: [VideoQuality; 3] =
    [VideoQuality::Low, VideoQuality::Medium, VideoQuality::High];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingConfig {
    pub enabled: bool,
    pub record_audio: bool,
    pub capture_mouse: bool,
    pub fps: u32,
    pub quality: VideoQuality,
    pub watermark: WatermarkConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WatermarkConfig {
    pub enabled: bool,
    pub text: String,
    pub position: WatermarkPosition,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WatermarkPosition {
    LeftTop,
    RightTop,
    LeftBottom,
    RightBottom,
    #[serde(alias = "moving")]
    MovingFlash,
    RandomWalk,
    #[serde(alias = "center")]
    Center,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VideoQuality {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingActivity {
    Idle,
    Selecting,
    Recording,
    Stopping,
    Error,
}

#[derive(Clone, Debug)]
pub struct RecordingSnapshot {
    pub activity: RecordingActivity,
    pub status_line: String,
    pub output_path: Option<PathBuf>,
}

type UpdateCallback = dyn Fn() + Send + Sync + 'static;

pub struct RecordingService {
    inner: Arc<ServiceInner>,
}

struct ServiceInner {
    ffmpeg_path: PathBuf,
    notify: Arc<UpdateCallback>,
    state: Mutex<ServiceState>,
}

struct ServiceState {
    snapshot: RecordingSnapshot,
    active: Option<RecordingSession>,
}

struct RecordingSession {
    ffmpeg_path: PathBuf,
    output_path: PathBuf,
    temp_dir: PathBuf,
    temp_video: PathBuf,
    temp_audio: PathBuf,
    started_at: Instant,
    frame: Option<RecordingFrame>,
    audio: Option<ActiveAudioCapture>,
    video: Option<ActiveVideoCapture>,
    runtime_config: RecordingConfig,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            record_audio: true,
            capture_mouse: true,
            fps: 60,
            quality: VideoQuality::Medium,
            watermark: WatermarkConfig::default(),
        }
    }
}

impl Default for WatermarkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            text: "saaaai.com".to_string(),
            position: WatermarkPosition::RightBottom,
        }
    }
}

impl Default for RecordingSnapshot {
    fn default() -> Self {
        Self {
            activity: RecordingActivity::Idle,
            status_line: "录屏：待机".to_string(),
            output_path: None,
        }
    }
}

impl RecordingConfig {
    pub fn normalize(&mut self) {
        if !matches!(self.fps, 30 | 60 | 90) {
            self.fps = 60;
        }
    }
}

impl WatermarkPosition {
    pub fn label(self) -> &'static str {
        match self {
            Self::LeftTop => "左上",
            Self::RightTop => "右上",
            Self::LeftBottom => "左下",
            Self::RightBottom => "右下",
            Self::MovingFlash => "移动闪现",
            Self::RandomWalk => "随机游走",
            Self::Center => "中间(兼容旧配置)",
        }
    }
}

impl VideoQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
        }
    }

    pub fn crf(self) -> u8 {
        match self {
            Self::Low => 28,
            Self::Medium => 20,
            Self::High => 15,
        }
    }
}

impl RecordingService {
    pub fn start(notify: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let ffmpeg_path = resolve_ffmpeg_path(None)?;
        Ok(Self {
            inner: Arc::new(ServiceInner {
                ffmpeg_path,
                notify: Arc::new(notify),
                state: Mutex::new(ServiceState {
                    snapshot: RecordingSnapshot::default(),
                    active: None,
                }),
            }),
        })
    }

    pub fn snapshot(&self) -> RecordingSnapshot {
        self.inner
            .state
            .lock()
            .map(|state| state.snapshot.clone())
            .unwrap_or_else(|_| RecordingSnapshot {
                activity: RecordingActivity::Error,
                status_line: "录屏：状态锁失败".to_string(),
                output_path: None,
            })
    }

    pub fn begin_recording(&self, mut config: RecordingConfig) -> Result<()> {
        config.normalize();
        {
            let state = self
                .inner
                .state
                .lock()
                .map_err(|_| anyhow!("录屏状态锁失败"))?;
            if state.active.is_some()
                || matches!(
                    state.snapshot.activity,
                    RecordingActivity::Selecting
                        | RecordingActivity::Recording
                        | RecordingActivity::Stopping
                )
            {
                return Err(anyhow!("当前已有录屏流程在进行"));
            }
        }

        self.inner.update_snapshot(RecordingSnapshot {
            activity: RecordingActivity::Selecting,
            status_line: "录屏：按住鼠标拖拽框选，Esc 或右键取消".to_string(),
            output_path: None,
        });

        let inner = self.inner.clone();
        thread::spawn(move || {
            if let Err(error) = begin_recording_impl(inner.clone(), config) {
                tracing::error!(error = %error, "recording start flow failed");
                inner.fail_flow(format!("录屏启动失败：{error}"));
            }
        });
        Ok(())
    }

    pub fn stop_recording(&self) -> Result<()> {
        let session = {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| anyhow!("录屏状态锁失败"))?;
            let Some(session) = state.active.take() else {
                return Err(anyhow!("当前没有正在录制的视频"));
            };
            state.snapshot = RecordingSnapshot {
                activity: RecordingActivity::Stopping,
                status_line: "录屏：正在停止并导出".to_string(),
                output_path: Some(session.output_path.clone()),
            };
            session
        };
        (self.inner.notify)();

        let inner = self.inner.clone();
        thread::spawn(move || {
            if let Err(error) = stop_recording_impl(inner.clone(), session) {
                tracing::error!(error = %error, "recording stop flow failed");
                inner.fail_flow(format!("录屏导出失败：{error}"));
            }
        });
        Ok(())
    }

    pub fn cancel_recording(&self) -> Result<()> {
        let session = {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| anyhow!("录屏状态锁失败"))?;
            if state.snapshot.activity == RecordingActivity::Selecting {
                return Err(anyhow!("当前正在框选，请直接按 Esc 或右键取消"));
            }
            let Some(session) = state.active.take() else {
                return Err(anyhow!("当前没有正在录制的视频"));
            };
            state.snapshot = RecordingSnapshot {
                activity: RecordingActivity::Idle,
                status_line: "录屏：已取消当前录制".to_string(),
                output_path: None,
            };
            session
        };
        (self.inner.notify)();

        thread::spawn(move || {
            session.abort();
        });
        Ok(())
    }
}

impl Drop for RecordingService {
    fn drop(&mut self) {
        if let Ok(mut state) = self.inner.state.lock()
            && let Some(session) = state.active.take()
        {
            session.abort();
        }
    }
}

impl ServiceInner {
    fn update_snapshot(&self, snapshot: RecordingSnapshot) {
        if let Ok(mut state) = self.state.lock() {
            state.snapshot = snapshot;
        }
        (self.notify)();
    }

    fn fail_flow(&self, status_line: String) {
        self.update_snapshot(RecordingSnapshot {
            activity: RecordingActivity::Error,
            status_line,
            output_path: None,
        });
    }
}

fn begin_recording_impl(inner: Arc<ServiceInner>, runtime_config: RecordingConfig) -> Result<()> {
    let Some(region) = choose_region_interactive()? else {
        inner.update_snapshot(RecordingSnapshot {
            activity: RecordingActivity::Idle,
            status_line: "录屏：已取消框选".to_string(),
            output_path: None,
        });
        return Ok(());
    };

    let region = normalize_region_for_encoder(region)?;
    let session = RecordingSession::start(&inner.ffmpeg_path, region, runtime_config)?;
    let output_path = session.output_path.clone();

    {
        let mut state = inner.state.lock().map_err(|_| anyhow!("录屏状态锁失败"))?;
        state.snapshot = RecordingSnapshot {
            activity: RecordingActivity::Recording,
            status_line: format!(
                "录屏：录制中 {}x{}，按 {} 停止，按 Esc 取消",
                region.width, region.height, STOP_HOTKEY
            ),
            output_path: Some(output_path.clone()),
        };
        state.active = Some(session);
    }
    (inner.notify)();
    tracing::info!(
        left = region.left,
        top = region.top,
        width = region.width,
        height = region.height,
        output = %output_path.display(),
        "recording started"
    );
    Ok(())
}

fn stop_recording_impl(inner: Arc<ServiceInner>, session: RecordingSession) -> Result<()> {
    let output = session.stop()?;
    inner.update_snapshot(RecordingSnapshot {
        activity: RecordingActivity::Idle,
        status_line: format!("录屏：已完成 {}", output.display()),
        output_path: Some(output.clone()),
    });
    tracing::info!(output = %output.display(), "recording finished");
    Ok(())
}

impl RecordingSession {
    fn start(
        ffmpeg_path: &Path,
        region: CaptureRegion,
        runtime_config: RecordingConfig,
    ) -> Result<Self> {
        let output_path = default_output_path()?;
        let temp_dir = std::env::temp_dir().join(format!("ainput-record-{}", timestamp_millis()));
        fs::create_dir_all(&temp_dir)
            .with_context(|| format!("创建临时目录失败: {}", temp_dir.display()))?;

        let temp_video = temp_dir.join("video.mkv");
        let temp_audio = temp_dir.join("audio.wav");

        let frame = RecordingFrame::show(region)?;
        let audio = if runtime_config.record_audio {
            match ActiveAudioCapture::start_loopback(temp_audio.clone()) {
                Ok(audio) => Some(audio),
                Err(error) => {
                    frame.close();
                    let _ = cleanup_temp_dir(&temp_dir);
                    return Err(error);
                }
            }
        } else {
            None
        };

        thread::sleep(Duration::from_millis(120));
        let video = match ActiveVideoCapture::start(
            ffmpeg_path,
            region,
            runtime_config.fps,
            runtime_config.capture_mouse,
            runtime_config.quality,
            &temp_video,
        ) {
            Ok(video) => video,
            Err(error) => {
                if let Some(audio) = audio {
                    let _ = audio.stop();
                }
                frame.close();
                let _ = cleanup_temp_dir(&temp_dir);
                return Err(error);
            }
        };

        Ok(Self {
            ffmpeg_path: ffmpeg_path.to_path_buf(),
            output_path,
            temp_dir,
            temp_video,
            temp_audio,
            started_at: Instant::now(),
            frame: Some(frame),
            audio,
            video: Some(video),
            runtime_config,
        })
    }

    fn stop(mut self) -> Result<PathBuf> {
        self.close_frame();

        let video = self
            .video
            .take()
            .ok_or_else(|| anyhow!("录屏视频句柄丢失"))?;
        video.stop()?;

        let video_summary = probe_media(&self.ffmpeg_path, &self.temp_video)?;
        if video_summary.video_streams == 0 {
            self.cleanup_partial_outputs();
            return Err(anyhow!("临时视频没有视频流，无法继续封装"));
        }

        let mut audio_path_for_output = None;
        if let Some(audio) = self.audio.take() {
            let audio_path = audio.stop()?;
            if !audio_path.exists() {
                self.cleanup_partial_outputs();
                return Err(anyhow!("系统音频文件未生成"));
            }
            let audio_summary = probe_media(&self.ffmpeg_path, &audio_path)?;
            let audio_file_size = fs::metadata(&audio_path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            if audio_summary.audio_streams > 0 && audio_file_size > 128 {
                audio_path_for_output = Some(audio_path);
            }
        }

        render_output(
            &self.ffmpeg_path,
            &self.temp_video,
            audio_path_for_output.as_deref(),
            &self.output_path,
            &self.runtime_config,
        )?;

        let output_summary = probe_media(&self.ffmpeg_path, &self.output_path)?;
        if output_summary.video_streams == 0 {
            self.cleanup_partial_outputs();
            return Err(anyhow!("生成的 mp4 没有视频流"));
        }

        if let Err(error) = cleanup_temp_dir(&self.temp_dir) {
            tracing::warn!(error = %error, "cleanup recording temp dir failed");
        }
        tracing::info!(
            seconds = self.started_at.elapsed().as_secs_f32(),
            "recording session stopped"
        );
        Ok(self.output_path)
    }

    fn abort(mut self) {
        self.close_frame();
        if let Some(video) = self.video.take() {
            let _ = video.stop();
        }
        if let Some(audio) = self.audio.take() {
            let _ = audio.stop();
        }
        self.cleanup_partial_outputs();
    }

    fn close_frame(&mut self) {
        if let Some(frame) = self.frame.take() {
            frame.close();
        }
    }

    fn cleanup_partial_outputs(&self) {
        let _ = remove_file_if_exists(&self.output_path);
        let _ = remove_file_if_exists(&self.temp_video);
        let _ = remove_file_if_exists(&self.temp_audio);
        let _ = cleanup_temp_dir(&self.temp_dir);
    }
}

fn render_output(
    ffmpeg_path: &Path,
    temp_video: &Path,
    audio_path: Option<&Path>,
    output_path: &Path,
    config: &RecordingConfig,
) -> Result<()> {
    if config.watermark.enabled && !config.watermark.text.trim().is_empty() {
        render_with_watermark(
            ffmpeg_path,
            temp_video,
            audio_path,
            output_path,
            &config.watermark,
            config.quality,
        )
    } else if let Some(audio_path) = audio_path {
        mux_audio_video(ffmpeg_path, temp_video, audio_path, 0.0, output_path)
    } else {
        mux_video_only(ffmpeg_path, temp_video, output_path)
    }
}

fn cleanup_temp_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let mut last_error = None;
    for _ in 0..10 {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    Err(anyhow!(
        "删除临时目录失败: {} ({})",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "未知错误".to_string())
    ))
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("删除文件失败: {}", path.display()))?;
    }
    Ok(())
}

fn default_output_path() -> Result<PathBuf> {
    let desktop = desktop_dir()?;
    fs::create_dir_all(&desktop)
        .with_context(|| format!("创建桌面目录失败: {}", desktop.display()))?;
    Ok(desktop.join(format!("ainput-record-{}.mp4", timestamp_millis())))
}

fn desktop_dir() -> Result<PathBuf> {
    let user_profile =
        std::env::var("USERPROFILE").context("读取 USERPROFILE 失败，无法定位桌面目录")?;
    Ok(PathBuf::from(user_profile).join("Desktop"))
}

fn timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn normalize_region_for_encoder(mut region: CaptureRegion) -> Result<CaptureRegion> {
    if region.width % 2 != 0 {
        region.width -= 1;
    }
    if region.height % 2 != 0 {
        region.height -= 1;
    }

    if region.width < 2 || region.height < 2 {
        return Err(anyhow!(
            "框选区域过小，无法录屏: {}x{}",
            region.width,
            region.height
        ));
    }

    Ok(region)
}
