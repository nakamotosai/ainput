use std::ffi::OsStr;
use std::io::Write;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use anyhow::{Context, Result, anyhow};

use crate::selection::CaptureRegion;
use crate::{VideoQuality, WatermarkConfig, WatermarkPosition};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

pub struct ActiveVideoCapture {
    child: Child,
    stdin: Option<ChildStdin>,
}

#[derive(Debug, Clone, Copy)]
pub struct MediaSummary {
    pub video_streams: usize,
    pub audio_streams: usize,
    pub video_fps: Option<f64>,
    pub video_frame_count: Option<u64>,
    pub video_duration_secs: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureBackend {
    GdiGrab,
    DdaGrab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VideoEncoderBackend {
    LibX264,
    H264Nvenc,
}

impl ActiveVideoCapture {
    pub fn start(
        ffmpeg_path: &Path,
        region: CaptureRegion,
        monitor_count: usize,
        fps: u32,
        capture_mouse: bool,
        quality: VideoQuality,
        output_path: &Path,
    ) -> Result<Self> {
        let capture_backend = preferred_capture_backend(ffmpeg_path, monitor_count);
        let encoder_backend = preferred_video_encoder(ffmpeg_path);
        let mut command = Command::new(ffmpeg_path);
        configure_background_process(&mut command);
        command
            .arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-nostats");
        append_capture_input_args(&mut command, capture_backend, region, fps, capture_mouse);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .arg("-vsync")
            .arg("cfr")
            .arg("-r")
            .arg(fps.to_string());
        append_video_encoder_args(
            &mut command,
            encoder_backend,
            fps,
            quality,
            capture_backend == CaptureBackend::DdaGrab,
        );
        command.arg("-f").arg("matroska").arg(output_path);

        let mut child = command
            .spawn()
            .with_context(|| format!("启动 FFmpeg 录屏失败: {}", ffmpeg_path.display()))?;
        let stdin = child.stdin.take();
        Ok(Self { child, stdin })
    }

    pub fn stop(mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }

        let output = self
            .child
            .wait_with_output()
            .context("等待 FFmpeg 录屏退出失败")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("FFmpeg 录屏失败: {}", stderr.trim()));
        }
        Ok(())
    }
}

pub fn mux_audio_video(
    ffmpeg_path: &Path,
    video_path: &Path,
    audio_path: &Path,
    audio_trim_secs: f64,
    output_path: &Path,
) -> Result<()> {
    let mut command = Command::new(ffmpeg_path);
    configure_background_process(&mut command);
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostats")
        .arg("-i")
        .arg(video_path);

    if audio_trim_secs > 0.0 {
        command.arg("-ss").arg(format!("{audio_trim_secs:.3}"));
    }

    let output = command
        .arg("-i")
        .arg(audio_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg("1:a:0")
        .arg("-c:v")
        .arg("copy")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-shortest")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output_path)
        .output()
        .with_context(|| format!("启动 FFmpeg 混流失败: {}", ffmpeg_path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("FFmpeg 混流失败: {}", stderr.trim()));
    }
    Ok(())
}

pub fn mux_video_only(ffmpeg_path: &Path, video_path: &Path, output_path: &Path) -> Result<()> {
    let mut command = Command::new(ffmpeg_path);
    configure_background_process(&mut command);
    let output = command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostats")
        .arg("-i")
        .arg(video_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-c:v")
        .arg("copy")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output_path)
        .output()
        .with_context(|| format!("启动 FFmpeg 纯视频封装失败: {}", ffmpeg_path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("FFmpeg 纯视频封装失败: {}", stderr.trim()));
    }

    Ok(())
}

pub fn render_with_watermark(
    ffmpeg_path: &Path,
    video_path: &Path,
    audio_path: Option<&Path>,
    output_path: &Path,
    watermark: &WatermarkConfig,
    quality: VideoQuality,
    fps: u32,
) -> Result<()> {
    let filter = build_drawtext_filter(watermark);
    let encoder_backend = preferred_video_encoder(ffmpeg_path);
    let mut command = Command::new(ffmpeg_path);
    configure_background_process(&mut command);
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostats")
        .arg("-i")
        .arg(video_path);

    if let Some(audio_path) = audio_path {
        command.arg("-i").arg(audio_path);
    }

    command
        .arg("-map")
        .arg("0:v:0")
        .arg("-vf")
        .arg(filter)
        .arg("-vsync")
        .arg("cfr")
        .arg("-r")
        .arg(fps.to_string());
    append_video_encoder_args(&mut command, encoder_backend, fps, quality, false);

    if audio_path.is_some() {
        command
            .arg("-map")
            .arg("1:a:0")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("192k")
            .arg("-shortest");
    }

    let output = command
        .arg("-movflags")
        .arg("+faststart")
        .arg(output_path)
        .output()
        .with_context(|| format!("启动 FFmpeg 水印封装失败: {}", ffmpeg_path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("FFmpeg 水印封装失败: {}", stderr.trim()));
    }

    Ok(())
}

pub fn probe_media(ffmpeg_path: &Path, media_path: &Path) -> Result<MediaSummary> {
    let ffprobe_path = resolve_ffprobe_path(ffmpeg_path);
    let mut command = Command::new(&ffprobe_path);
    configure_background_process(&mut command);
    let output = command
        .arg("-v")
        .arg("error")
        .arg("-count_frames")
        .arg("-show_entries")
        .arg("stream=codec_type,avg_frame_rate,nb_read_frames,duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1")
        .arg(media_path)
        .output()
        .with_context(|| format!("启动 ffprobe 失败: {}", ffprobe_path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffprobe 检查失败: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut summary = MediaSummary {
        video_streams: 0,
        audio_streams: 0,
        video_fps: None,
        video_frame_count: None,
        video_duration_secs: None,
    };
    let mut current_is_video = false;
    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(value) = line.strip_prefix("codec_type=") {
            current_is_video = value == "video";
            match value {
                "video" => summary.video_streams += 1,
                "audio" => summary.audio_streams += 1,
                _ => {}
            }
            continue;
        }

        if !current_is_video {
            continue;
        }

        if let Some(value) = line.strip_prefix("avg_frame_rate=") {
            if summary.video_fps.is_none() {
                summary.video_fps = parse_ffprobe_rational(value);
            }
            continue;
        }

        if let Some(value) = line.strip_prefix("nb_read_frames=") {
            if summary.video_frame_count.is_none() {
                summary.video_frame_count = value.parse::<u64>().ok();
            }
            continue;
        }

        if let Some(value) = line.strip_prefix("duration=")
            && summary.video_duration_secs.is_none()
        {
            summary.video_duration_secs = value.parse::<f64>().ok();
        }
    }

    Ok(summary)
}

fn append_capture_input_args(
    command: &mut Command,
    backend: CaptureBackend,
    region: CaptureRegion,
    fps: u32,
    capture_mouse: bool,
) {
    match backend {
        CaptureBackend::GdiGrab => {
            command
                .arg("-f")
                .arg("gdigrab")
                .arg("-framerate")
                .arg(fps.to_string())
                .arg("-offset_x")
                .arg(region.left.to_string())
                .arg("-offset_y")
                .arg(region.top.to_string())
                .arg("-video_size")
                .arg(format!("{}x{}", region.width, region.height))
                .arg("-draw_mouse")
                .arg(if capture_mouse { "1" } else { "0" })
                .arg("-i")
                .arg("desktop");
        }
        CaptureBackend::DdaGrab => {
            let input = format!(
                "ddagrab=output_idx=0:framerate={fps}:video_size={}x{}:offset_x={}:offset_y={}:draw_mouse={}",
                region.width,
                region.height,
                region.left,
                region.top,
                if capture_mouse { 1 } else { 0 }
            );
            command.arg("-f").arg("lavfi").arg("-i").arg(input);
        }
    }
}

fn append_video_encoder_args(
    command: &mut Command,
    backend: VideoEncoderBackend,
    fps: u32,
    quality: VideoQuality,
    hardware_input: bool,
) {
    match backend {
        VideoEncoderBackend::LibX264 => {
            command
                .arg("-c:v")
                .arg("libx264")
                .arg("-preset")
                .arg(x264_preset_for_fps(fps))
                .arg("-crf")
                .arg(quality.crf().to_string())
                .arg("-x264-params")
                .arg("force-cfr=1")
                .arg("-pix_fmt")
                .arg("yuv420p");
        }
        VideoEncoderBackend::H264Nvenc => {
            command
                .arg("-c:v")
                .arg("h264_nvenc")
                .arg("-preset")
                .arg(nvenc_preset_for_fps(fps))
                .arg("-tune")
                .arg("ull")
                .arg("-rc")
                .arg("vbr")
                .arg("-cq")
                .arg(nvenc_cq_for_quality(quality).to_string())
                .arg("-b:v")
                .arg("0")
                .arg("-profile:v")
                .arg("high");
            if !hardware_input {
                command.arg("-pix_fmt").arg("yuv420p");
            }
        }
    }
}

fn parse_ffprobe_rational(value: &str) -> Option<f64> {
    if let Some((numerator, denominator)) = value.split_once('/') {
        let numerator = numerator.parse::<f64>().ok()?;
        let denominator = denominator.parse::<f64>().ok()?;
        if denominator.abs() < f64::EPSILON {
            return None;
        }
        return Some(numerator / denominator);
    }

    value.parse::<f64>().ok()
}

fn preferred_capture_backend(ffmpeg_path: &Path, monitor_count: usize) -> CaptureBackend {
    if cfg!(windows)
        && monitor_count == 1
        && ffmpeg_supports_filter(ffmpeg_path, "ddagrab")
        && ffmpeg_supports_encoder(ffmpeg_path, "h264_nvenc")
    {
        CaptureBackend::DdaGrab
    } else {
        CaptureBackend::GdiGrab
    }
}

fn preferred_video_encoder(ffmpeg_path: &Path) -> VideoEncoderBackend {
    if cfg!(windows) && ffmpeg_supports_encoder(ffmpeg_path, "h264_nvenc") {
        VideoEncoderBackend::H264Nvenc
    } else {
        VideoEncoderBackend::LibX264
    }
}

fn ffmpeg_supports_filter(ffmpeg_path: &Path, filter_name: &str) -> bool {
    ffmpeg_output_contains(ffmpeg_path, &["-hide_banner", "-filters"], filter_name)
}

fn ffmpeg_supports_encoder(ffmpeg_path: &Path, encoder_name: &str) -> bool {
    ffmpeg_output_contains(ffmpeg_path, &["-hide_banner", "-encoders"], encoder_name)
}

fn ffmpeg_output_contains(ffmpeg_path: &Path, args: &[&str], needle: &str) -> bool {
    let mut command = Command::new(ffmpeg_path);
    configure_background_process(&mut command);
    let output = match command.args(args).output() {
        Ok(output) => output,
        Err(_) => return false,
    };

    if !output.status.success() {
        return false;
    }

    String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn x264_preset_for_fps(fps: u32) -> &'static str {
    match fps {
        0..=60 => "veryfast",
        61..=90 => "superfast",
        _ => "ultrafast",
    }
}

fn nvenc_preset_for_fps(fps: u32) -> &'static str {
    match fps {
        0..=60 => "p4",
        61..=90 => "p2",
        _ => "p1",
    }
}

fn nvenc_cq_for_quality(quality: VideoQuality) -> u8 {
    match quality {
        VideoQuality::Low => 30,
        VideoQuality::Medium => 24,
        VideoQuality::High => 19,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureBackend, VideoEncoderBackend, nvenc_cq_for_quality, nvenc_preset_for_fps,
        parse_ffprobe_rational, preferred_capture_backend, preferred_video_encoder,
        x264_preset_for_fps,
    };
    use crate::VideoQuality;
    use std::path::Path;

    #[test]
    fn parses_ffprobe_rational_fps() {
        assert_eq!(parse_ffprobe_rational("144/1"), Some(144.0));
        assert_eq!(parse_ffprobe_rational("30000/1001"), Some(30000.0 / 1001.0));
    }

    #[test]
    fn picks_faster_x264_preset_for_high_fps() {
        assert_eq!(x264_preset_for_fps(60), "veryfast");
        assert_eq!(x264_preset_for_fps(90), "superfast");
        assert_eq!(x264_preset_for_fps(144), "ultrafast");
    }

    #[test]
    fn picks_faster_nvenc_preset_for_high_fps() {
        assert_eq!(nvenc_preset_for_fps(60), "p4");
        assert_eq!(nvenc_preset_for_fps(90), "p2");
        assert_eq!(nvenc_preset_for_fps(144), "p1");
    }

    #[test]
    fn nvenc_quality_mapping_stays_speed_first() {
        assert_eq!(nvenc_cq_for_quality(VideoQuality::Low), 30);
        assert_eq!(nvenc_cq_for_quality(VideoQuality::Medium), 24);
        assert_eq!(nvenc_cq_for_quality(VideoQuality::High), 19);
    }

    #[test]
    fn prefers_gdigrab_without_single_monitor_hint() {
        assert_eq!(
            preferred_capture_backend(Path::new("ffmpeg"), 2),
            CaptureBackend::GdiGrab
        );
    }

    #[test]
    fn encoder_backend_stays_in_known_set() {
        assert!(matches!(
            preferred_video_encoder(Path::new("ffmpeg")),
            VideoEncoderBackend::LibX264 | VideoEncoderBackend::H264Nvenc
        ));
    }
}

pub fn resolve_ffmpeg_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        return Err(anyhow!("指定的 FFmpeg 不存在: {}", path.display()));
    }

    let candidates = [
        PathBuf::from(r"C:\Users\sai\ffmpeg\bin\ffmpeg.exe"),
        PathBuf::from(r"C:\Users\sai\record\node_modules\ffmpeg-static\ffmpeg.exe"),
        PathBuf::from("ffmpeg.exe"),
        PathBuf::from("ffmpeg"),
    ];

    for candidate in candidates {
        if candidate.components().count() == 1 {
            if command_exists(candidate.as_os_str()) {
                return Ok(candidate);
            }
            continue;
        }
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(anyhow!("未找到可用的 FFmpeg，可用 --ffmpeg 指定路径"))
}

fn resolve_ffprobe_path(ffmpeg_path: &Path) -> PathBuf {
    if let Some(parent) = ffmpeg_path.parent() {
        let candidate = parent.join(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        if candidate.exists() {
            return candidate;
        }
    }

    PathBuf::from(if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    })
}

fn build_drawtext_filter(watermark: &WatermarkConfig) -> String {
    let font_path = r"C\:/Windows/Fonts/arial.ttf";
    let escaped_text = escape_drawtext_text(&watermark.text);
    let (x, y) = watermark_position_expr(watermark.position);
    format!(
        "drawtext=fontfile='{font_path}':text='{escaped_text}':fontcolor=white@0.42:fontsize=30:borderw=2:bordercolor=black@0.45:x={x}:y={y}"
    )
}

fn watermark_position_expr(position: WatermarkPosition) -> (&'static str, &'static str) {
    match position {
        WatermarkPosition::LeftTop => ("20", "20"),
        WatermarkPosition::RightTop => ("w-tw-20", "20"),
        WatermarkPosition::LeftBottom => ("20", "h-th-20"),
        WatermarkPosition::RightBottom => ("w-tw-20", "h-th-20"),
        WatermarkPosition::MovingFlash => (
            "if(eq(mod(t\\,1)\\,0)\\,rand(20\\,(w-tw-20))\\,x)",
            "if(eq(mod(t\\,1)\\,0)\\,rand(20\\,(h-th-20))\\,y)",
        ),
        WatermarkPosition::RandomWalk => (
            "20+(w-tw-40)*(0.5+0.25*sin(t*0.365)+0.25*sin(t*0.655+1.7))",
            "20+(h-th-40)*(0.5+0.25*sin(t*0.455+0.4)+0.25*sin(t*0.765+2.1))",
        ),
        WatermarkPosition::Center => ("(w-tw)/2", "(h-th)/2"),
    }
}

fn escape_drawtext_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'")
        .replace('%', "\\%")
}

fn configure_background_process(command: &mut Command) {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
}

fn command_exists(program: &OsStr) -> bool {
    let mut command = Command::new(program);
    configure_background_process(&mut command);
    command
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
