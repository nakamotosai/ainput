use std::path::PathBuf;
use std::sync::atomic::{AtomicBool as StdAtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, atomic::AtomicBool, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use tracing::{info, warn};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, BeginPaint, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection,
    CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
    DEFAULT_QUALITY, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FF_DONTCARE,
    FillRect, GGI_MARK_NONEXISTING_GLYPHS, GetDC, GetGlyphIndicesW, HBRUSH, HDC, HFONT, HGDIOBJ,
    HPEN, IntersectClipRect, InvalidateRect, LineTo, MoveToEx, OUT_OUTLINE_PRECIS, PAINTSTRUCT,
    PS_SOLID, ReleaseDC, RestoreDC, SaveDC, SelectObject, SetBkColor, SetBkMode, SetTextColor,
    SetWindowRgn, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GWL_EXSTYLE, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, HWND_TOPMOST, IsWindowVisible,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, MSG, PM_REMOVE, PeekMessageW, RegisterClassW,
    SET_WINDOW_POS_FLAGS, SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
    ShowWindow, SystemParametersInfoW, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow,
    WINDOW_STYLE, WM_CTLCOLORSTATIC, WM_NCHITTEST, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::config::{
    HUD_BACKGROUND_ALPHA_MIN_PERCENT, HudAnchor, HudAnimationTheme, HudConfig, HudExpandOrigin,
    HudTextAlign, HudTextEffect, HudUserConfig, HudVisualStyle, alpha_byte_to_percent,
    alpha_percent_to_byte, save_hud_user_config,
};

const HUD_SCREEN_MARGIN_PX: i32 = 8;
const HUD_CHAR_STREAM_INTERVAL: Duration = Duration::from_millis(16);
const HUD_CHAR_STREAM_CATCHUP_TICKS: usize = 8;
const HUD_CHAR_STREAM_MAX_CHARS_PER_TICK: usize = 8;
/// ~144–166 Hz tick so 144 Hz monitors get time-based motion, not 60 Hz steps.
const HUD_TICK_INTERVAL: Duration = Duration::from_millis(6);
const HUD_REQUIRED_STATUS_TEXT: &str = "听写中识别中改写中翻译中已复制中文English日本語";
const LWA_ALPHA_ONLY: LAYERED_WINDOW_ATTRIBUTES_FLAGS = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000002);
const LWA_COLORKEY_ALPHA: LAYERED_WINDOW_ATTRIBUTES_FLAGS =
    LAYERED_WINDOW_ATTRIBUTES_FLAGS(0x00000003);
const HUD_TEXT_COLORKEY: COLORREF = COLORREF(0x00ff00ff);
/// B′ S5 soft-glow orb footprint (fixed — no text resize jump).
/// Room for full disc + soft transparent falloff (per-pixel alpha).
const METER_WIDTH_PX: i32 = 160;
const METER_HEIGHT_PX: i32 = 160;
/// Sit clearly above the taskbar (work-area bottom margin).
const METER_BOTTOM_MARGIN_PX: i32 = 56;
const METER_BOUNCE_AMP_PX: f32 = 28.0;
/// Exit bounce duration (pop back down, mirror of enter spring).
const METER_EXIT_DUR_SEC: f32 = 0.42;
const METER_SENTINEL: &str = "\u{2060}"; // word-joiner; paint ignores text in silent mode

static HUD_TEXT_COLOR: AtomicU32 = AtomicU32::new(0x00111111);
static HUD_BACKGROUND_BRUSH: AtomicIsize = AtomicIsize::new(0);
static HUD_FONT_HANDLE: AtomicIsize = AtomicIsize::new(0);
static HUD_PADDING_X: AtomicI32 = AtomicI32::new(14);
static HUD_PADDING_Y: AtomicI32 = AtomicI32::new(8);
static HUD_CENTER_TEXT: StdAtomicBool = StdAtomicBool::new(true);
static HUD_ANIMATION_THEME: OnceLock<Mutex<HudAnimationTheme>> = OnceLock::new();
static HUD_ACTIVITY_KIND: OnceLock<Mutex<HudActivityKind>> = OnceLock::new();
static HUD_ANIMATION_FRAME: AtomicU32 = AtomicU32::new(0);
static HUD_PAINT_STYLE: OnceLock<Mutex<HudPaintStyle>> = OnceLock::new();
/// Shared mic level (0..=1000) from AudioHub; optional until bound after mic start.
static HUD_AUDIO_LEVEL: OnceLock<Arc<AtomicU32>> = OnceLock::new();
/// Smoothed envelope for live voice bars / particles (0..=1000).
static HUD_ENVELOPE_MILLI: AtomicU32 = AtomicU32::new(0);
/// B′ silent particle meter (no text, transparent bg) for CapsLock local path.
static HUD_SILENT_METER: StdAtomicBool = StdAtomicBool::new(false);
/// Time origin (ms since process) for particle phase; set on first silent show.
static HUD_ANIM_T0_MS: AtomicU32 = AtomicU32::new(0);
static HUD_PROCESS_START: OnceLock<Instant> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum HudActivityKind {
    #[default]
    Text,
    Listening,
    Recognizing,
    Rewriting,
    Translating,
    RawPastedRewriting,
    Done,
    Warning,
}

#[derive(Clone)]
pub struct HudController {
    inner: Arc<HudControllerInner>,
}

struct HudControllerInner {
    tx: mpsc::Sender<HudCommand>,
    appearance: Arc<Mutex<HudFontAppearance>>,
    user_config: Arc<Mutex<HudUserConfig>>,
    user_config_path: Arc<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudFontAppearance {
    pub family: String,
    pub height_px: i32,
    pub weight: i32,
    pub background_alpha_percent: u8,
}

#[derive(Debug, Clone)]
struct HudPaintStyle {
    text_color: COLORREF,
    background_color: COLORREF,
    text_effect: HudTextEffect,
    shadow_enabled: bool,
    shadow_color: COLORREF,
    shadow_alpha: u8,
    shadow_offset_x_px: i32,
    shadow_offset_y_px: i32,
    rainbow_saturation_percent: u8,
    rainbow_lightness_percent: u8,
    rainbow_step_degree: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeterPhase {
    /// Hold-to-talk: particles react to mic level.
    Listening,
    /// Recognizing / rewriting: soft settle particles, no text.
    Busy,
}

enum HudCommand {
    Show {
        message: String,
        persistent: bool,
        char_streaming: bool,
    },
    /// B′ silent particle meter (CapsLock local non-streaming).
    ShowMeter {
        phase: MeterPhase,
    },
    Clear,
    ApplyAppearance(HudFontAppearance),
    ApplyUserConfig(HudUserConfig),
    Shutdown,
}

#[derive(Debug, Clone)]
struct HudStyle {
    visual_style: HudVisualStyle,
    animation_theme: HudAnimationTheme,
    anchor: HudAnchor,
    expand_origin: HudExpandOrigin,
    offset_x_px: i32,
    offset_y_px: i32,
    width_px: i32,
    height_px: i32,
    min_width_px: i32,
    min_height_px: i32,
    min_text_width_px: i32,
    padding_x_px: i32,
    padding_y_px: i32,
    font_height_px: i32,
    font_weight: i32,
    font_family: String,
    text_align: HudTextAlign,
    text_color: COLORREF,
    text_alpha: u8,
    text_effect: HudTextEffect,
    shadow_enabled: bool,
    shadow_color: COLORREF,
    shadow_alpha: u8,
    shadow_offset_x_px: i32,
    shadow_offset_y_px: i32,
    rainbow_saturation_percent: u8,
    rainbow_lightness_percent: u8,
    rainbow_step_degree: u16,
    background_color: COLORREF,
    background_alpha: u8,
    corner_radius_px: i32,
    display_min: Duration,
}

struct HudUi {
    window: HudWindow,
    stream: HudMicrostreamState,
    message: String,
    placeholder_active: bool,
    char_streaming: bool,
    last_char_tick_at: Instant,
    last_tick_at: Instant,
    persistent: bool,
    hold_until: Option<Instant>,
    visibility: f32,
    shown: bool,
    activity_kind: HudActivityKind,
    /// B′: particle-only transparent meter (no dark rect, no status text).
    silent_meter: bool,
    /// When the current enter bounce started (for spring-from-bottom).
    enter_at: Option<Instant>,
    /// When silent exit bounce started (pop back down); None while idle/enter.
    exit_at: Option<Instant>,
}

struct HudWindow {
    hwnd: HWND,
    text_hwnd: HWND,
    brush: HBRUSH,
    font: HFONT,
    style: HudStyle,
}

#[derive(Debug, Clone, Default)]
struct HudMicrostreamState {
    committed_prefix: String,
    target_suffix: String,
    display_suffix: String,
}

impl HudController {
    pub fn start(
        mut config: HudConfig,
        user_config_path: PathBuf,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (tx, rx) = mpsc::channel::<HudCommand>();
        let requested_appearance = HudFontAppearance::from_config(&config);
        let effective_appearance = match validate_font_appearance_for_hud(&requested_appearance) {
            Ok(appearance) => appearance,
            Err(error) => {
                warn!(
                    error = %error,
                    ?requested_appearance,
                    "HUD startup font rejected; using default HUD appearance without blocking startup"
                );
                HudFontAppearance::default()
            }
        };
        effective_appearance.apply_to_config(&mut config);
        let appearance = Arc::new(Mutex::new(effective_appearance));
        let user_config = Arc::new(Mutex::new(HudUserConfig::from_config(&config)));
        let inner = Arc::new(HudControllerInner {
            tx,
            appearance,
            user_config,
            user_config_path: Arc::new(user_config_path),
        });
        thread::spawn(move || {
            let result = run_hud_thread(config, rx, shutdown, ready_tx);
            if let Err(error) = result {
                warn!(error = %error, "HUD thread failed");
            }
        });
        ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("HUD thread did not initialize"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Self { inner })
    }

    pub fn show_text(&self, message: &str, persistent: bool, char_streaming: bool) {
        let _ = self.inner.tx.send(HudCommand::Show {
            message: message.to_string(),
            persistent,
            char_streaming,
        });
    }

    pub fn show_active(&self) {
        self.show_text("听写中", true, false);
    }

    /// B′ silent listening meter (CapsLock local): particles only, transparent, bounce-in.
    pub fn show_meter_listening(&self) {
        let _ = self.inner.tx.send(HudCommand::ShowMeter {
            phase: MeterPhase::Listening,
        });
    }

    /// B′ silent busy meter (recognize / rewrite): soft particles, no text.
    pub fn show_meter_busy(&self) {
        let _ = self
            .inner
            .tx
            .send(HudCommand::ShowMeter {
                phase: MeterPhase::Busy,
            });
    }

    /// Bind resident mic level so Listening HUD can draw a real meter (design B).
    pub fn bind_audio_level(&self, level: Arc<AtomicU32>) {
        if HUD_AUDIO_LEVEL.set(level).is_err() {
            warn!("HUD audio level already bound; ignoring duplicate bind");
        }
    }

    pub fn clear(&self) {
        let _ = self.inner.tx.send(HudCommand::Clear);
    }

    pub fn font_appearance(&self) -> HudFontAppearance {
        self.inner
            .appearance
            .lock()
            .map(|appearance| appearance.clone())
            .unwrap_or_else(|_| HudFontAppearance::default())
    }

    pub fn apply_font_appearance(&self, appearance: &HudFontAppearance) -> Result<()> {
        let appearance = validate_font_appearance_for_hud(appearance)?;
        if let Ok(mut stored) = self.inner.appearance.lock() {
            if *stored == appearance {
                return Ok(());
            }
            *stored = appearance.clone();
        } else {
            return Err(anyhow!("HUD appearance state lock failed"));
        }
        let user = {
            let mut user = self
                .inner
                .user_config
                .lock()
                .map_err(|_| anyhow!("HUD user config state lock failed"))?;
            user.font_family = Some(appearance.family.clone());
            user.font_height_px = Some(appearance.height_px);
            user.font_weight = Some(appearance.weight);
            user.background_alpha_percent = Some(appearance.background_alpha_percent);
            user.clone()
        };
        self.persist_user_config(&user);
        let _ = self
            .inner
            .tx
            .send(HudCommand::ApplyAppearance(appearance.clone()));
        Ok(())
    }

    pub fn animation_theme(&self) -> HudAnimationTheme {
        self.inner
            .user_config
            .lock()
            .ok()
            .and_then(|user| user.animation_theme)
            .unwrap_or_default()
    }

    pub fn set_animation_theme(&self, theme: HudAnimationTheme) -> Result<()> {
        let user = {
            let mut user = self
                .inner
                .user_config
                .lock()
                .map_err(|_| anyhow!("HUD user config state lock failed"))?;
            user.animation_theme = Some(theme);
            user.clone()
        };
        self.persist_user_config(&user);
        let _ = self.inner.tx.send(HudCommand::ApplyUserConfig(user));
        Ok(())
    }

    pub fn hud_user_config(&self) -> HudUserConfig {
        self.inner
            .user_config
            .lock()
            .map(|config| config.clone())
            .unwrap_or_else(|_| HudUserConfig::from_config(&HudConfig::default()))
    }

    pub fn apply_hud_user_config(&self, next: HudUserConfig) -> Result<HudUserConfig> {
        let merged = {
            let mut user = self
                .inner
                .user_config
                .lock()
                .map_err(|_| anyhow!("HUD user config state lock failed"))?;
            user.merge(next);
            normalize_hud_user_config(user.clone())?
        };
        let mut config = HudConfig::default();
        config.apply_user_config(&merged);
        let appearance = HudFontAppearance::from_config(&config);
        validate_font_appearance_for_hud(&appearance)?;
        if let Ok(mut stored) = self.inner.appearance.lock() {
            *stored = appearance;
        }
        {
            let mut user = self
                .inner
                .user_config
                .lock()
                .map_err(|_| anyhow!("HUD user config state lock failed"))?;
            *user = merged.clone();
        }
        self.persist_user_config(&merged);
        let _ = self
            .inner
            .tx
            .send(HudCommand::ApplyUserConfig(merged.clone()));
        Ok(merged)
    }

    fn persist_user_config(&self, user: &HudUserConfig) {
        if let Err(error) = save_hud_user_config(&self.inner.user_config_path, &user) {
            warn!(error = %error, path = %self.inner.user_config_path.display(), "save HUD appearance user config failed");
        }
    }
}

fn normalize_hud_user_config(mut user: HudUserConfig) -> Result<HudUserConfig> {
    let mut config = HudConfig::default();
    config.apply_user_config(&user);
    if config.auto_font_fit {
        let height = auto_fit_font_height_px(config.height_px, config.padding_y_px);
        user.font_height_px = Some(height);
    }
    user.width_px = Some(config.width_px.clamp(120, 10_000));
    user.height_px = Some(config.height_px.clamp(24, 1000));
    user.padding_x_px = Some(config.padding_x_px.clamp(0, 96));
    user.padding_y_px = Some(config.padding_y_px.clamp(0, 48));
    user.offset_x_px = Some(config.offset_x_px.clamp(-10_000, 10_000));
    user.offset_y_px = Some(config.offset_y_px.clamp(-10_000, 10_000));
    user.background_alpha_percent = Some(alpha_byte_to_percent(config.background_alpha));
    user.text_color = Some(config.text_color.clone());
    user.text_alpha_percent = Some(alpha_byte_to_percent(config.text_alpha));
    user.background_color = Some(config.background_color.clone());
    user.text_effect = Some(config.text_effect);
    user.shadow_enabled = Some(config.shadow_enabled);
    user.shadow_color = Some(config.shadow_color.clone());
    user.shadow_alpha_percent = Some(alpha_byte_to_percent(config.shadow_alpha));
    user.shadow_offset_x_px = Some(config.shadow_offset_x_px.clamp(-32, 32));
    user.shadow_offset_y_px = Some(config.shadow_offset_y_px.clamp(-32, 32));
    user.rainbow_saturation_percent = Some(config.rainbow_saturation_percent.clamp(0, 100));
    user.rainbow_lightness_percent = Some(config.rainbow_lightness_percent.clamp(0, 100));
    user.rainbow_step_degree = Some(config.rainbow_step_degree.clamp(1, 180));
    Ok(user)
}

impl Drop for HudControllerInner {
    fn drop(&mut self) {
        let _ = self.tx.send(HudCommand::Shutdown);
    }
}

impl HudFontAppearance {
    fn from_config(config: &HudConfig) -> Self {
        let mut appearance = Self {
            family: config.font_family.clone(),
            height_px: config.font_height_px,
            weight: config.font_weight,
            background_alpha_percent: alpha_byte_to_percent(config.background_alpha),
        };
        appearance.normalize();
        appearance
    }

    fn normalize(&mut self) {
        if self.family.trim().is_empty() {
            self.family = Self::default().family;
        } else {
            self.family = self.family.trim().to_string();
        }
        self.height_px = normalize_font_height_px(self.height_px);
        self.weight = self.weight.clamp(100, 900);
        self.background_alpha_percent = self
            .background_alpha_percent
            .clamp(HUD_BACKGROUND_ALPHA_MIN_PERCENT, 100);
    }

    fn apply_to_config(&self, config: &mut HudConfig) {
        config.font_family = self.family.clone();
        config.font_height_px = self.height_px;
        config.font_weight = self.weight;
        config.background_alpha = alpha_percent_to_byte(self.background_alpha_percent);
    }
}

pub fn validate_font_appearance_for_hud(
    appearance: &HudFontAppearance,
) -> Result<HudFontAppearance> {
    let mut normalized = appearance.clone();
    normalized.normalize();
    validate_hud_font_status_glyphs(&normalized.family, normalized.height_px, normalized.weight)?;
    Ok(normalized)
}

impl HudActivityKind {
    fn from_message(message: &str) -> Self {
        let message = message.trim();
        if message == "听写中" {
            Self::Listening
        } else if message == "识别中..." || message == "本地识别中..." {
            Self::Recognizing
        } else if message == "改写中..." {
            Self::Rewriting
        } else if message == "翻译中..." {
            Self::Translating
        } else if message == "已上屏，改写中..." {
            Self::RawPastedRewriting
        } else if message == "已复制"
            || message.contains("不可用")
            || message.contains("未上屏")
            || message.contains("未改写")
            || message.contains("原文保留")
        {
            Self::Warning
        } else if message.starts_with("已替换：") {
            Self::Done
        } else {
            Self::Text
        }
    }

    /// Only in-progress stages paint decoration-only; Done/Warning must show body text.
    fn is_animated(self) -> bool {
        matches!(
            self,
            Self::Listening
                | Self::Recognizing
                | Self::Rewriting
                | Self::Translating
                | Self::RawPastedRewriting
        )
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Text => "",
            Self::Listening => "听写",
            Self::Recognizing => "识别",
            Self::Rewriting => "改写",
            Self::Translating => "翻译",
            Self::RawPastedRewriting => "改写",
            Self::Done => "完成",
            Self::Warning => "提示",
        }
    }
}

fn store_hud_animation_theme(theme: HudAnimationTheme) {
    let stored = HUD_ANIMATION_THEME.get_or_init(|| Mutex::new(theme));
    if let Ok(mut current) = stored.lock() {
        *current = theme;
    }
}

fn current_hud_animation_theme() -> HudAnimationTheme {
    HUD_ANIMATION_THEME
        .get()
        .and_then(|stored| stored.lock().ok().map(|value| *value))
        .unwrap_or_default()
}

fn store_hud_activity_kind(kind: HudActivityKind) {
    let stored = HUD_ACTIVITY_KIND.get_or_init(|| Mutex::new(kind));
    if let Ok(mut current) = stored.lock() {
        *current = kind;
    }
}

fn current_hud_activity_kind() -> HudActivityKind {
    HUD_ACTIVITY_KIND
        .get()
        .and_then(|stored| stored.lock().ok().map(|value| *value))
        .unwrap_or_default()
}

impl Default for HudFontAppearance {
    fn default() -> Self {
        Self {
            family: "Microsoft YaHei".to_string(),
            height_px: 32,
            weight: 700,
            background_alpha_percent: alpha_byte_to_percent(224),
        }
    }
}

fn run_hud_thread(
    config: HudConfig,
    rx: mpsc::Receiver<HudCommand>,
    shutdown: Arc<AtomicBool>,
    ready_tx: mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    let mut ui = match HudUi::create(&config) {
        Ok(ui) => {
            let _ = ready_tx.send(Ok(()));
            ui
        }
        Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
            return Err(error);
        }
    };
    info!("HUD thread started");

    while !shutdown.load(Ordering::Relaxed) {
        while pump_messages()? {}
        match rx.recv_timeout(HUD_TICK_INTERVAL) {
            Ok(HudCommand::Show {
                message,
                persistent,
                char_streaming,
            }) => ui.show_status(&message, persistent, char_streaming),
            Ok(HudCommand::ShowMeter { phase }) => ui.show_meter(phase),
            Ok(HudCommand::Clear) => ui.clear(),
            Ok(HudCommand::ApplyAppearance(appearance)) => ui.apply_appearance(&appearance),
            Ok(HudCommand::ApplyUserConfig(user)) => ui.apply_user_config(&user),
            Ok(HudCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        ui.tick();
    }

    info!("HUD thread stopped");
    Ok(())
}

impl HudUi {
    fn create(config: &HudConfig) -> Result<Self> {
        let style = HudStyle::from_config(config);
        unsafe {
            let instance = GetModuleHandleW(None)
                .map_err(|error| anyhow!("resolve module handle: {error}"))?;
            let instance = HINSTANCE(instance.0);
            let brush_color = style.background_color;
            let brush = CreateSolidBrush(brush_color);
            if brush.is_invalid() {
                return Err(anyhow!("create HUD brush failed"));
            }
            register_hud_class(instance, brush)?;
            let window = create_hud_window(instance, brush, &style)?;
            Ok(Self {
                window,
                stream: HudMicrostreamState::default(),
                message: String::new(),
                placeholder_active: false,
                char_streaming: false,
                last_char_tick_at: Instant::now(),
                last_tick_at: Instant::now(),
                persistent: false,
                hold_until: None,
                visibility: 0.0,
                shown: false,
                activity_kind: HudActivityKind::Text,
                silent_meter: false,
                enter_at: None,
                exit_at: None,
            })
        }
    }

    fn show_meter(&mut self, phase: MeterPhase) {
        let now = Instant::now();
        let was_silent = self.silent_meter && self.shown && self.exit_at.is_none();
        self.silent_meter = true;
        HUD_SILENT_METER.store(true, Ordering::Relaxed);
        ensure_anim_clock();
        let kind = match phase {
            MeterPhase::Listening => HudActivityKind::Listening,
            MeterPhase::Busy => HudActivityKind::Recognizing,
        };
        self.activity_kind = kind;
        store_hud_activity_kind(kind);
        self.stream.set_immediate(METER_SENTINEL);
        self.message = METER_SENTINEL.to_string();
        self.placeholder_active = false;
        self.char_streaming = false;
        self.persistent = true;
        self.hold_until = Some(now + self.window.style.display_min);
        // Cancel any exit-in-progress; bounce-in when freshly appearing.
        self.exit_at = None;
        if !was_silent {
            self.enter_at = Some(now);
            if HUD_ANIM_T0_MS.load(Ordering::Relaxed) == 0 {
                HUD_ANIM_T0_MS.store(process_ms(), Ordering::Relaxed);
            }
        }
        self.window.resize_to_meter();
        self.window.set_status_text_raw(METER_SENTINEL);
        info!(?phase, "HUD silent particle meter shown");
    }

    fn show_status(&mut self, message: &str, persistent: bool, char_streaming: bool) {
        let message = message.trim();
        let now = Instant::now();
        info!(
            message_chars = message.chars().count(),
            persistent,
            char_streaming,
            text = %short_hud_text(message, 160),
            "HUD show command received"
        );

        // Leaving silent meter for normal text HUD.
        if self.silent_meter {
            self.silent_meter = false;
            HUD_SILENT_METER.store(false, Ordering::Relaxed);
            self.enter_at = None;
            self.exit_at = None;
            self.window.restore_text_colorkey_layer();
        }

        if message.is_empty() {
            self.stream.clear();
            self.message.clear();
            self.persistent = false;
            self.hold_until = Some(now);
            self.placeholder_active = false;
            self.char_streaming = false;
            self.activity_kind = HudActivityKind::Text;
            store_hud_activity_kind(self.activity_kind);
            self.window.set_status_text("", false);
            return;
        }

        self.activity_kind = HudActivityKind::from_message(message);
        store_hud_activity_kind(self.activity_kind);
        if !char_streaming {
            self.placeholder_active = false;
            if self.stream.display_message() != message || self.message != message {
                self.stream.set_immediate(message);
                self.message = message.to_string();
                self.window.set_status_text(message, false);
            }
            self.char_streaming = false;
            self.last_char_tick_at = now;
        } else {
            self.placeholder_active = false;
            if self.stream.target_message() != message || self.message != message {
                self.stream.retarget(message);
                if self.stream.display_message().trim().is_empty()
                    && self.stream.has_pending_chars()
                {
                    self.stream.advance_one_char();
                }
                let display_message = self.stream.display_message();
                self.message = message.to_string();
                self.window.set_status_text(&display_message, true);
                self.char_streaming = self.stream.has_pending_chars();
            } else {
                self.char_streaming = self.stream.has_pending_chars();
            }
            self.last_char_tick_at = now;
        }

        self.persistent = persistent;
        self.hold_until = Some(now + self.window.style.display_min);
    }

    fn clear(&mut self) {
        self.stream.clear();
        self.message.clear();
        self.persistent = false;
        // Start fade-out clock; silent meter keeps drawing until exit bounce finishes.
        self.hold_until = Some(Instant::now());
        self.placeholder_active = false;
        self.char_streaming = false;
        // Keep silent_meter true during exit so lamp pops down (not instant vanish).
        if !self.silent_meter {
            self.activity_kind = HudActivityKind::Text;
            store_hud_activity_kind(self.activity_kind);
            self.window.set_status_text("", false);
        } else {
            if self.exit_at.is_none() {
                self.exit_at = Some(Instant::now());
                self.enter_at = None;
            }
            // Keep sentinel so last frames still present lamp while bouncing out.
            self.window.set_status_text_raw(METER_SENTINEL);
        }
    }

    fn apply_appearance(&mut self, appearance: &HudFontAppearance) {
        let visible_text = self.stream.display_message();
        if let Err(error) = self.window.apply_appearance(appearance, &visible_text) {
            warn!(error = %error, ?appearance, "apply HUD appearance failed");
        }
    }

    fn apply_user_config(&mut self, user: &HudUserConfig) {
        let visible_text = self.stream.display_message();
        if let Err(error) = self.window.apply_user_config(user, &visible_text) {
            warn!(error = %error, ?user, "apply HUD user config failed");
        }
        self.show_status("HUD 预览：拖动参数会直接改这里", true, false);
    }

    fn tick(&mut self) {
        self.advance_char_stream();

        let now = Instant::now();
        let dt = now
            .duration_since(self.last_tick_at)
            .as_secs_f32()
            .clamp(0.001, 0.05);
        self.last_tick_at = now;

        let should_show =
            self.persistent || self.hold_until.is_some_and(|until| Instant::now() <= until);
        // Silent exit: visibility driven by bounce-down progress (stay lit while leaving).
        // Silent enter/hold: snappy in, soft out only if no exit_at (fallback).
        if self.silent_meter {
            if let Some(exit_start) = self.exit_at {
                let p = (now.duration_since(exit_start).as_secs_f32() / METER_EXIT_DUR_SEC)
                    .clamp(0.0, 1.0);
                // Bright through first third, then ease off as it drops.
                self.visibility = if p < 0.30 {
                    1.0
                } else {
                    let u = ((p - 0.30) / 0.70).clamp(0.0, 1.0);
                    (1.0 - u * u).clamp(0.0, 1.0)
                };
            } else {
                let fade_k = if should_show {
                    1.0 - (-14.0 * dt).exp()
                } else {
                    1.0 - (-3.2 * dt).exp()
                };
                self.visibility = smooth_step(
                    self.visibility,
                    if should_show { 1.0 } else { 0.0 },
                    fade_k,
                );
            }
        } else {
            self.visibility = smooth_step(
                self.visibility,
                if should_show { 1.0 } else { 0.0 },
                0.18,
            );
        }

        if self.visibility > 0.01 && !self.shown {
            if self.silent_meter {
                self.window.show_meter_surface();
            } else {
                self.window.show();
            }
            self.shown = true;
        }
        // Keep silent surface alive through exit bounce even when almost faded.
        if self.silent_meter && self.exit_at.is_some() && !self.shown {
            self.window.show_meter_surface();
            self.shown = true;
        }
        if self.shown {
            if self.silent_meter {
                let bounce_y = meter_bounce_y(self.enter_at, self.exit_at, now);
                self.window.reposition_meter(bounce_y);
            } else {
                let alpha =
                    (self.window.style.background_alpha as f32 * self.visibility).round() as u8;
                self.window.set_alpha(alpha);
            }
        }
        let silent_exit_done = self
            .exit_at
            .map(|start| now.duration_since(start).as_secs_f32() >= METER_EXIT_DUR_SEC)
            .unwrap_or(true);
        let hide_silent = self.silent_meter
            && !should_show
            && self.shown
            && silent_exit_done
            && self.visibility < 0.02;
        let hide_text = !self.silent_meter && self.visibility < 0.01 && self.shown && !should_show;
        if hide_silent || hide_text {
            self.window.hide();
            self.shown = false;
            self.silent_meter = false;
            HUD_SILENT_METER.store(false, Ordering::Relaxed);
            self.enter_at = None;
            self.exit_at = None;
            self.activity_kind = HudActivityKind::Text;
            store_hud_activity_kind(self.activity_kind);
            HUD_ENVELOPE_MILLI.store(0, Ordering::Relaxed);
        }
        let animating = self.shown
            && (self.silent_meter
                || (self.activity_kind.is_animated()
                    && self.window.style.animation_theme != HudAnimationTheme::TextOnly));
        if animating {
            HUD_ANIMATION_FRAME.fetch_add(1, Ordering::Relaxed);
            if self.silent_meter {
                if self.exit_at.is_none() && self.activity_kind == HudActivityKind::Listening {
                    update_listening_envelope_dt(dt);
                } else if self.exit_at.is_none() {
                    settle_busy_envelope_dt(dt);
                }
                // Per-pixel alpha lamp (do NOT Invalidate — WM_PAINT would wipe ULW).
                let envelope = HUD_ENVELOPE_MILLI.load(Ordering::Relaxed);
                let t = anim_time_sec();
                self.window.present_s5_glow(
                    envelope,
                    t,
                    self.activity_kind,
                    self.visibility.clamp(0.0, 1.0),
                );
            } else if self.activity_kind == HudActivityKind::Listening {
                update_listening_envelope_dt(dt);
                self.window.invalidate();
            } else {
                HUD_ENVELOPE_MILLI.store(0, Ordering::Relaxed);
                self.window.invalidate();
            }
        } else if !self.shown {
            HUD_ENVELOPE_MILLI.store(0, Ordering::Relaxed);
        }
    }

    fn advance_char_stream(&mut self) {
        if !self.char_streaming || !self.stream.has_pending_chars() {
            self.char_streaming = false;
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.last_char_tick_at) < HUD_CHAR_STREAM_INTERVAL {
            return;
        }
        let advance_chars = self
            .stream
            .pending_char_count()
            .div_ceil(HUD_CHAR_STREAM_CATCHUP_TICKS)
            .clamp(1, HUD_CHAR_STREAM_MAX_CHARS_PER_TICK);
        self.stream.advance_chars(advance_chars);
        let next_display = self.stream.display_message();
        self.window.set_status_text(&next_display, true);
        self.last_char_tick_at = now;
        self.char_streaming = self.stream.has_pending_chars();
    }
}

impl HudStyle {
    fn from_config(config: &HudConfig) -> Self {
        Self {
            visual_style: config.style,
            animation_theme: config.animation_theme,
            anchor: config.anchor,
            expand_origin: config.expand_origin,
            offset_x_px: config.offset_x_px,
            offset_y_px: config.offset_y_px,
            width_px: config.width_px.max(220),
            height_px: config.height_px.max(config.min_height_px).clamp(24, 1000),
            min_width_px: config.min_width_px.max(36),
            min_height_px: config.min_height_px.max(36),
            min_text_width_px: config.min_text_width_px.max(1),
            padding_x_px: config.padding_x_px.max(8),
            padding_y_px: config.padding_y_px.max(6),
            font_height_px: if config.auto_font_fit {
                auto_fit_font_height_px(config.height_px, config.padding_y_px)
            } else {
                normalize_font_height_px(config.font_height_px)
            },
            font_weight: config.font_weight.clamp(100, 900),
            font_family: if config.font_family.trim().is_empty() {
                "Microsoft YaHei".to_string()
            } else {
                config.font_family.trim().to_string()
            },
            text_align: config.text_align,
            text_color: parse_color_ref(&config.text_color, "#FFFFFF"),
            text_alpha: config.text_alpha,
            text_effect: config.text_effect,
            shadow_enabled: config.shadow_enabled,
            shadow_color: parse_color_ref(&config.shadow_color, "#000000"),
            shadow_alpha: config.shadow_alpha,
            shadow_offset_x_px: config.shadow_offset_x_px.clamp(-32, 32),
            shadow_offset_y_px: config.shadow_offset_y_px.clamp(-32, 32),
            rainbow_saturation_percent: config.rainbow_saturation_percent.clamp(0, 100),
            rainbow_lightness_percent: config.rainbow_lightness_percent.clamp(0, 100),
            rainbow_step_degree: config.rainbow_step_degree.clamp(1, 180),
            background_color: parse_color_ref(&config.background_color, "#0B0B0B"),
            background_alpha: config.background_alpha,
            corner_radius_px: config.corner_radius_px.clamp(0, 120),
            display_min: Duration::from_millis(config.display_hold_ms.clamp(100, 10_000)),
        }
    }
}

impl HudPaintStyle {
    fn from_style(style: &HudStyle) -> Self {
        Self {
            text_color: style.text_color,
            background_color: style.background_color,
            text_effect: style.text_effect,
            shadow_enabled: style.shadow_enabled,
            shadow_color: style.shadow_color,
            shadow_alpha: style.shadow_alpha,
            shadow_offset_x_px: style.shadow_offset_x_px,
            shadow_offset_y_px: style.shadow_offset_y_px,
            rainbow_saturation_percent: style.rainbow_saturation_percent,
            rainbow_lightness_percent: style.rainbow_lightness_percent,
            rainbow_step_degree: style.rainbow_step_degree,
        }
    }
}

fn store_hud_paint_style(style: &HudStyle) {
    let paint = HudPaintStyle::from_style(style);
    let stored = HUD_PAINT_STYLE.get_or_init(|| Mutex::new(paint.clone()));
    if let Ok(mut current) = stored.lock() {
        *current = paint;
    }
    store_hud_animation_theme(style.animation_theme);
    HUD_TEXT_COLOR.store(style.text_color.0, Ordering::Relaxed);
}

fn current_hud_paint_style() -> HudPaintStyle {
    HUD_PAINT_STYLE
        .get()
        .and_then(|stored| stored.lock().ok().map(|value| value.clone()))
        .unwrap_or(HudPaintStyle {
            text_color: COLORREF(HUD_TEXT_COLOR.load(Ordering::Relaxed)),
            background_color: COLORREF(0x00141007),
            text_effect: HudTextEffect::Solid,
            shadow_enabled: false,
            shadow_color: COLORREF(0),
            shadow_alpha: 160,
            shadow_offset_x_px: 1,
            shadow_offset_y_px: 1,
            rainbow_saturation_percent: 45,
            rainbow_lightness_percent: 78,
            rainbow_step_degree: 28,
        })
}

unsafe fn register_hud_class(instance: HINSTANCE, brush: HBRUSH) -> Result<()> {
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(hud_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("ainput2_hud_surface"),
        hbrBackground: brush,
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    Ok(())
}

unsafe fn create_hud_window(
    instance: HINSTANCE,
    brush: HBRUSH,
    style: &HudStyle,
) -> Result<HudWindow> {
    let initial_width = style.width_px.max(style.min_width_px);
    let initial_height = style.height_px.max(style.min_height_px);

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("ainput2_hud_surface"),
            w!(""),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            initial_width,
            initial_height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create HUD window failed: {error}"))?;

    unsafe { apply_rounded_region(hwnd, initial_width, initial_height, style.corner_radius_px)? };
    unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA_ONLY) }
        .map_err(|_| anyhow!("configure HUD transparency failed"))?;

    let text_hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("ainput2_hud_surface"),
            w!(""),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            initial_width,
            initial_height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| anyhow!("create HUD text failed: {error}"))?;
    unsafe {
        apply_rounded_region(
            text_hwnd,
            initial_width,
            initial_height,
            style.corner_radius_px,
        )?
    };
    unsafe { SetLayeredWindowAttributes(text_hwnd, HUD_TEXT_COLORKEY, 0, LWA_COLORKEY_ALPHA) }
        .map_err(|_| anyhow!("configure HUD text transparency failed"))?;

    let font = unsafe { create_hud_font(style) };
    if font.is_invalid() {
        let _ = unsafe { DestroyWindow(text_hwnd) };
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(anyhow!("create HUD font failed"));
    }

    HUD_TEXT_COLOR.store(style.text_color.0, Ordering::Relaxed);
    HUD_BACKGROUND_BRUSH.store(brush.0 as isize, Ordering::Relaxed);
    HUD_FONT_HANDLE.store(font.0 as isize, Ordering::Relaxed);
    HUD_PADDING_X.store(style.padding_x_px, Ordering::Relaxed);
    HUD_PADDING_Y.store(style.padding_y_px, Ordering::Relaxed);
    HUD_CENTER_TEXT.store(style.text_align == HudTextAlign::Center, Ordering::Relaxed);
    store_hud_paint_style(style);

    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = ShowWindow(text_hwnd, SW_HIDE);
    }

    Ok(HudWindow {
        hwnd,
        text_hwnd,
        brush,
        font,
        style: style.clone(),
    })
}

unsafe fn create_hud_font(style: &HudStyle) -> HFONT {
    let font_family = HSTRING::from(style.font_family.as_str());
    unsafe {
        CreateFontW(
            -style.font_height_px.abs(),
            0,
            0,
            0,
            style.font_weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_OUTLINE_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            u32::from(DEFAULT_PITCH.0 | FF_DONTCARE.0),
            PCWSTR(font_family.as_ptr()),
        )
    }
}

fn validate_hud_font_status_glyphs(family: &str, height_px: i32, weight: i32) -> Result<()> {
    unsafe {
        let hdc = GetDC(None);
        if hdc.0.is_null() {
            return Err(anyhow!("get screen DC for HUD font validation failed"));
        }
        let family = HSTRING::from(family);
        let font = CreateFontW(
            -normalize_font_height_px(height_px).abs(),
            0,
            0,
            0,
            weight.clamp(100, 900),
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_OUTLINE_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            u32::from(DEFAULT_PITCH.0 | FF_DONTCARE.0),
            PCWSTR(family.as_ptr()),
        );
        if font.is_invalid() {
            let _ = ReleaseDC(None, hdc);
            return Err(anyhow!("create HUD validation font failed"));
        }
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        let sample = HUD_REQUIRED_STATUS_TEXT.encode_utf16().collect::<Vec<_>>();
        let mut glyphs = vec![0u16; sample.len()];
        let result = GetGlyphIndicesW(
            hdc,
            PCWSTR(sample.as_ptr()),
            sample.len() as i32,
            glyphs.as_mut_ptr(),
            GGI_MARK_NONEXISTING_GLYPHS,
        );
        let _ = SelectObject(hdc, old_font);
        let _ = DeleteObject(font.into());
        let _ = ReleaseDC(None, hdc);
        if result == u32::MAX {
            return Err(anyhow!("check HUD font glyphs failed"));
        }
        if glyphs.iter().any(|glyph| *glyph == 0xffff) {
            return Err(anyhow!("字体不能完整显示 HUD 状态文字"));
        }
        Ok(())
    }
}

impl HudWindow {
    fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = ShowWindow(self.text_hwnd, SW_SHOWNOACTIVATE);
        }
        info!(rect = ?self.rect_array(), "HUD window shown");
    }

    /// B′: only lamp surface — no dark background rectangle.
    fn show_meter_surface(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            let _ = SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 0, LWA_ALPHA_ONLY);
            // After SetLayeredWindowAttributes, ULW fails until WS_EX_LAYERED is toggled.
            reset_layered_exstyle(self.text_hwnd);
            let _ = ShowWindow(self.text_hwnd, SW_SHOWNOACTIVATE);
        }
        info!(rect = ?self.rect_array(), "HUD silent meter surface shown");
    }

    /// Leave ULW mode so colorkey + GDI text path works again.
    fn restore_text_colorkey_layer(&self) {
        unsafe {
            reset_layered_exstyle(self.text_hwnd);
            let _ = SetLayeredWindowAttributes(
                self.text_hwnd,
                HUD_TEXT_COLORKEY,
                255,
                LWA_COLORKEY_ALPHA,
            );
        }
    }

    fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.text_hwnd, SW_HIDE);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        info!("HUD window hidden");
    }

    fn set_alpha(&self, alpha: u8) {
        let text_alpha = ((self.style.text_alpha as f32)
            * (alpha as f32 / self.style.background_alpha.max(1) as f32))
            .round()
            .clamp(0.0, 255.0) as u8;
        unsafe {
            let _ = SetLayeredWindowAttributes(self.hwnd, COLORREF(0), alpha, LWA_ALPHA_ONLY);
            let _ = SetLayeredWindowAttributes(
                self.text_hwnd,
                HUD_TEXT_COLORKEY,
                text_alpha,
                LWA_COLORKEY_ALPHA,
            );
        }
    }

    /// S5 lamp: per-pixel ARGB via UpdateLayeredWindow (true soft transparent edges).
    fn present_s5_glow(
        &self,
        level_milli: u32,
        t: f32,
        kind: HudActivityKind,
        visibility: f32,
    ) {
        let w = METER_WIDTH_PX;
        let h = METER_HEIGHT_PX;
        if w <= 0 || h <= 0 {
            return;
        }
        let pixels = render_s5_glow_premul_bgra(w as usize, h as usize, level_milli, t, kind, visibility);
        unsafe {
            let screen_dc = GetDC(None);
            if screen_dc.0.is_null() {
                return;
            }
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            if mem_dc.0.is_null() {
                let _ = ReleaseDC(None, screen_dc);
                return;
            }
            let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let hbmp = match CreateDIBSection(
                Some(mem_dc),
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            ) {
                Ok(bmp) => bmp,
                Err(_) => {
                    let _ = DeleteDC(mem_dc);
                    let _ = ReleaseDC(None, screen_dc);
                    return;
                }
            };
            if bits.is_null() {
                let _ = DeleteObject(hbmp.into());
                let _ = DeleteDC(mem_dc);
                let _ = ReleaseDC(None, screen_dc);
                return;
            }
            let dst = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
            dst.copy_from_slice(&pixels);
            let old = SelectObject(mem_dc, HGDIOBJ(hbmp.0));
            let mut win_rect = RECT::default();
            let _ = GetWindowRect(self.text_hwnd, &mut win_rect);
            let pos = POINT {
                x: win_rect.left,
                y: win_rect.top,
            };
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // Bg fully invisible; lamp uses ULW alpha on text surface.
            let _ = SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 0, LWA_ALPHA_ONLY);
            let _ = UpdateLayeredWindow(
                self.text_hwnd,
                Some(screen_dc),
                Some(&pos),
                Some(&size),
                Some(mem_dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            let _ = SelectObject(mem_dc, old);
            let _ = DeleteObject(hbmp.into());
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(None, screen_dc);
        }
    }

    fn set_status_text(&self, text: &str, _stable: bool) {
        self.resize_to_fit(text);
        self.set_status_text_raw(text);
        info!(
            text_chars = text.chars().count(),
            visible = self.is_visible(),
            rect = ?self.rect_array(),
            text = %short_hud_text(text, 180),
            "HUD SetWindowText applied"
        );
    }

    fn set_status_text_raw(&self, text: &str) {
        let text = HSTRING::from(text);
        unsafe {
            let _ = SetWindowTextW(self.hwnd, w!(""));
            let _ = SetWindowTextW(self.text_hwnd, &text);
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            let _ = InvalidateRect(Some(self.text_hwnd), None, false);
        }
    }

    fn resize_to_meter(&self) {
        // Always park above the taskbar (work area), not inside the taskbar strip.
        let layout_area = work_area_rect();
        let available_width = (layout_area.right - layout_area.left - HUD_SCREEN_MARGIN_PX * 2).max(48);
        let available_height =
            (layout_area.bottom - layout_area.top - HUD_SCREEN_MARGIN_PX * 2).max(48);
        let hud_width = METER_WIDTH_PX.clamp(48, available_width);
        let hud_height = METER_HEIGHT_PX.clamp(48, available_height);
        self.apply_meter_size(layout_area, hud_width, hud_height, 0);
    }

    fn reposition_meter(&self, bounce_y_px: i32) {
        let layout_area = work_area_rect();
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.text_hwnd, &mut rect);
        }
        let hud_width = (rect.right - rect.left).max(METER_WIDTH_PX / 2);
        let hud_height = (rect.bottom - rect.top).max(METER_HEIGHT_PX / 2);
        self.apply_meter_size(layout_area, hud_width, hud_height, bounce_y_px);
    }

    /// Bottom-center of the work area (above taskbar) + bounce offset.
    fn apply_meter_size(
        &self,
        layout_area: RECT,
        hud_width: i32,
        hud_height: i32,
        bounce_y_px: i32,
    ) {
        let bounds = desktop_rect();
        let base_x = layout_area.left + ((layout_area.right - layout_area.left - hud_width) / 2);
        let base_y = layout_area.bottom - hud_height - METER_BOTTOM_MARGIN_PX;
        let hud_x = clamp_i32(
            base_x + self.style.offset_x_px,
            bounds.left,
            (bounds.right - hud_width).max(bounds.left),
        );
        // Positive bounce_y = enter from below; still stays above taskbar when settled.
        let hud_y = clamp_i32(
            base_y + self.style.offset_y_px + bounce_y_px,
            bounds.top,
            (bounds.bottom - hud_height).max(bounds.top),
        );
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                hud_x,
                hud_y,
                hud_width,
                hud_height,
                SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0),
            );
            // Must CLEAR any prior rounded region — leftover rgn clips half the orb.
            let _ = clear_window_region(self.hwnd);
            let _ = SetWindowPos(
                self.text_hwnd,
                Some(HWND_TOPMOST),
                hud_x,
                hud_y,
                hud_width,
                hud_height,
                SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0),
            );
            let _ = clear_window_region(self.text_hwnd);
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            let _ = InvalidateRect(Some(self.text_hwnd), None, false);
        }
    }

    fn resize_to_fit(&self, text: &str) {
        let layout_area = hud_layout_rect(self.style.anchor);
        let available_width = (layout_area.right - layout_area.left - HUD_SCREEN_MARGIN_PX * 2)
            .max(self.style.min_width_px);
        let available_height = (layout_area.bottom - layout_area.top - HUD_SCREEN_MARGIN_PX * 2)
            .max(self.style.min_height_px);
        let min_width = self.style.min_width_px.min(available_width).max(1);
        let max_hud_width = self.style.width_px.max(min_width).min(available_width);
        let max_text_width = (max_hud_width - self.style.padding_x_px * 2).max(1);
        let (text_width, _text_height) =
            measure_hud_text(self.text_hwnd, self.font, text, max_text_width, &self.style);

        let (hud_width, hud_height) = match self.style.visual_style {
            HudVisualStyle::AiConsole => {
                let width = max_hud_width;
                let height = self.style.height_px.clamp(24, available_height);
                (width, height)
            }
            HudVisualStyle::Minimal | HudVisualStyle::FloatingText => {
                let width =
                    (text_width + self.style.padding_x_px * 2).clamp(min_width, available_width);
                let height = self.style.height_px.clamp(24, available_height);
                (width, height)
            }
        };
        self.apply_size(layout_area, hud_width, hud_height);
    }

    fn apply_size(&self, layout_area: RECT, hud_width: i32, hud_height: i32) {
        self.apply_size_with_bounce(layout_area, hud_width, hud_height, 0);
    }

    fn apply_size_with_bounce(
        &self,
        layout_area: RECT,
        hud_width: i32,
        hud_height: i32,
        bounce_y_px: i32,
    ) {
        let bounds = desktop_rect();
        let area_width = layout_area.right - layout_area.left;
        let area_height = layout_area.bottom - layout_area.top;
        let base_x = match self.style.anchor {
            HudAnchor::BottomLeft => layout_area.left + HUD_SCREEN_MARGIN_PX,
            HudAnchor::BottomCenter => {
                layout_area.left + ((layout_area.right - layout_area.left - hud_width) / 2)
            }
            HudAnchor::TaskbarLeft => layout_area.left + HUD_SCREEN_MARGIN_PX,
            HudAnchor::TaskbarCenter => match self.style.expand_origin {
                HudExpandOrigin::Center => layout_area.left + ((area_width - hud_width) / 2),
                HudExpandOrigin::Left => layout_area.left + (area_width / 2),
            },
            HudAnchor::TaskbarRight => layout_area.right - hud_width - HUD_SCREEN_MARGIN_PX,
        };
        let base_y = if is_taskbar_anchor(self.style.anchor) {
            layout_area.top + ((area_height - hud_height) / 2)
        } else {
            layout_area.bottom - hud_height - HUD_SCREEN_MARGIN_PX
        };
        let hud_x = clamp_i32(
            base_x + self.style.offset_x_px,
            bounds.left,
            (bounds.right - hud_width).max(bounds.left),
        );
        // Positive bounce_y pushes window downward (enter from below).
        let hud_y = clamp_i32(
            base_y + self.style.offset_y_px + bounce_y_px,
            bounds.top,
            (bounds.bottom - hud_height).max(bounds.top),
        );

        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                hud_x,
                hud_y,
                hud_width,
                hud_height,
                SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0),
            );
            let _ = apply_rounded_region(
                self.hwnd,
                hud_width,
                hud_height,
                self.style.corner_radius_px,
            );
            let _ = SetWindowPos(
                self.text_hwnd,
                Some(HWND_TOPMOST),
                hud_x,
                hud_y,
                hud_width,
                hud_height,
                SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0),
            );
            let _ = apply_rounded_region(
                self.text_hwnd,
                hud_width,
                hud_height,
                self.style.corner_radius_px,
            );
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            let _ = InvalidateRect(Some(self.text_hwnd), None, false);
        }
    }

    fn apply_appearance(
        &mut self,
        appearance: &HudFontAppearance,
        current_text: &str,
    ) -> Result<()> {
        let mut next_style = self.style.clone();
        next_style.font_family = if appearance.family.trim().is_empty() {
            HudFontAppearance::default().family
        } else {
            appearance.family.trim().to_string()
        };
        next_style.font_height_px = normalize_font_height_px(appearance.height_px);
        next_style.font_weight = appearance.weight.clamp(100, 900);
        next_style.background_alpha = alpha_percent_to_byte(
            appearance
                .background_alpha_percent
                .clamp(HUD_BACKGROUND_ALPHA_MIN_PERCENT, 100),
        );
        let font = unsafe { create_hud_font(&next_style) };
        if font.is_invalid() {
            return Err(anyhow!("create HUD font failed"));
        }
        self.style = next_style;
        unsafe {
            let old_font = std::mem::replace(&mut self.font, font);
            HUD_FONT_HANDLE.store(self.font.0 as isize, Ordering::Relaxed);
            let _ = DeleteObject(old_font.into());
        }
        store_hud_paint_style(&self.style);
        self.resize_to_fit(current_text);
        info!(
            font_family = %self.style.font_family,
            font_height_px = self.style.font_height_px,
            font_weight = self.style.font_weight,
            background_alpha_percent = alpha_byte_to_percent(self.style.background_alpha),
            "HUD appearance updated"
        );
        Ok(())
    }

    fn apply_user_config(&mut self, user: &HudUserConfig, current_text: &str) -> Result<()> {
        let mut config = HudConfig::default();
        config.apply_user_config(user);
        let next_style = HudStyle::from_config(&config);
        let font = unsafe { create_hud_font(&next_style) };
        if font.is_invalid() {
            return Err(anyhow!("create HUD font failed"));
        }
        self.style = next_style;
        unsafe {
            let old_font = std::mem::replace(&mut self.font, font);
            let new_brush = CreateSolidBrush(self.style.background_color);
            if !new_brush.is_invalid() {
                let old_brush = std::mem::replace(&mut self.brush, new_brush);
                HUD_BACKGROUND_BRUSH.store(self.brush.0 as isize, Ordering::Relaxed);
                let _ = DeleteObject(old_brush.into());
            }
            HUD_FONT_HANDLE.store(self.font.0 as isize, Ordering::Relaxed);
            HUD_PADDING_X.store(self.style.padding_x_px, Ordering::Relaxed);
            HUD_PADDING_Y.store(self.style.padding_y_px, Ordering::Relaxed);
            HUD_CENTER_TEXT.store(
                self.style.text_align == HudTextAlign::Center,
                Ordering::Relaxed,
            );
            let _ = DeleteObject(old_font.into());
        }
        store_hud_paint_style(&self.style);
        self.resize_to_fit(current_text);
        info!(
            anchor = ?self.style.anchor,
            width_px = self.style.width_px,
            height_px = self.style.height_px,
            font_height_px = self.style.font_height_px,
            "HUD layout updated"
        );
        Ok(())
    }

    fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    fn rect_array(&self) -> [i32; 4] {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
        }
        [rect.left, rect.top, rect.right, rect.bottom]
    }
}

impl Drop for HudWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.text_hwnd);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.font.into());
            let _ = DeleteObject(self.brush.into());
        }
    }
}

impl HudMicrostreamState {
    fn clear(&mut self) {
        self.committed_prefix.clear();
        self.target_suffix.clear();
        self.display_suffix.clear();
    }

    fn set_immediate(&mut self, message: &str) {
        self.committed_prefix = message.to_string();
        self.target_suffix.clear();
        self.display_suffix.clear();
    }

    fn display_message(&self) -> String {
        format!("{}{}", self.committed_prefix, self.display_suffix)
    }

    fn target_message(&self) -> String {
        format!("{}{}", self.committed_prefix, self.target_suffix)
    }

    fn has_pending_chars(&self) -> bool {
        self.display_suffix != self.target_suffix
    }

    fn pending_char_count(&self) -> usize {
        self.target_suffix
            .chars()
            .count()
            .saturating_sub(self.display_suffix.chars().count())
    }

    fn retarget(&mut self, message: &str) {
        let (candidate_committed_prefix, candidate_suffix) = split_committed_prefix(message);
        let previous_display = self.display_message();
        let preserved_message_chars = if message.starts_with(&previous_display) {
            previous_display.chars().count()
        } else {
            longest_common_prefix_chars(&previous_display, message)
        };

        self.committed_prefix = candidate_committed_prefix;
        let next_target_suffix = message
            .strip_prefix(&self.committed_prefix)
            .map(str::to_string)
            .unwrap_or(candidate_suffix);
        let preserved_display = take_prefix_chars(message, preserved_message_chars);
        let next_display_suffix = preserved_display
            .strip_prefix(&self.committed_prefix)
            .map(str::to_string)
            .unwrap_or_default();
        self.target_suffix = next_target_suffix;
        self.display_suffix = next_display_suffix;
    }

    fn advance_one_char(&mut self) {
        self.advance_chars(1);
    }

    fn advance_chars(&mut self, char_count: usize) {
        if self.display_suffix == self.target_suffix {
            return;
        }
        let next_char_count = (self.display_suffix.chars().count() + char_count.max(1))
            .min(self.target_suffix.chars().count());
        self.display_suffix = take_prefix_chars(&self.target_suffix, next_char_count);
    }
}

fn pump_messages() -> Result<bool> {
    unsafe {
        let mut msg = MSG::default();
        if !PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            return Ok(false);
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
        Ok(true)
    }
}

/// Toggle WS_EX_LAYERED so SetLayeredWindowAttributes ↔ UpdateLayeredWindow can switch.
unsafe fn reset_layered_exstyle(hwnd: HWND) {
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let layered = WS_EX_LAYERED.0 as isize;
    let base = ex & !layered;
    let _ = unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, base) };
    let _ = unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, base | layered) };
}

unsafe fn clear_window_region(hwnd: HWND) -> Result<()> {
    // SetWindowRgn(None) removes clip so the full soft disc is visible.
    if unsafe { SetWindowRgn(hwnd, None, true) } != 1 {
        // Some hosts return 0 when there was already no region — non-fatal.
    }
    Ok(())
}

unsafe fn apply_rounded_region(hwnd: HWND, width: i32, height: i32, radius: i32) -> Result<()> {
    if radius <= 0 {
        return unsafe { clear_window_region(hwnd) };
    }
    let region = unsafe { CreateRoundRectRgn(0, 0, width, height, radius, radius) };
    if region.is_invalid() {
        return Err(anyhow!("create rounded HUD region failed"));
    }
    if unsafe { SetWindowRgn(hwnd, Some(region), true) } != 1 {
        let _ = unsafe { DeleteObject(region.into()) };
        return Err(anyhow!("apply HUD region failed"));
    }
    Ok(())
}

fn work_area_rect() -> RECT {
    unsafe {
        let mut work_area = RECT::default();
        if SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut work_area as *mut RECT).cast()),
            Default::default(),
        )
        .is_ok()
        {
            return work_area;
        }
        RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN).max(0),
            bottom: GetSystemMetrics(SM_CYSCREEN).max(0),
        }
    }
}

fn desktop_rect() -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: unsafe { GetSystemMetrics(SM_CXSCREEN).max(0) },
        bottom: unsafe { GetSystemMetrics(SM_CYSCREEN).max(0) },
    }
}

fn hud_layout_rect(anchor: HudAnchor) -> RECT {
    if is_taskbar_anchor(anchor) {
        if let Some(rect) = taskbar_rect() {
            return rect;
        }
    }
    work_area_rect()
}

fn is_taskbar_anchor(anchor: HudAnchor) -> bool {
    matches!(
        anchor,
        HudAnchor::TaskbarLeft | HudAnchor::TaskbarCenter | HudAnchor::TaskbarRight
    )
}

fn taskbar_rect() -> Option<RECT> {
    unsafe {
        let hwnd = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok()?;
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok()
            && rect.right > rect.left
            && rect.bottom > rect.top
        {
            return Some(rect);
        }
    }
    None
}

fn measure_hud_text(
    text_hwnd: HWND,
    font: HFONT,
    text: &str,
    max_text_width: i32,
    style: &HudStyle,
) -> (i32, i32) {
    if text.trim().is_empty() {
        return (style.min_text_width_px, style.font_height_px);
    }
    unsafe {
        let hdc = GetDC(Some(text_hwnd));
        if hdc.0.is_null() {
            return (style.min_text_width_px, style.font_height_px);
        }
        let old_font = SelectObject(hdc, font.into());
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut utf16 = text.encode_utf16().collect::<Vec<_>>();
        let align = match style.text_align {
            HudTextAlign::Left => DT_LEFT,
            HudTextAlign::Center => DT_CENTER,
        };
        let _ = DrawTextW(
            hdc,
            utf16.as_mut_slice(),
            &mut rect,
            DT_CALCRECT | align | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old_font);
        let _ = ReleaseDC(Some(text_hwnd), hdc);
        (
            (rect.right - rect.left).clamp(
                style.min_text_width_px,
                max_text_width.max(style.min_text_width_px),
            ),
            (rect.bottom - rect.top).max(style.font_height_px),
        )
    }
}

unsafe extern "system" fn hud_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as _);
            let _ = unsafe { SetBkMode(hdc, TRANSPARENT) };
            let _ = unsafe { SetTextColor(hdc, COLORREF(HUD_TEXT_COLOR.load(Ordering::Relaxed))) };
            LRESULT(HUD_BACKGROUND_BRUSH.load(Ordering::Relaxed))
        }
        WM_PAINT => {
            paint_hud_window(hwnd);
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(-1),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn paint_hud_window(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.0.is_null() {
            return;
        }

        let mut rect = RECT::default();
        if windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect).is_err() {
            let _ = EndPaint(hwnd, &ps);
            return;
        }

        let text = hud_window_text(hwnd);
        let silent = HUD_SILENT_METER.load(Ordering::Relaxed);
        let kind = current_hud_activity_kind();

        // S5 uses UpdateLayeredWindow (per-pixel alpha). Do not GDI-paint — it would
        // replace the soft lamp with colorkey/black rings.
        if silent {
            let _ = EndPaint(hwnd, &ps);
            return;
        }

        if text.trim().is_empty() {
            // Background surface: solid fill (hidden entirely in silent mode).
            let brush = HBRUSH(HUD_BACKGROUND_BRUSH.load(Ordering::Relaxed) as _);
            if !brush.is_invalid() {
                let _ = FillRect(hdc, &rect, brush);
            }
            let _ = EndPaint(hwnd, &ps);
            return;
        }

        let key_brush = CreateSolidBrush(HUD_TEXT_COLORKEY);
        if !key_brush.is_invalid() {
            let _ = FillRect(hdc, &rect, key_brush);
            let _ = DeleteObject(key_brush.into());
        }
        if !text.trim().is_empty() {
            let theme = current_hud_animation_theme();
            if theme != HudAnimationTheme::TextOnly && kind.is_animated() {
                let paint_style = current_hud_paint_style();
                draw_hud_animation(hdc, rect, kind, theme, &paint_style);
                let _ = EndPaint(hwnd, &ps);
                return;
            }
            let font = HFONT(HUD_FONT_HANDLE.load(Ordering::Relaxed) as _);
            let old_font = if font.is_invalid() {
                HGDIOBJ::default()
            } else {
                SelectObject(hdc, HGDIOBJ(font.0))
            };
            let _ = SetBkMode(hdc, TRANSPARENT);
            let paint_style = current_hud_paint_style();
            // GDI anti-aliasing blends edge pixels against the current bk color even when
            // text background is transparent. Use the real HUD fill instead of the magenta
            // colorkey surface so transparent text does not get a red fringe.
            let _ = SetBkColor(hdc, paint_style.background_color);
            let _ = SetTextColor(hdc, paint_style.text_color);

            let padding_x = HUD_PADDING_X.load(Ordering::Relaxed).max(0);
            let padding_y = HUD_PADDING_Y.load(Ordering::Relaxed).max(0);
            rect.left += padding_x;
            rect.right -= padding_x;
            rect.top += padding_y;
            rect.bottom -= padding_y;
            if rect.right > rect.left && rect.bottom > rect.top {
                if let Some((status, body)) = split_hud_status_text(&text) {
                    let status_width = measure_hud_text_width(hdc, status);
                    let available_width = (rect.right - rect.left).max(1);
                    let max_status_width = (available_width / 2).max(1);
                    let status_rect_width = (status_width + 16)
                        .max(72)
                        .min(max_status_width)
                        .min(available_width);
                    let mut status_rect = rect;
                    status_rect.right = (status_rect.left + status_rect_width).min(rect.right);
                    draw_hud_single_line(hdc, status, status_rect, false, false, &paint_style);

                    let mut body_rect = rect;
                    body_rect.left = (status_rect.right + 12).min(rect.right);
                    draw_hud_single_line(
                        hdc,
                        body,
                        body_rect,
                        HUD_CENTER_TEXT.load(Ordering::Relaxed),
                        true,
                        &paint_style,
                    );
                } else {
                    draw_hud_single_line(
                        hdc,
                        &text,
                        rect,
                        HUD_CENTER_TEXT.load(Ordering::Relaxed),
                        true,
                        &paint_style,
                    );
                }
            }

            if !old_font.is_invalid() {
                let _ = SelectObject(hdc, old_font);
            }
        }

        let _ = EndPaint(hwnd, &ps);
    }
}

fn hud_window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let read = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..read as usize])
    }
}

fn split_hud_status_text(text: &str) -> Option<(&str, &str)> {
    let (status, body) = text.split_once(" | ")?;
    if status.starts_with("改写")
        || status.starts_with("翻译")
        || status == "未调用AI"
        || status == "未启用AI"
    {
        return Some((status, body));
    }
    None
}

fn draw_hud_single_line(
    hdc: HDC,
    text: &str,
    rect: RECT,
    center_if_fits: bool,
    latest_on_overflow: bool,
    style: &HudPaintStyle,
) {
    if rect.right <= rect.left || rect.bottom <= rect.top || text.trim().is_empty() {
        return;
    }
    let text_width = measure_hud_text_width(hdc, text);
    let available_width = (rect.right - rect.left).max(1);
    let align = if text_width > available_width {
        DT_LEFT
    } else if center_if_fits {
        DT_CENTER
    } else {
        DT_LEFT
    };
    if style.shadow_enabled {
        let mut shadow_rect = rect;
        shadow_rect.left += style.shadow_offset_x_px;
        shadow_rect.right += style.shadow_offset_x_px;
        shadow_rect.top += style.shadow_offset_y_px;
        shadow_rect.bottom += style.shadow_offset_y_px;
        let _ =
            unsafe { SetTextColor(hdc, blend_with_key(style.shadow_color, style.shadow_alpha)) };
        draw_hud_text_effect(
            hdc,
            text,
            shadow_rect,
            align,
            latest_on_overflow,
            style,
            true,
        );
    }
    let _ = unsafe { SetTextColor(hdc, style.text_color) };
    draw_hud_text_effect(hdc, text, rect, align, latest_on_overflow, style, false);
}

fn draw_hud_text_effect(
    hdc: HDC,
    text: &str,
    rect: RECT,
    align: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
    latest_on_overflow: bool,
    style: &HudPaintStyle,
    shadow: bool,
) {
    if style.text_effect == HudTextEffect::Rainbow && !shadow {
        draw_rainbow_hud_line(hdc, text, rect, align, latest_on_overflow, style);
        return;
    }
    let text_width = measure_hud_text_width(hdc, text);
    let draw_rect = if latest_on_overflow {
        latest_text_draw_rect(rect, text_width)
    } else {
        rect
    };
    let mut text_wide = text.encode_utf16().collect::<Vec<_>>();
    unsafe {
        let saved_dc = SaveDC(hdc);
        let _ = IntersectClipRect(hdc, rect.left, rect.top, rect.right, rect.bottom);
        let _ = DrawTextW(
            hdc,
            text_wide.as_mut_slice(),
            &mut draw_rect.clone(),
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | align,
        );
        if saved_dc != 0 {
            let _ = RestoreDC(hdc, saved_dc);
        }
    }
}

fn draw_hud_animation(
    hdc: HDC,
    mut rect: RECT,
    kind: HudActivityKind,
    theme: HudAnimationTheme,
    style: &HudPaintStyle,
) {
    let padding_x = HUD_PADDING_X.load(Ordering::Relaxed).max(0);
    let padding_y = HUD_PADDING_Y.load(Ordering::Relaxed).max(0);
    rect.left += padding_x;
    rect.right -= padding_x;
    rect.top += padding_y;
    rect.bottom -= padding_y;
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    let frame = HUD_ANIMATION_FRAME.load(Ordering::Relaxed);
    let accent = activity_accent_color(kind, frame);
    let muted = blend_color(style.background_color, accent, 0.18);
    let quiet = blend_color(style.background_color, accent, 0.08);
    // Design B: Listening = full-width live mic bars only (no left status chip).
    if kind == HudActivityKind::Listening && theme != HudAnimationTheme::TextOnly {
        if theme == HudAnimationTheme::MinimalPulse {
            draw_minimal_pulse(hdc, rect, kind, frame, accent, muted, style);
            return;
        }
        let envelope = HUD_ENVELOPE_MILLI.load(Ordering::Relaxed);
        draw_live_voice_bars(hdc, rect, envelope, frame, accent, muted);
        return;
    }

    // Never draw left status chip ("听写/识别/改写") — user wants pure meter/dots only.
    match theme {
        HudAnimationTheme::TextOnly => {
            draw_hud_single_line(hdc, kind.short_label(), rect, true, false, style)
        }
        HudAnimationTheme::VoiceBars
        | HudAnimationTheme::Waveform
        | HudAnimationTheme::StageDots => {
            draw_stage_dots(hdc, rect, kind, frame, accent, muted);
        }
        HudAnimationTheme::AiGlow => {
            draw_ai_glow(hdc, rect, kind, frame, accent, quiet, style);
        }
        HudAnimationTheme::MinimalPulse => {
            draw_minimal_pulse(hdc, rect, kind, frame, accent, muted, style)
        }
        HudAnimationTheme::FullAnimated => match kind {
            HudActivityKind::Listening => {
                let envelope = HUD_ENVELOPE_MILLI.load(Ordering::Relaxed);
                draw_live_voice_bars(hdc, rect, envelope, frame, accent, muted);
            }
            HudActivityKind::Recognizing => {
                draw_stage_dots(hdc, rect, kind, frame, accent, muted)
            }
            HudActivityKind::Rewriting
            | HudActivityKind::Translating
            | HudActivityKind::RawPastedRewriting => {
                draw_ai_glow(hdc, rect, kind, frame, accent, quiet, style);
            }
            HudActivityKind::Done | HudActivityKind::Warning | HudActivityKind::Text => {}
        },
    }
}

fn ensure_anim_clock() {
    let _ = HUD_PROCESS_START.get_or_init(Instant::now);
}

fn process_ms() -> u32 {
    let start = HUD_PROCESS_START.get_or_init(Instant::now);
    start.elapsed().as_millis().min(u32::MAX as u128) as u32
}

fn anim_time_sec() -> f32 {
    ensure_anim_clock();
    let t0 = HUD_ANIM_T0_MS.load(Ordering::Relaxed);
    let now = process_ms();
    let t0 = if t0 == 0 {
        HUD_ANIM_T0_MS.store(now, Ordering::Relaxed);
        now
    } else {
        t0
    };
    now.saturating_sub(t0) as f32 / 1000.0
}

/// Spring bounce from below: starts at +amp, settles to 0 with cute overshoot.
fn bounce_y_offset(enter_at: Option<Instant>, now: Instant) -> i32 {
    let Some(start) = enter_at else {
        return 0;
    };
    let t = now.duration_since(start).as_secs_f32();
    if t > 0.85 {
        return 0;
    }
    // Underdamped: e^{-ζωt} cos(ωd t)
    let omega = 13.5_f32;
    let zeta = 0.52_f32;
    let wd = omega * (1.0 - zeta * zeta).sqrt();
    let envelope = (-zeta * omega * t).exp();
    let y = METER_BOUNCE_AMP_PX * envelope * (wd * t).cos();
    y.round() as i32
}

/// Combined enter/exit Y: enter pops up from below; exit pops back down.
fn meter_bounce_y(enter_at: Option<Instant>, exit_at: Option<Instant>, now: Instant) -> i32 {
    if let Some(start) = exit_at {
        let t = now.duration_since(start).as_secs_f32();
        let p = (t / METER_EXIT_DUR_SEC).clamp(0.0, 1.0);
        // Ease-in downward (accelerate toward taskbar) + soft overshoot like reverse spring.
        let ease = p * p * (1.0 + 0.22 * (1.0 - p));
        let spring_tail = if p < 0.55 {
            let u = p / 0.55;
            (u * std::f32::consts::PI).sin() * 0.10 * (1.0 - u)
        } else {
            0.0
        };
        return (METER_BOUNCE_AMP_PX * 1.35 * (ease + spring_tail)).round() as i32;
    }
    bounce_y_offset(enter_at, now)
}

fn update_listening_envelope_dt(dt: f32) {
    let raw = HUD_AUDIO_LEVEL
        .get()
        .map(|level| level.load(Ordering::Relaxed))
        .unwrap_or(0);
    let t = anim_time_sec();
    // Silence floor: soft breath so bars/particles do not freeze dead-flat when quiet.
    let effective = if raw < 35 {
        let breath = (t * 2.2).sin() * 0.5 + 0.5;
        12 + (breath * 22.0).round() as u32
    } else {
        raw
    };
    let env = HUD_ENVELOPE_MILLI.load(Ordering::Relaxed) as f32;
    // Fast attack / slow release — time-based for 144 Hz smoothness.
    let attack = 1.0 - (-18.0 * dt).exp();
    let release = 1.0 - (-5.5 * dt).exp();
    let target = effective as f32;
    let next = if target >= env {
        env + (target - env) * attack
    } else {
        env + (target - env) * release
    };
    HUD_ENVELOPE_MILLI.store(next.round().clamp(0.0, 1000.0) as u32, Ordering::Relaxed);
}

fn settle_busy_envelope_dt(dt: f32) {
    let t = anim_time_sec();
    let breath = (t * 1.6).sin() * 0.5 + 0.5;
    let target = 90.0 + breath * 70.0;
    let env = HUD_ENVELOPE_MILLI.load(Ordering::Relaxed) as f32;
    let k = 1.0 - (-4.0 * dt).exp();
    let next = env + (target - env) * k;
    HUD_ENVELOPE_MILLI.store(next.round().clamp(0.0, 1000.0) as u32, Ordering::Relaxed);
}

/// S5 lamp buffer: premultiplied BGRA, edge alpha → 0 (true transparent, not black).
fn render_s5_glow_premul_bgra(
    width: usize,
    height: usize,
    level_milli: u32,
    t: f32,
    kind: HudActivityKind,
    visibility: f32,
) -> Vec<u32> {
    let mut out = vec![0u32; width * height];
    if width == 0 || height == 0 {
        return out;
    }
    let level = (level_milli as f32 / 1000.0).clamp(0.0, 1.0);
    let busy = kind != HudActivityKind::Listening;
    let breath = (t * 2.05).sin() * 0.5 + 0.5;
    let energy = if busy {
        (0.34 + breath * 0.10 + level * 0.32).clamp(0.28, 0.82)
    } else {
        (0.26 + breath * 0.08 + level * 0.82).clamp(0.22, 1.0)
    };
    let visibility = visibility.clamp(0.0, 1.0);
    let cx = (width as f32 - 1.0) * 0.5;
    let cy = (height as f32 - 1.0) * 0.5;
    // Radius leaves a transparent margin so the disc is never clipped mid-glow.
    let max_r = (width.min(height) as f32) * 0.40 * (0.78 + energy * 0.28);
    let accent = activity_accent_color(kind, HUD_ANIMATION_FRAME.load(Ordering::Relaxed));
    let (ar, ag, ab) = colorref_to_rgb(accent);
    // Brighter cool-white core tinted by activity accent.
    let core_r = (ar as f32 * 0.28 + 255.0 * 0.72).clamp(0.0, 255.0);
    let core_g = (ag as f32 * 0.28 + 250.0 * 0.72).clamp(0.0, 255.0);
    let core_b = (ab as f32 * 0.18 + 255.0 * 0.82).clamp(0.0, 255.0);

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt() / max_r.max(1.0);
            if dist >= 1.0 {
                continue; // fully transparent — not black
            }
            // Smooth falloff: denser core, pure transparent rim (灯晕).
            let u = 1.0 - dist;
            let falloff = (u * u * (3.0 - 2.0 * u)).powf(0.95);
            // Hotspot only in the very center (user: 中心再亮一点).
            let hotspot = falloff.powf(2.8);
            let a_f = ((falloff * 0.82 + hotspot * 0.55) * energy * visibility).clamp(0.0, 1.0);
            if a_f < 0.004 {
                continue;
            }
            // Core hotter + white fleck; outer cooler (still no dark ring).
            let heat = falloff.powf(0.48);
            let r = (core_r * (0.48 + 0.52 * heat) + 55.0 * hotspot).clamp(0.0, 255.0);
            let g = (core_g * (0.48 + 0.52 * heat) + 50.0 * hotspot).clamp(0.0, 255.0);
            let b = (core_b * (0.52 + 0.48 * heat) + 40.0 * hotspot).clamp(0.0, 255.0);
            let a = (a_f * 255.0).round().clamp(0.0, 255.0);
            // Premultiplied BGRA for UpdateLayeredWindow + AC_SRC_ALPHA.
            let pr = (r * a_f).round().clamp(0.0, 255.0) as u32;
            let pg = (g * a_f).round().clamp(0.0, 255.0) as u32;
            let pb = (b * a_f).round().clamp(0.0, 255.0) as u32;
            let pa = a as u32;
            out[y * width + x] = pb | (pg << 8) | (pr << 16) | (pa << 24);
        }
    }
    out
}

/// iOS-style live voice bars driven by mic envelope (0..=1000).
fn draw_live_voice_bars(
    hdc: HDC,
    rect: RECT,
    level_milli: u32,
    frame: u32,
    accent: COLORREF,
    muted: COLORREF,
) {
    let bar_count = 27;
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let gap = 2;
    // Wider bars, nearly full HUD height — was too timid (tiny center strip).
    let bar_width = ((width - gap * (bar_count - 1)) / bar_count).clamp(3, 7);
    let total_width = bar_width * bar_count + gap * (bar_count - 1);
    let mut x = rect.left + (width - total_width) / 2;
    let usable_height = (height - 2).max(10);
    // Extra display gain so quiet speech still looks lively
    let level = ((level_milli as f32 / 1000.0) * 1.35).clamp(0.0, 1.0);
    let center_y = rect.top + height / 2;
    let center_i = (bar_count - 1) as f32 / 2.0;
    for index in 0..bar_count {
        let dist = ((index as f32 - center_i) / center_i).abs();
        let shape = (1.0 - dist * 0.42).clamp(0.40, 1.0);
        let jitter =
            ((frame as f32 * 0.18 + index as f32 * 0.75).sin() * 0.5 + 0.5) * 0.14 * level.max(0.2);
        let amp = (level * shape * (0.92 + jitter)).clamp(0.0, 1.0);
        let min_h = 4;
        let bar_height =
            (min_h as f32 + (usable_height - min_h) as f32 * amp).round() as i32;
        let bar_height = bar_height.clamp(min_h, usable_height);
        let color = if amp > 0.50 {
            accent
        } else if amp > 0.18 {
            blend_color(muted, accent, 0.62)
        } else {
            muted
        };
        fill_rect_color(
            hdc,
            RECT {
                left: x,
                top: center_y - bar_height / 2,
                right: x + bar_width,
                bottom: center_y + bar_height / 2,
            },
            color,
        );
        x += bar_width + gap;
    }
}

#[allow(dead_code)]
fn draw_voice_bars(hdc: HDC, rect: RECT, frame: u32, accent: COLORREF, muted: COLORREF) {
    // Legacy sin decoration kept only as compile fallback; live path uses draw_live_voice_bars.
    draw_live_voice_bars(hdc, rect, 420, frame, accent, muted);
}

#[allow(dead_code)]
fn draw_waveform(hdc: HDC, rect: RECT, frame: u32, accent: COLORREF, muted: COLORREF) {
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let center_y = rect.top + height / 2;
    let mut base = rect;
    base.top = center_y - 1;
    base.bottom = center_y + 1;
    fill_rect_color(hdc, base, muted);
    draw_wave_line(hdc, rect, center_y, frame, muted, 0.20, 0.9, 1);
    draw_wave_line(hdc, rect, center_y, frame + 9, accent, 0.30, 1.25, 1);
    let head_x = rect.left + ((frame as i32 * 4).rem_euclid(width));
    fill_rect_color(
        hdc,
        RECT {
            left: head_x - 1,
            top: center_y - height / 4,
            right: head_x + 1,
            bottom: center_y + height / 4,
        },
        accent,
    );
}

#[allow(dead_code)]
fn draw_wave_line(
    hdc: HDC,
    rect: RECT,
    center_y: i32,
    frame: u32,
    color: COLORREF,
    amp_factor: f32,
    speed: f32,
    pen_width: i32,
) {
    unsafe {
        let pen = windows::Win32::Graphics::Gdi::CreatePen(PS_SOLID, pen_width.max(1), color);
        if pen.is_invalid() {
            return;
        }
        let old = SelectObject(hdc, HGDIOBJ(pen.0));
        let width = (rect.right - rect.left).max(1);
        let amplitude = ((rect.bottom - rect.top) as f32 * amp_factor / 2.0).max(2.0);
        for step in 0..=64 {
            let x = rect.left + width * step / 64;
            let phase = step as f32 * 0.30 + frame as f32 * 0.09 * speed;
            let y = center_y + (phase.sin() * amplitude).round() as i32;
            if step == 0 {
                let _ = MoveToEx(hdc, x, y, None);
            } else {
                let _ = LineTo(hdc, x, y);
            }
        }
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(HPEN(pen.0).into());
    }
}

fn draw_stage_dots(
    hdc: HDC,
    rect: RECT,
    kind: HudActivityKind,
    frame: u32,
    accent: COLORREF,
    muted: COLORREF,
) {
    let active = match kind {
        HudActivityKind::Listening => 0,
        HudActivityKind::Recognizing => 1,
        HudActivityKind::Rewriting
        | HudActivityKind::Translating
        | HudActivityKind::RawPastedRewriting => 2,
        HudActivityKind::Done => 3,
        HudActivityKind::Warning => 2,
        HudActivityKind::Text => 0,
    };
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let dot_radius = (height / 10).clamp(3, 5);
    let segment = width / 4;
    let cy = rect.top + height / 2;
    for index in 0..4 {
        let cx = rect.left + segment * index as i32 + segment / 2;
        let pulse = ((frame as f32 * 0.12 + index as f32).sin() * 0.5 + 0.5) * 2.0;
        let radius = if index == active {
            dot_radius + pulse.round() as i32
        } else {
            dot_radius
        };
        let color = if index <= active { accent } else { muted };
        fill_ellipse_color(
            hdc,
            RECT {
                left: cx - radius,
                top: cy - radius,
                right: cx + radius,
                bottom: cy + radius,
            },
            color,
        );
        if index > 0 {
            let mut connector = rect;
            connector.left =
                rect.left + segment * (index as i32 - 1) + segment / 2 + dot_radius + 5;
            connector.right = cx - dot_radius - 5;
            connector.top = cy - 1;
            connector.bottom = cy + 1;
            if connector.right > connector.left {
                fill_rect_color(hdc, connector, if index <= active { accent } else { muted });
            }
        }
    }
}

fn draw_ai_glow(
    hdc: HDC,
    rect: RECT,
    kind: HudActivityKind,
    frame: u32,
    accent: COLORREF,
    muted: COLORREF,
    style: &HudPaintStyle,
) {
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let pulse = (frame as f32 * 0.10).sin() * 0.5 + 0.5;
    let glow_width = (width as f32 * (0.16 + pulse * 0.24)).round() as i32;
    let glow_left = rect.left + (width - glow_width) / 2;
    fill_rect_color(
        hdc,
        RECT {
            left: glow_left,
            top: rect.top + height / 2 - 2,
            right: glow_left + glow_width,
            bottom: rect.top + height / 2 + 2,
        },
        blend_color(style.background_color, accent, 0.42),
    );
    let mut rail = rect;
    rail.top = rect.top + height / 2 - 1;
    rail.bottom = rect.top + height / 2 + 1;
    fill_rect_color(hdc, rail, muted);
    draw_stage_dots(hdc, rect, kind, frame, accent, muted);
}

fn draw_minimal_pulse(
    hdc: HDC,
    rect: RECT,
    kind: HudActivityKind,
    frame: u32,
    accent: COLORREF,
    muted: COLORREF,
    style: &HudPaintStyle,
) {
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let pulse = (frame as f32 * 0.13).sin() * 0.5 + 0.5;
    let core_radius = ((height as f32 * (0.10 + pulse * 0.04)).round() as i32).clamp(3, 7);
    let halo_radius = (core_radius + 5 + (pulse * 3.0).round() as i32).clamp(8, 15);
    let cx = rect.left + 24.min(width / 2);
    let cy = rect.top + height / 2;
    fill_ellipse_color(
        hdc,
        RECT {
            left: cx - halo_radius,
            top: cy - halo_radius,
            right: cx + halo_radius,
            bottom: cy + halo_radius,
        },
        muted,
    );
    fill_ellipse_color(
        hdc,
        RECT {
            left: cx - core_radius,
            top: cy - core_radius,
            right: cx + core_radius,
            bottom: cy + core_radius,
        },
        accent,
    );
    let mut label_rect = rect;
    label_rect.left = (rect.left + 48).min(rect.right);
    draw_compact_status_label(hdc, label_rect, kind, accent, style);
}

fn draw_compact_status_label(
    hdc: HDC,
    rect: RECT,
    kind: HudActivityKind,
    accent: COLORREF,
    style: &HudPaintStyle,
) {
    let label = kind.short_label();
    if label.is_empty() {
        return;
    }
    let mut label_style = style.clone();
    label_style.text_color = blend_color(style.text_color, accent, 0.26);
    label_style.shadow_enabled = false;
    draw_hud_single_line(hdc, label, rect, false, false, &label_style);
}

fn fill_rect_color(hdc: HDC, rect: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        if brush.is_invalid() {
            return;
        }
        let _ = FillRect(hdc, &rect, brush);
        let _ = DeleteObject(brush.into());
    }
}

fn fill_ellipse_color(hdc: HDC, rect: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        if brush.is_invalid() {
            return;
        }
        let pen = windows::Win32::Graphics::Gdi::CreatePen(PS_SOLID, 1, color);
        let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
        let old_pen = if pen.is_invalid() {
            HGDIOBJ::default()
        } else {
            SelectObject(hdc, HGDIOBJ(pen.0))
        };
        let _ = Ellipse(hdc, rect.left, rect.top, rect.right, rect.bottom);
        let _ = SelectObject(hdc, old_brush);
        if !old_pen.is_invalid() {
            let _ = SelectObject(hdc, old_pen);
        }
        let _ = DeleteObject(brush.into());
        if !pen.is_invalid() {
            let _ = DeleteObject(HPEN(pen.0).into());
        }
    }
}

fn activity_accent_color(kind: HudActivityKind, frame: u32) -> COLORREF {
    match kind {
        HudActivityKind::Listening => hsl_to_colorref((186 + frame % 10) as f32, 0.48, 0.66),
        HudActivityKind::Recognizing => hsl_to_colorref((212 + frame % 12) as f32, 0.46, 0.68),
        HudActivityKind::Rewriting | HudActivityKind::RawPastedRewriting => {
            hsl_to_colorref((266 + frame % 14) as f32, 0.42, 0.70)
        }
        HudActivityKind::Translating => hsl_to_colorref((146 + frame % 12) as f32, 0.42, 0.64),
        HudActivityKind::Done => hsl_to_colorref(126.0, 0.44, 0.62),
        HudActivityKind::Warning => hsl_to_colorref(38.0, 0.58, 0.64),
        HudActivityKind::Text => hsl_to_colorref(0.0, 0.0, 0.96),
    }
}

fn blend_color(left: COLORREF, right: COLORREF, amount: f32) -> COLORREF {
    let amount = amount.clamp(0.0, 1.0);
    let (lr, lg, lb) = colorref_to_rgb(left);
    let (rr, rg, rb) = colorref_to_rgb(right);
    rgb_to_colorref(
        (lr as f32 + (rr as f32 - lr as f32) * amount).round() as u8,
        (lg as f32 + (rg as f32 - lg as f32) * amount).round() as u8,
        (lb as f32 + (rb as f32 - lb as f32) * amount).round() as u8,
    )
}

fn draw_rainbow_hud_line(
    hdc: HDC,
    text: &str,
    mut rect: RECT,
    align: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
    latest_on_overflow: bool,
    style: &HudPaintStyle,
) {
    let text_width = measure_hud_text_width(hdc, text);
    let available_width = (rect.right - rect.left).max(1);
    let clip_rect = rect;
    if latest_on_overflow && text_width > available_width {
        rect = latest_text_draw_rect(rect, text_width);
    } else if align == DT_CENTER && text_width < available_width {
        rect.left += (available_width - text_width) / 2;
    } else if align == DT_RIGHT && text_width < available_width {
        rect.left = rect.right - text_width;
    }
    let mut x = rect.left;
    unsafe {
        let saved_dc = SaveDC(hdc);
        let _ = IntersectClipRect(
            hdc,
            clip_rect.left,
            clip_rect.top,
            clip_rect.right,
            clip_rect.bottom,
        );
        for (index, ch) in text.chars().enumerate() {
            let char_text = ch.to_string();
            let char_width = measure_hud_text_width(hdc, &char_text).max(1);
            let hue = ((index as u16 * style.rainbow_step_degree) % 360) as f32;
            let color = hsl_to_colorref(
                hue,
                style.rainbow_saturation_percent as f32 / 100.0,
                style.rainbow_lightness_percent as f32 / 100.0,
            );
            let _ = SetTextColor(hdc, color);
            let mut char_rect = rect;
            char_rect.left = x;
            char_rect.right = (x + char_width + 2).min(rect.right);
            if char_rect.right > char_rect.left && char_rect.right > clip_rect.left {
                let mut wide = char_text.encode_utf16().collect::<Vec<_>>();
                let _ = DrawTextW(
                    hdc,
                    wide.as_mut_slice(),
                    &mut char_rect,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_LEFT,
                );
            }
            x += char_width;
            if x >= rect.right {
                break;
            }
        }
        if saved_dc != 0 {
            let _ = RestoreDC(hdc, saved_dc);
        }
    }
}

fn latest_text_draw_rect(rect: RECT, text_width: i32) -> RECT {
    let available_width = (rect.right - rect.left).max(1);
    if text_width <= available_width {
        return rect;
    }
    let mut draw_rect = rect;
    draw_rect.left = rect.right - text_width;
    draw_rect
}

fn measure_hud_text_width(hdc: HDC, text: &str) -> i32 {
    if text.trim().is_empty() {
        return 0;
    }
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut text_wide = text.encode_utf16().collect::<Vec<_>>();
    unsafe {
        let _ = DrawTextW(
            hdc,
            text_wide.as_mut_slice(),
            &mut rect,
            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
    (rect.right - rect.left).max(0)
}

fn smooth_step(current: f32, target: f32, amount: f32) -> f32 {
    current + (target - current) * amount
}

fn clamp_i32(value: i32, min: i32, max: i32) -> i32 {
    if min > max {
        return min;
    }
    value.clamp(min, max)
}

fn normalize_font_height_px(value: i32) -> i32 {
    if value > 0 {
        value.clamp(8, 240)
    } else {
        HudFontAppearance::default().height_px
    }
}

fn auto_fit_font_height_px(height_px: i32, padding_y_px: i32) -> i32 {
    (height_px - padding_y_px.max(0) * 2 - 3).clamp(8, 240)
}

fn short_hud_text(text: &str, max_chars: usize) -> String {
    let mut value = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        value.push_str("...");
    }
    value
}

fn longest_common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(a, b)| a == b)
        .count()
}

fn take_prefix_chars(text: &str, char_count: usize) -> String {
    text.chars().take(char_count).collect()
}

fn split_committed_prefix(text: &str) -> (String, String) {
    let mut committed_end = 0usize;
    let mut boundary_seen = false;
    for (index, ch) in text.char_indices() {
        let char_end = index + ch.len_utf8();
        if is_sentence_commit_char(ch) {
            committed_end = char_end;
            boundary_seen = true;
            continue;
        }
        if boundary_seen && is_sentence_trailing_char(ch) {
            committed_end = char_end;
        }
    }
    let (committed, live) = text.split_at(committed_end);
    (committed.to_string(), live.to_string())
}

fn is_sentence_commit_char(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '!' | '?' | '；' | ';')
}

fn is_sentence_trailing_char(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\t'
            | '\n'
            | '\r'
            | '"'
            | '\''
            | '”'
            | '’'
            | ')'
            | '）'
            | ']'
            | '】'
            | '>'
            | '》'
            | '〉'
            | '」'
            | '』'
    )
}

fn parse_color_ref(value: &str, fallback: &str) -> COLORREF {
    parse_color_ref_hex(value)
        .unwrap_or_else(|| parse_color_ref_hex(fallback).unwrap_or(COLORREF(0x00111111)))
}

fn parse_color_ref_hex(value: &str) -> Option<COLORREF> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let rgb = u32::from_str_radix(hex, 16).ok()?;
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    Some(COLORREF((b << 16) | (g << 8) | r))
}

fn blend_with_key(color: COLORREF, alpha: u8) -> COLORREF {
    let t = alpha as f32 / 255.0;
    let (r, g, b) = colorref_to_rgb(color);
    let (kr, kg, kb) = colorref_to_rgb(HUD_TEXT_COLORKEY);
    rgb_to_colorref(
        (kr as f32 + (r as f32 - kr as f32) * t).round() as u8,
        (kg as f32 + (g as f32 - kg as f32) * t).round() as u8,
        (kb as f32 + (b as f32 - kb as f32) * t).round() as u8,
    )
}

fn hsl_to_colorref(hue: f32, saturation: f32, lightness: f32) -> COLORREF {
    let c = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = (hue / 60.0) % 6.0;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r1, g1, b1) = if (0.0..1.0).contains(&h) {
        (c, x, 0.0)
    } else if (1.0..2.0).contains(&h) {
        (x, c, 0.0)
    } else if (2.0..3.0).contains(&h) {
        (0.0, c, x)
    } else if (3.0..4.0).contains(&h) {
        (0.0, x, c)
    } else if (4.0..5.0).contains(&h) {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = lightness - c / 2.0;
    rgb_to_colorref(
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn colorref_to_rgb(color: COLORREF) -> (u8, u8, u8) {
    (
        (color.0 & 0xff) as u8,
        ((color.0 >> 8) & 0xff) as u8,
        ((color.0 >> 16) & 0xff) as u8,
    )
}

fn rgb_to_colorref(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
}

#[cfg(test)]
mod tests {
    use super::{
        HudFontAppearance, HudMicrostreamState, latest_text_draw_rect, normalize_font_height_px,
        split_hud_status_text,
    };
    use windows::Win32::Foundation::RECT;

    #[test]
    fn microstream_retarget_resets_unrelated_sentence() {
        let mut state = HudMicrostreamState::default();
        state.retarget("嗯。");
        state.advance_chars(8);
        assert_eq!(state.display_message(), "嗯。");

        state.retarget("毕竟人家");
        assert_eq!(state.display_message(), "");
        assert_eq!(state.target_message(), "毕竟人家");

        state.advance_one_char();
        assert_eq!(state.display_message(), "毕");
    }

    #[test]
    fn microstream_retarget_resets_unrelated_mixed_text() {
        let mut state = HudMicrostreamState::default();
        state.retarget("是。AI.");
        state.advance_chars(16);
        assert_eq!(state.display_message(), "是。AI.");

        state.retarget("太牛逼了");
        assert_eq!(state.display_message(), "");
        assert_eq!(state.target_message(), "太牛逼了");
    }

    #[test]
    fn microstream_retarget_preserves_visible_prefix_for_growth() {
        let mut state = HudMicrostreamState::default();
        state.retarget("你好");
        state.advance_chars(2);
        state.retarget("你好世界");

        assert_eq!(state.display_message(), "你好");
        assert_eq!(state.target_message(), "你好世界");
    }

    #[test]
    fn font_height_keeps_large_values_without_legacy_cap() {
        assert_eq!(normalize_font_height_px(180), 180);
        let mut appearance = HudFontAppearance {
            height_px: 180,
            ..HudFontAppearance::default()
        };
        appearance.normalize();
        assert_eq!(appearance.height_px, 180);
    }

    #[test]
    fn split_hud_status_text_recognizes_rewrite_prefixes() {
        assert_eq!(
            split_hud_status_text("改写成功 1428ms | 这是最终文字"),
            Some(("改写成功 1428ms", "这是最终文字"))
        );
        assert_eq!(
            split_hud_status_text("未调用AI | 这是最终文字"),
            Some(("未调用AI", "这是最终文字"))
        );
        assert_eq!(
            split_hud_status_text("翻译成功 918ms | This is final text."),
            Some(("翻译成功 918ms", "This is final text."))
        );
        assert_eq!(split_hud_status_text("普通 HUD 文字 | 内容"), None);
    }

    #[test]
    fn latest_text_draw_rect_keeps_fitting_text_unchanged() {
        let rect = RECT {
            left: 10,
            top: 2,
            right: 210,
            bottom: 42,
        };
        assert_eq!(latest_text_draw_rect(rect, 120), rect);
    }

    #[test]
    fn latest_text_draw_rect_shifts_overflow_left() {
        let rect = RECT {
            left: 10,
            top: 2,
            right: 210,
            bottom: 42,
        };
        let shifted = latest_text_draw_rect(rect, 320);
        assert_eq!(shifted.left, -110);
        assert_eq!(shifted.right, rect.right);
        assert_eq!(shifted.top, rect.top);
        assert_eq!(shifted.bottom, rect.bottom);
    }
}
