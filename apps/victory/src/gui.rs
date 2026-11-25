//! Standalone native Win32/D3D12 launcher and settings GUI for VIctory.
//!
//! Opens when `victory` is launched without a subcommand or via `victory gui`.
//! Probes the host DXGI/D3D12 adapter (GPU name, vendor, dedicated VRAM,
//! Shader Model 6.6, Mesh Shaders, and Wave32/Wave64 lane support), lets the
//! user configure paths, console profile, AMD FSR 1.0 upscaling, and VRAM
//! reduction settings, saves `victory.toml`, and launches the title.

use std::path::{Path, PathBuf};

use anyhow::Result;
use victory_core::{Config, GpuConfig};
use victory_gpu::fsr::FsrMode;

use crate::{Console, Unimplemented};

/// Portable launcher settings model backed by `victory.toml` (`Config`) and
/// `VICTORY_*` environment variables.
#[derive(Debug, Clone, PartialEq)]
pub struct LauncherState {
    pub exe_path: String,
    pub game_dir: String,
    pub cache_dir: String,
    pub data_dir: String,
    pub console: Console,
    pub unimplemented: Unimplemented,
    pub strict: bool,
    pub fsr_mode: FsrMode,
    pub fsr_sharpness: f32,
    pub fsr_resolution: String,
    pub vram_budget_mb: u64,
    pub texture_max: u64,
    pub card_buffers_mb: u64,
    pub staging_budget_mb: u64,
    pub keep_frames: u64,
    pub ambient: String,
    pub hair: bool,
}

impl LauncherState {
    /// Initializes launcher state from resolved configuration and auto-detects
    /// the main title executable in `game_dir` if one is not yet recorded.
    pub fn from_config(cfg: &Config) -> Self {
        let exe_path = cfg
            .gpu
            .game_exe
            .as_ref()
            .map(|p| p.display().to_string())
            .or_else(|| cfg.game_dir.as_deref().and_then(find_title_exe))
            .unwrap_or_default();
        let game_dir = cfg
            .game_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let console = match cfg.gpu.console.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("series-s") || s.eq_ignore_ascii_case("seriess") => {
                Console::SeriesS
            }
            _ => Console::SeriesX,
        };
        let fsr_mode = cfg
            .gpu
            .fsr
            .as_deref()
            .map(FsrMode::parse)
            .unwrap_or(FsrMode::Quality);
        let fsr_sharpness = cfg.gpu.fsr_sharpness.unwrap_or(0.2).clamp(0.0, 2.0);
        let fsr_resolution = cfg
            .gpu
            .fsr_resolution
            .clone()
            .unwrap_or_else(|| "auto".to_owned());
        LauncherState {
            exe_path,
            game_dir,
            cache_dir: cfg.cache_dir.display().to_string(),
            data_dir: cfg.data_dir.display().to_string(),
            console,
            unimplemented: Unimplemented::Trap,
            strict: false,
            fsr_mode,
            fsr_sharpness,
            fsr_resolution,
            vram_budget_mb: cfg.gpu.vram_budget_mb.unwrap_or(0),
            texture_max: cfg.gpu.texture_max.unwrap_or(0),
            card_buffers_mb: cfg.gpu.card_buffers_mb.unwrap_or(512),
            staging_budget_mb: cfg.gpu.staging_budget_mb.unwrap_or(192),
            keep_frames: cfg.gpu.keep_frames.unwrap_or(8),
            ambient: cfg.gpu.ambient.clone().unwrap_or_else(|| "0.6".to_owned()),
            hair: cfg.gpu.hair.unwrap_or(false),
        }
    }

    /// Applies the low-VRAM preset tailored for 8–12 GB GPUs.
    pub fn apply_low_vram_preset(&mut self) {
        self.console = Console::SeriesS;
        self.fsr_mode = FsrMode::Quality;
        self.fsr_sharpness = 0.2;
        self.vram_budget_mb = 4096;
        self.texture_max = 512;
        self.card_buffers_mb = 256;
        self.staging_budget_mb = 128;
        self.keep_frames = 4;
    }

    /// Applies the ultra-low-VRAM preset tailored for 4 GB GPUs (e.g. GTX 1650,
    /// RTX 3050 4 GB, RX 6500 XT): Series S profile, FSR 1.0 Performance (2.0x),
    /// 1536 MiB texture VRAM cap, 128 px max streamed block-compressed texture
    /// dimension, 0 MiB VRAM card buffers (all shadow buffers in L0 system RAM),
    /// 96 MiB staging budget, and 2-frame resource retirement.
    pub fn apply_4gb_vram_preset(&mut self) {
        self.console = Console::SeriesS;
        self.fsr_mode = FsrMode::Performance;
        self.fsr_sharpness = 0.2;
        self.vram_budget_mb = 1536;
        self.texture_max = 128;
        self.card_buffers_mb = 0;
        self.staging_budget_mb = 96;
        self.keep_frames = 2;
    }

    /// Converts the UI state back into a [`Config`] ready to be saved to `victory.toml`.
    pub fn to_config(&self, base: &Config) -> Config {
        let non_empty_path = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| PathBuf::from(t))
        };
        let game_exe = non_empty_path(&self.exe_path);
        let game_dir = non_empty_path(&self.game_dir).or_else(|| {
            game_exe
                .as_ref()
                .and_then(|p| p.parent().map(Path::to_path_buf))
        });
        let res = {
            let r = self.fsr_resolution.trim();
            (!r.is_empty() && !r.eq_ignore_ascii_case("auto")).then(|| r.to_owned())
        };
        Config {
            game_dir,
            cache_dir: non_empty_path(&self.cache_dir).unwrap_or_else(|| base.cache_dir.clone()),
            data_dir: non_empty_path(&self.data_dir).unwrap_or_else(|| base.data_dir.clone()),
            test_titles: base.test_titles.clone(),
            gpu: GpuConfig {
                game_exe,
                console: Some(
                    match self.console {
                        Console::SeriesX => "series-x",
                        Console::SeriesS => "series-s",
                    }
                    .to_owned(),
                ),
                fsr: Some(self.fsr_mode.as_config_str().to_owned()),
                fsr_sharpness: Some(self.fsr_sharpness),
                fsr_resolution: res,
                vram_budget_mb: (self.vram_budget_mb > 0).then_some(self.vram_budget_mb),
                texture_max: (self.texture_max > 0).then_some(self.texture_max),
                card_buffers_mb: Some(self.card_buffers_mb),
                staging_budget_mb: Some(self.staging_budget_mb),
                keep_frames: Some(self.keep_frames),
                ambient: Some(self.ambient.trim().to_owned()),
                hair: Some(self.hair),
            },
            source: base.source.clone(),
        }
    }

    /// Applies the launcher settings to the current process's environment
    /// variables before starting the title.
    pub fn apply_env(&self) {
        let set = |k: &str, v: &str| {
            // SAFETY: called on the main GUI thread before title threads start.
            unsafe { std::env::set_var(k, v) };
        };
        let remove = |k: &str| {
            // SAFETY: as above.
            unsafe { std::env::remove_var(k) };
        };
        if !self.game_dir.trim().is_empty() {
            set("VICTORY_GAME_DIR", self.game_dir.trim());
        }
        if !self.cache_dir.trim().is_empty() {
            set("VICTORY_CACHE_DIR", self.cache_dir.trim());
        }
        if !self.data_dir.trim().is_empty() {
            set("VICTORY_DATA_DIR", self.data_dir.trim());
        }
        set("VICTORY_FSR", self.fsr_mode.as_config_str());
        set("VICTORY_FSR_SHARPNESS", &format!("{:.2}", self.fsr_sharpness));
        if !self.fsr_resolution.trim().is_empty()
            && !self.fsr_resolution.trim().eq_ignore_ascii_case("auto")
        {
            set("VICTORY_FSR_RESOLUTION", self.fsr_resolution.trim());
        } else {
            remove("VICTORY_FSR_RESOLUTION");
        }
        if self.vram_budget_mb > 0 {
            set("VICTORY_VRAM_BUDGET_MB", &self.vram_budget_mb.to_string());
        } else {
            remove("VICTORY_VRAM_BUDGET_MB");
        }
        if self.texture_max > 0 {
            set("VICTORY_TEXTURE_MAX", &self.texture_max.to_string());
        } else {
            remove("VICTORY_TEXTURE_MAX");
        }
        if self.card_buffers_mb == 0 {
            set("VICTORY_CARD_BUFFERS", "0");
        } else {
            set("VICTORY_CARD_BUFFERS", "1");
            set("VICTORY_CARD_BUFFERS_MB", &self.card_buffers_mb.to_string());
        }
        set(
            "VICTORY_STAGING_BUDGET_MB",
            &self.staging_budget_mb.to_string(),
        );
        set("VICTORY_KEEP_FRAMES", &self.keep_frames.to_string());
        if !self.ambient.trim().is_empty() {
            set("VICTORY_AMBIENT", self.ambient.trim());
        }
        if self.hair {
            set("VICTORY_HAIR", "1");
        } else {
            remove("VICTORY_HAIR");
        }
    }
}

/// Scans `dir` for a likely Xbox Series title executable (`*scarlett*.exe` or
/// the largest `.exe` in the directory).
fn find_title_exe(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(bool, u64, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("exe") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let scarlett = name.contains("scarlett") || name.contains("game");
        let len = entry.metadata().map_or(0, |m| m.len());
        if best
            .as_ref()
            .is_none_or(|(b_scarlett, b_len, _)| (scarlett, len) > (*b_scarlett, *b_len))
        {
            best = Some((scarlett, len, path));
        }
    }
    best.map(|(_, _, p)| p.display().to_string())
}

#[cfg(not(windows))]
pub fn run() -> Result<()> {
    anyhow::bail!("the native Win32/D3D12 GUI launcher requires Windows")
}

#[cfg(windows)]
pub fn run() -> Result<()> {
    win32::run_launcher()
}

#[cfg(windows)]
mod win32 {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use anyhow::Result;
    use victory_core::Config;
    use victory_gpu::fsr::FsrMode;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{PCWSTR, w};

    use super::LauncherState;
    use crate::{Console, Unimplemented};

    const ID_EXE_EDIT: usize = 101;
    const ID_EXE_BROWSE: usize = 102;
    const ID_GAME_DIR_EDIT: usize = 103;
    const ID_CACHE_DIR_EDIT: usize = 104;
    const ID_DATA_DIR_EDIT: usize = 105;
    const ID_CONSOLE_COMBO: usize = 106;
    const ID_UNIMPL_COMBO: usize = 107;
    const ID_STRICT_CHECK: usize = 108;
    const ID_FSR_MODE_COMBO: usize = 109;
    const ID_FSR_SHARP_COMBO: usize = 110;
    const ID_FSR_RES_COMBO: usize = 111;
    const ID_VRAM_BUDGET_COMBO: usize = 112;
    const ID_TEX_MAX_COMBO: usize = 113;
    const ID_CARD_BUF_COMBO: usize = 114;
    const ID_STAGING_COMBO: usize = 115;
    const ID_AMBIENT_COMBO: usize = 116;
    const ID_HAIR_CHECK: usize = 117;
    const ID_PRESET_LOW_VRAM: usize = 118;
    const ID_SAVE_BTN: usize = 119;
    const ID_LAUNCH_BTN: usize = 120;
    const ID_PRESET_4GB_VRAM: usize = 121;

    const CB_ADDSTRING: u32 = 0x0143;
    const CB_SETCURSEL: u32 = 0x014E;
    const CB_GETCURSEL: u32 = 0x0147;
    const BM_SETCHECK: u32 = 0x00F1;
    const BM_GETCHECK: u32 = 0x00F0;
    const BST_CHECKED: usize = 1;

    #[repr(C)]
    struct OpenFileNameW {
        l_struct_size: u32,
        hwnd_owner: HWND,
        h_instance: isize,
        lpstr_filter: *const u16,
        lpstr_custom_filter: *mut u16,
        n_max_cust_filter: u32,
        n_filter_index: u32,
        lpstr_file: *mut u16,
        n_max_file: u32,
        lpstr_file_title: *mut u16,
        n_max_file_title: u32,
        lpstr_initial_dir: *const u16,
        lpstr_title: *const u16,
        flags: u32,
        n_file_offset: u16,
        n_file_extension: u16,
        lpstr_def_ext: *const u16,
        l_cust_data: isize,
        lpfn_hook: usize,
        lp_template_name: *const u16,
        pv_reserved: *mut core::ffi::c_void,
        dw_reserved: u32,
        flags_ex: u32,
    }

    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
    }

    struct UiHandles {
        exe_edit: HWND,
        game_dir_edit: HWND,
        cache_dir_edit: HWND,
        data_dir_edit: HWND,
        console_combo: HWND,
        unimpl_combo: HWND,
        strict_check: HWND,
        fsr_mode_combo: HWND,
        fsr_sharp_combo: HWND,
        fsr_res_combo: HWND,
        vram_budget_combo: HWND,
        tex_max_combo: HWND,
        card_buf_combo: HWND,
        staging_combo: HWND,
        ambient_combo: HWND,
        hair_check: HWND,
        status_label: HWND,
        cfg: Config,
        state: LauncherState,
        launch_requested: bool,
    }

    thread_local! {
        static UI: RefCell<Option<UiHandles>> = const { RefCell::new(None) };
    }

    const FSR_MODES: [FsrMode; 5] = [
        FsrMode::Off,
        FsrMode::UltraQuality,
        FsrMode::Quality,
        FsrMode::Balanced,
        FsrMode::Performance,
    ];
    const FSR_SHARP_VALUES: [f32; 4] = [0.0, 0.2, 0.5, 1.0];
    const FSR_RES_VALUES: [&str; 4] = ["auto", "1920x1080", "2560x1440", "3840x2160"];
    const VRAM_BUDGET_VALUES: [u64; 7] = [0, 1536, 2048, 4096, 6144, 8192, 12288];
    const TEX_MAX_VALUES: [u64; 6] = [0, 2048, 1024, 512, 256, 128];
    const CARD_BUF_VALUES: [u64; 4] = [512, 256, 1024, 0];
    const STAGING_VALUES: [u64; 4] = [192, 128, 96, 384];
    const AMBIENT_VALUES: [&str; 4] = ["0.6", "0.8", "0.4", "off"];

    /// Probes the primary high-performance DXGI/D3D12 adapter for the hardware
    /// banner at the top of the launcher window.
    fn probe_gpu_banner() -> String {
        // SAFETY: plain DXGI and D3D12 feature queries on the primary adapter.
        unsafe {
            let Ok(factory) = CreateDXGIFactory2::<IDXGIFactory6>(DXGI_CREATE_FACTORY_FLAGS(0))
            else {
                return "Host GPU: DXGI unavailable".to_owned();
            };
            let Ok(adapter) =
                factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
            else {
                return "Host GPU: No D3D12 adapter found".to_owned();
            };
            let Ok(desc) = adapter.GetDesc1() else {
                return "Host GPU: Unknown adapter".to_owned();
            };
            let end = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..end]);
            let vram_mib = desc.DedicatedVideoMemory >> 20;
            let vendor = match desc.VendorId {
                0x10DE => "NVIDIA",
                0x1002 | 0x1022 => "AMD",
                0x8086 => "Intel",
                _ => "GPU",
            };
            let mut device: Option<ID3D12Device> = None;
            if D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device).is_ok()
                && let Some(dev) = device
            {
                let mut opt1 = D3D12_FEATURE_DATA_D3D12_OPTIONS1::default();
                let _ = dev.CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS1,
                    (&raw mut opt1).cast(),
                    size_of_val(&opt1) as u32,
                );
                let mut opt7 = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
                let _ = dev.CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS7,
                    (&raw mut opt7).cast(),
                    size_of_val(&opt7) as u32,
                );
                let mesh = opt7.MeshShaderTier != D3D12_MESH_SHADER_TIER_NOT_SUPPORTED;
                let wave_note = if opt1.WaveLaneCountMax == 32 {
                    "Wave32 (NVIDIA Wave64-on-Wave32 Active)"
                } else {
                    "Wave32/Wave64 Native"
                };
                format!(
                    "{vendor}: {name} ({vram_mib} MiB VRAM)  |  Mesh Shaders: {}  |  Waves: {}-{} ({wave_note})",
                    if mesh { "Yes" } else { "No" },
                    opt1.WaveLaneCountMin,
                    opt1.WaveLaneCountMax
                )
            } else {
                format!("{vendor}: {name} ({vram_mib} MiB VRAM)")
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn set_text(hwnd: HWND, text: &str) {
        let w = wide(text);
        // SAFETY: valid control handle and NUL-terminated UTF-16 string.
        unsafe {
            let _ = SetWindowTextW(hwnd, PCWSTR(w.as_ptr()));
        }
    }

    fn get_text(hwnd: HWND) -> String {
        let mut buf = vec![0u16; 1024];
        // SAFETY: valid control handle and writable UTF-16 buffer.
        let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..len.max(0) as usize])
    }

    fn combo_add(combo: HWND, items: &[&str], selected: usize) {
        for &item in items {
            let w = wide(item);
            // SAFETY: sending CB_ADDSTRING with a NUL-terminated UTF-16 pointer.
            unsafe {
                SendMessageW(combo, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(w.as_ptr() as isize)));
            }
        }
        combo_set(combo, selected);
    }

    fn combo_set(combo: HWND, selected: usize) {
        // SAFETY: sending CB_SETCURSEL to a combobox control.
        unsafe {
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(selected)), Some(LPARAM(0)));
        }
    }

    fn combo_get(combo: HWND) -> usize {
        // SAFETY: sending CB_GETCURSEL to a combobox control.
        let idx = unsafe { SendMessageW(combo, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))) }.0;
        if idx < 0 { 0 } else { idx as usize }
    }

    fn check_set(btn: HWND, checked: bool) {
        // SAFETY: sending BM_SETCHECK to a checkbox button.
        unsafe {
            SendMessageW(
                btn,
                BM_SETCHECK,
                Some(WPARAM(if checked { BST_CHECKED } else { 0 })),
                Some(LPARAM(0)),
            );
        }
    }

    fn check_get(btn: HWND) -> bool {
        // SAFETY: sending BM_GETCHECK to a checkbox button.
        unsafe { SendMessageW(btn, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))) }.0
            == BST_CHECKED as isize
    }

    fn sync_ui_from_state(ui: &UiHandles) {
        set_text(ui.exe_edit, &ui.state.exe_path);
        set_text(ui.game_dir_edit, &ui.state.game_dir);
        set_text(ui.cache_dir_edit, &ui.state.cache_dir);
        set_text(ui.data_dir_edit, &ui.state.data_dir);
        combo_set(
            ui.console_combo,
            match ui.state.console {
                Console::SeriesX => 0,
                Console::SeriesS => 1,
            },
        );
        combo_set(
            ui.unimpl_combo,
            match ui.state.unimplemented {
                Unimplemented::Trap => 0,
                Unimplemented::Zero => 1,
            },
        );
        check_set(ui.strict_check, ui.state.strict);
        let fsr_idx = FSR_MODES
            .iter()
            .position(|&m| m == ui.state.fsr_mode)
            .unwrap_or(2);
        combo_set(ui.fsr_mode_combo, fsr_idx);
        let sharp_idx = FSR_SHARP_VALUES
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (ui.state.fsr_sharpness - **a)
                    .abs()
                    .total_cmp(&(ui.state.fsr_sharpness - **b).abs())
            })
            .map_or(1, |(i, _)| i);
        combo_set(ui.fsr_sharp_combo, sharp_idx);
        let res_idx = FSR_RES_VALUES
            .iter()
            .position(|r| r.eq_ignore_ascii_case(&ui.state.fsr_resolution))
            .unwrap_or(0);
        combo_set(ui.fsr_res_combo, res_idx);
        let vram_idx = VRAM_BUDGET_VALUES
            .iter()
            .position(|&v| v == ui.state.vram_budget_mb)
            .unwrap_or(0);
        combo_set(ui.vram_budget_combo, vram_idx);
        let tex_idx = TEX_MAX_VALUES
            .iter()
            .position(|&v| v == ui.state.texture_max)
            .unwrap_or(0);
        combo_set(ui.tex_max_combo, tex_idx);
        let card_idx = CARD_BUF_VALUES
            .iter()
            .position(|&v| v == ui.state.card_buffers_mb)
            .unwrap_or(0);
        combo_set(ui.card_buf_combo, card_idx);
        let stage_idx = STAGING_VALUES
            .iter()
            .position(|&v| v == ui.state.staging_budget_mb)
            .unwrap_or(0);
        combo_set(ui.staging_combo, stage_idx);
        let amb_idx = AMBIENT_VALUES
            .iter()
            .position(|a| a.eq_ignore_ascii_case(&ui.state.ambient))
            .unwrap_or(0);
        combo_set(ui.ambient_combo, amb_idx);
        check_set(ui.hair_check, ui.state.hair);
    }

    fn read_state_from_ui(ui: &mut UiHandles) {
        ui.state.exe_path = get_text(ui.exe_edit);
        ui.state.game_dir = get_text(ui.game_dir_edit);
        if ui.state.game_dir.trim().is_empty()
            && let Some(parent) = std::path::Path::new(ui.state.exe_path.trim()).parent()
            && !parent.as_os_str().is_empty()
        {
            ui.state.game_dir = parent.display().to_string();
        }
        ui.state.cache_dir = get_text(ui.cache_dir_edit);
        ui.state.data_dir = get_text(ui.data_dir_edit);
        ui.state.console = if combo_get(ui.console_combo) == 1 {
            Console::SeriesS
        } else {
            Console::SeriesX
        };
        ui.state.unimplemented = if combo_get(ui.unimpl_combo) == 1 {
            Unimplemented::Zero
        } else {
            Unimplemented::Trap
        };
        ui.state.strict = check_get(ui.strict_check);
        ui.state.fsr_mode = *FSR_MODES
            .get(combo_get(ui.fsr_mode_combo))
            .unwrap_or(&FsrMode::Quality);
        ui.state.fsr_sharpness = *FSR_SHARP_VALUES
            .get(combo_get(ui.fsr_sharp_combo))
            .unwrap_or(&0.2);
        ui.state.fsr_resolution = (*FSR_RES_VALUES
            .get(combo_get(ui.fsr_res_combo))
            .unwrap_or(&"auto"))
        .to_owned();
        ui.state.vram_budget_mb = *VRAM_BUDGET_VALUES
            .get(combo_get(ui.vram_budget_combo))
            .unwrap_or(&0);
        ui.state.texture_max = *TEX_MAX_VALUES
            .get(combo_get(ui.tex_max_combo))
            .unwrap_or(&0);
        ui.state.card_buffers_mb = *CARD_BUF_VALUES
            .get(combo_get(ui.card_buf_combo))
            .unwrap_or(&512);
        ui.state.staging_budget_mb = *STAGING_VALUES
            .get(combo_get(ui.staging_combo))
            .unwrap_or(&192);
        ui.state.ambient = (*AMBIENT_VALUES
            .get(combo_get(ui.ambient_combo))
            .unwrap_or(&"0.6"))
        .to_owned();
        ui.state.hair = check_get(ui.hair_check);
    }

    fn browse_exe(owner: HWND) -> Option<PathBuf> {
        let mut file_buf = vec![0u16; 1024];
        let filter: Vec<u16> = "Xbox Title Executables (*.exe)\0*.exe\0All Files (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
        let title = wide("Select Xbox Series Title Executable");
        let mut ofn = OpenFileNameW {
            l_struct_size: size_of::<OpenFileNameW>() as u32,
            hwnd_owner: owner,
            h_instance: 0,
            lpstr_filter: filter.as_ptr(),
            lpstr_custom_filter: std::ptr::null_mut(),
            n_max_cust_filter: 0,
            n_filter_index: 1,
            lpstr_file: file_buf.as_mut_ptr(),
            n_max_file: file_buf.len() as u32,
            lpstr_file_title: std::ptr::null_mut(),
            n_max_file_title: 0,
            lpstr_initial_dir: std::ptr::null(),
            lpstr_title: title.as_ptr(),
            flags: 0x0000_1000 | 0x0000_0800 | 0x0000_0008, // OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR
            n_file_offset: 0,
            n_file_extension: 0,
            lpstr_def_ext: std::ptr::null(),
            l_cust_data: 0,
            lpfn_hook: 0,
            lp_template_name: std::ptr::null(),
            pv_reserved: std::ptr::null_mut(),
            dw_reserved: 0,
            flags_ex: 0,
        };
        // SAFETY: `ofn` points to valid buffers that outlive `GetOpenFileNameW`.
        let ok = unsafe { GetOpenFileNameW(&raw mut ofn) != 0 };
        if !ok {
            return None;
        }
        let end = file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len());
        Some(PathBuf::from(String::from_utf16_lossy(&file_buf[..end])))
    }

    extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_COMMAND => {
                let id = wparam.0 & 0xFFFF;
                UI.with(|cell| {
                    let mut borrow = cell.borrow_mut();
                    let Some(ui) = borrow.as_mut() else {
                        return;
                    };
                    match id {
                        ID_EXE_BROWSE => {
                            if let Some(path) = browse_exe(hwnd) {
                                let s = path.display().to_string();
                                set_text(ui.exe_edit, &s);
                                if let Some(parent) = path.parent() {
                                    set_text(ui.game_dir_edit, &parent.display().to_string());
                                }
                                set_text(ui.status_label, "Selected title executable.");
                            }
                        }
                        ID_PRESET_LOW_VRAM => {
                            read_state_from_ui(ui);
                            ui.state.apply_low_vram_preset();
                            sync_ui_from_state(ui);
                            set_text(
                                ui.status_label,
                                "Applied 8 GB Low-VRAM preset (Series S, FSR Quality 1.5x, 4 GiB VRAM cap, 512px textures).",
                            );
                        }
                        ID_PRESET_4GB_VRAM => {
                            read_state_from_ui(ui);
                            ui.state.apply_4gb_vram_preset();
                            sync_ui_from_state(ui);
                            set_text(
                                ui.status_label,
                                "Applied 4 GB GPU preset (Series S, FSR Performance 2.0x, 1.5 GiB texture cap, 128px mips, L0 buffers).",
                            );
                        }
                        ID_SAVE_BTN => {
                            read_state_from_ui(ui);
                            let new_cfg = ui.state.to_config(&ui.cfg);
                            match new_cfg.save(None) {
                                Ok(path) => {
                                    ui.cfg = new_cfg;
                                    set_text(
                                        ui.status_label,
                                        &format!("Saved configuration to {}", path.display()),
                                    );
                                }
                                Err(e) => {
                                    set_text(ui.status_label, &format!("Save error: {e}"));
                                }
                            }
                        }
                        ID_LAUNCH_BTN => {
                            read_state_from_ui(ui);
                            let exe = PathBuf::from(ui.state.exe_path.trim());
                            if exe.as_os_str().is_empty() || !exe.is_file() {
                                set_text(
                                    ui.status_label,
                                    "Error: Please select a valid title executable (.exe) before launching.",
                                );
                                return;
                            }
                            let new_cfg = ui.state.to_config(&ui.cfg);
                            let _ = new_cfg.save(None);
                            ui.launch_requested = true;
                            // SAFETY: closes the launcher window so the message loop exits and starts the title.
                            unsafe {
                                let _ = DestroyWindow(hwnd);
                            }
                        }
                        _ => {}
                    }
                });
                LRESULT(0)
            }
            WM_CLOSE => {
                // SAFETY: standard window destruction on close.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // SAFETY: posts WM_QUIT to exit the launcher loop.
                unsafe {
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            // SAFETY: default window procedure for unhandled messages.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    pub fn run_launcher() -> Result<()> {
        let cfg = Config::load().unwrap_or_default();
        let state = LauncherState::from_config(&cfg);
        let banner = probe_gpu_banner();

        const CLASS_NAME: PCWSTR = w!("VIctoryLauncherWindow");
        // SAFETY: Win32 window and child control creation on the main thread.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                lpszClassName: CLASS_NAME,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as *mut _),
                ..Default::default()
            };
            RegisterClassExW(&class);

            let (win_w, win_h) = (760, 620);
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: win_w,
                bottom: win_h,
            };
            let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE;
            let _ = AdjustWindowRect(&mut rect, style, false);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_NAME,
                w!("VIctory Launcher — Xbox Series X|S Runtime, NVIDIA/AMD D3D12 & FSR 1.0"),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                None,
                None,
                Some(instance.into()),
                None,
            )?;

            let font = GetStockObject(DEFAULT_GUI_FONT);
            let apply_font = |ctrl: HWND| {
                SendMessageW(
                    ctrl,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            };

            let make_ctrl = |cls: PCWSTR,
                             text: &str,
                             st: WINDOW_STYLE,
                             ex: WINDOW_EX_STYLE,
                             (x, y, w, h): (i32, i32, i32, i32),
                             id: usize|
             -> HWND {
                let wt = wide(text);
                let ctrl = CreateWindowExW(
                    ex,
                    cls,
                    PCWSTR(wt.as_ptr()),
                    WS_CHILD | WS_VISIBLE | st,
                    x,
                    y,
                    w,
                    h,
                    Some(hwnd),
                    Some(HMENU(id as *mut _)),
                    Some(instance.into()),
                    None,
                )
                .unwrap_or_default();
                apply_font(ctrl);
                ctrl
            };

            let label = |text: &str, (x, y, w, h): (i32, i32, i32, i32)| {
                make_ctrl(w!("STATIC"), text, WINDOW_STYLE(0), WINDOW_EX_STYLE(0), (x, y, w, h), 0)
            };
            let edit = |text: &str, (x, y, w, h): (i32, i32, i32, i32), id: usize| {
                make_ctrl(
                    w!("EDIT"),
                    text,
                    WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                    WINDOW_EX_STYLE(0),
                    (x, y, w, h),
                    id,
                )
            };
            let combo = |(x, y, w, h): (i32, i32, i32, i32), id: usize| {
                make_ctrl(
                    w!("COMBOBOX"),
                    "",
                    WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                    WINDOW_EX_STYLE(0),
                    (x, y, w, h),
                    id,
                )
            };
            let button = |text: &str, (x, y, w, h): (i32, i32, i32, i32), id: usize| {
                make_ctrl(
                    w!("BUTTON"),
                    text,
                    WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                    WINDOW_EX_STYLE(0),
                    (x, y, w, h),
                    id,
                )
            };
            let checkbox = |text: &str, (x, y, w, h): (i32, i32, i32, i32), id: usize| {
                make_ctrl(
                    w!("BUTTON"),
                    text,
                    WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
                    WINDOW_EX_STYLE(0),
                    (x, y, w, h),
                    id,
                )
            };

            // Hardware status banner
            label(&banner, (16, 12, 728, 22));

            // Paths section
            label("Title Executable (.exe):", (16, 44, 160, 20));
            let exe_edit = edit(&state.exe_path, (180, 42, 470, 24), ID_EXE_EDIT);
            button("Browse...", (658, 41, 86, 26), ID_EXE_BROWSE);

            label("Game Directory:", (16, 76, 160, 20));
            let game_dir_edit = edit(&state.game_dir, (180, 74, 564, 24), ID_GAME_DIR_EDIT);

            label("Cache Directory:", (16, 108, 160, 20));
            let cache_dir_edit = edit(&state.cache_dir, (180, 106, 564, 24), ID_CACHE_DIR_EDIT);

            label("Data / Saves Directory:", (16, 140, 160, 20));
            let data_dir_edit = edit(&state.data_dir, (180, 138, 564, 24), ID_DATA_DIR_EDIT);

            // Console & Runtime section
            label("Console Profile:", (16, 180, 160, 20));
            let console_combo = combo((180, 176, 260, 120), ID_CONSOLE_COMBO);
            combo_add(
                console_combo,
                &[
                    "Xbox Series X (Default)",
                    "Xbox Series S (Lower VRAM Use)",
                ],
                0,
            );

            label("Unimplemented Imports:", (456, 180, 140, 20));
            let unimpl_combo = combo((600, 176, 144, 100), ID_UNIMPL_COMBO);
            combo_add(unimpl_combo, &["Trap", "Return Zero"], 0);

            // AMD FSR 1.0 Upscaling & Sharpening section
            label("AMD FSR 1.0 Upscaling:", (16, 222, 160, 20));
            let fsr_mode_combo = combo((180, 218, 260, 160), ID_FSR_MODE_COMBO);
            combo_add(
                fsr_mode_combo,
                &[
                    "Off (Native 1.0x)",
                    "Ultra Quality (1.3x Scale)",
                    "Quality (1.5x Scale - Recommended)",
                    "Balanced (1.7x Scale)",
                    "Performance (2.0x Scale)",
                ],
                2,
            );

            label("FSR RCAS Sharpness:", (456, 222, 140, 20));
            let fsr_sharp_combo = combo((600, 218, 144, 140), ID_FSR_SHARP_COMBO);
            combo_add(
                fsr_sharp_combo,
                &[
                    "0.0 Stops (Max)",
                    "0.2 Stops (Default)",
                    "0.5 Stops (Medium)",
                    "1.0 Stops (Soft)",
                ],
                1,
            );

            label("FSR Output Resolution:", (16, 258, 160, 20));
            let fsr_res_combo = combo((180, 254, 260, 140), ID_FSR_RES_COMBO);
            combo_add(
                fsr_res_combo,
                &[
                    "Auto (Scale by FSR Preset)",
                    "1920x1080 (1080p)",
                    "2560x1440 (1440p)",
                    "3840x2160 (4K UHD)",
                ],
                0,
            );

            // VRAM Consumption & GPU Memory Optimization section
            label("VRAM Texture Budget:", (16, 302, 160, 20));
            let vram_budget_combo = combo((180, 298, 260, 180), ID_VRAM_BUDGET_COMBO);
            combo_add(
                vram_budget_combo,
                &[
                    "Auto (45% on 4GB / 65% on 8GB+)",
                    "1536 MiB (4 GB GPU)",
                    "2048 MiB (4-6 GB GPU)",
                    "4096 MiB (6-8 GB GPU)",
                    "6144 MiB (8-12 GB GPU)",
                    "8192 MiB (12-16 GB GPU)",
                    "12288 MiB (16+ GB GPU)",
                ],
                0,
            );

            label("Max Texture Resolution:", (456, 302, 140, 20));
            let tex_max_combo = combo((600, 298, 144, 180), ID_TEX_MAX_COMBO);
            combo_add(
                tex_max_combo,
                &[
                    "Auto (By VRAM %)",
                    "2048 px",
                    "1024 px",
                    "512 px (8 GB GPU)",
                    "256 px (6 GB GPU)",
                    "128 px (4 GB GPU)",
                ],
                0,
            );

            label("VRAM Card Buffers Cap:", (16, 340, 160, 20));
            let card_buf_combo = combo((180, 336, 260, 140), ID_CARD_BUF_COMBO);
            combo_add(
                card_buf_combo,
                &[
                    "512 MiB (Recommended)",
                    "256 MiB (Low VRAM)",
                    "1024 MiB (High VRAM)",
                    "0 MiB (4 GB GPU - Host RAM Only)",
                ],
                0,
            );

            label("Staging Pool Budget:", (456, 340, 140, 20));
            let staging_combo = combo((600, 336, 144, 140), ID_STAGING_COMBO);
            combo_add(
                staging_combo,
                &[
                    "192 MiB (Default)",
                    "128 MiB (Low)",
                    "96 MiB (4 GB GPU)",
                    "384 MiB (High)",
                ],
                0,
            );

            label("Ambient Shadow Light:", (16, 378, 160, 20));
            let ambient_combo = combo((180, 374, 260, 140), ID_AMBIENT_COMBO);
            combo_add(
                ambient_combo,
                &[
                    "0.6 (Default Constant)",
                    "0.8 (Brighter Shadows)",
                    "0.4 (Darker Shadows)",
                    "off (Game Raw Resolve)",
                ],
                0,
            );

            let hair_check = checkbox(
                "Enable Hair Strands (VICTORY_HAIR)",
                (180, 414, 260, 24),
                ID_HAIR_CHECK,
            );
            let strict_check = checkbox(
                "Strict HLE Mode (No Host Pass-Through)",
                (456, 414, 288, 24),
                ID_STRICT_CHECK,
            );

            // Action Buttons
            button(
                "4 GB GPU Preset",
                (16, 464, 150, 34),
                ID_PRESET_4GB_VRAM,
            );
            button(
                "8 GB Low-VRAM Preset",
                (174, 464, 170, 34),
                ID_PRESET_LOW_VRAM,
            );
            button("Save Config", (352, 464, 160, 34), ID_SAVE_BTN);
            button("Launch Title", (520, 464, 224, 34), ID_LAUNCH_BTN);

            let status_label = label(
                "Ready. Configure FSR 1.0, 4 GB / 8 GB VRAM preset, and NVIDIA/AMD settings above, then click Launch Title.",
                (16, 516, 728, 40),
            );

            let handles = UiHandles {
                exe_edit,
                game_dir_edit,
                cache_dir_edit,
                data_dir_edit,
                console_combo,
                unimpl_combo,
                strict_check,
                fsr_mode_combo,
                fsr_sharp_combo,
                fsr_res_combo,
                vram_budget_combo,
                tex_max_combo,
                card_buf_combo,
                staging_combo,
                ambient_combo,
                hair_check,
                status_label,
                cfg,
                state,
                launch_requested: false,
            };
            sync_ui_from_state(&handles);
            UI.with(|cell| *cell.borrow_mut() = Some(handles));

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        let launch = UI.with(|cell| {
            let mut guard = cell.borrow_mut();
            let ui = guard.take()?;
            ui.launch_requested.then_some(ui.state)
        });

        if let Some(state) = launch {
            state.apply_env();
            let exe = PathBuf::from(state.exe_path.trim());
            crate::run::run(
                &exe,
                state.unimplemented,
                state.strict,
                true,
                state.console,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_state_round_trips_through_config() {
        let mut cfg = Config {
            game_dir: Some(PathBuf::from("/tmp/victory-game")),
            cache_dir: PathBuf::from("/tmp/victory-cache"),
            data_dir: PathBuf::from("/tmp/victory-data"),
            ..Default::default()
        };
        cfg.gpu.fsr = Some("balanced".into());
        cfg.gpu.vram_budget_mb = Some(6144);
        cfg.gpu.texture_max = Some(512);

        let mut state = LauncherState::from_config(&cfg);
        assert_eq!(state.fsr_mode, FsrMode::Balanced);
        assert_eq!(state.vram_budget_mb, 6144);
        assert_eq!(state.texture_max, 512);

        state.apply_low_vram_preset();
        assert_eq!(state.console, Console::SeriesS);
        assert_eq!(state.fsr_mode, FsrMode::Quality);
        assert_eq!(state.vram_budget_mb, 4096);
        assert_eq!(state.texture_max, 512);
        assert_eq!(state.card_buffers_mb, 256);

        let saved = state.to_config(&cfg);
        assert_eq!(saved.gpu.console.as_deref(), Some("series-s"));
        assert_eq!(saved.gpu.fsr.as_deref(), Some("quality"));
        assert_eq!(saved.gpu.vram_budget_mb, Some(4096));
        assert_eq!(saved.gpu.texture_max, Some(512));
        assert_eq!(saved.gpu.card_buffers_mb, Some(256));

        state.apply_4gb_vram_preset();
        assert_eq!(state.console, Console::SeriesS);
        assert_eq!(state.fsr_mode, FsrMode::Performance);
        assert_eq!(state.vram_budget_mb, 1536);
        assert_eq!(state.texture_max, 128);
        assert_eq!(state.card_buffers_mb, 0);
        assert_eq!(state.staging_budget_mb, 96);
        assert_eq!(state.keep_frames, 2);

        let saved_4gb = state.to_config(&cfg);
        assert_eq!(saved_4gb.gpu.fsr.as_deref(), Some("performance"));
        assert_eq!(saved_4gb.gpu.vram_budget_mb, Some(1536));
        assert_eq!(saved_4gb.gpu.texture_max, Some(128));
        assert_eq!(saved_4gb.gpu.card_buffers_mb, Some(0));
        assert_eq!(saved_4gb.gpu.staging_budget_mb, Some(96));
        assert_eq!(saved_4gb.gpu.keep_frames, Some(2));
    }
}
