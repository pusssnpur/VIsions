//! AMD FidelityFX Super Resolution 1.0 (FSR 1.0: EASU spatial upscaling +
//! RCAS contrast-adaptive sharpening) for the D3D12 presentation pipeline.
//!
//! Configured via `VICTORY_FSR` (`off`, `ultra-quality`, `quality`,
//! `balanced`, `performance`, or a numeric scale factor `1.0..=3.0`),
//! `VICTORY_FSR_SHARPNESS` (`0.0..=2.0` stops, default `0.2`), and optional
//! `VICTORY_FSR_RESOLUTION` (`WxH`, e.g. `2560x1440` or `3840x2160`).

use std::sync::OnceLock;

/// Standard AMD FSR 1.0 quality presets and their spatial scale ratios.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FsrMode {
    /// Disabled (1.0x native presentation).
    Off,
    /// Ultra Quality: 1.3x per-axis scale (77% render resolution).
    UltraQuality,
    /// Quality: 1.5x per-axis scale (67% render resolution).
    Quality,
    /// Balanced: 1.7x per-axis scale (59% render resolution).
    Balanced,
    /// Performance: 2.0x per-axis scale (50% render resolution).
    Performance,
    /// Custom per-axis scale factor (`1.0..=4.0`).
    Custom(f32),
}

impl FsrMode {
    /// Parses a preset name or scale factor from configuration / environment text.
    pub fn parse(text: &str) -> Self {
        let s = text.trim();
        if s.is_empty()
            || s.eq_ignore_ascii_case("off")
            || s.eq_ignore_ascii_case("none")
            || s.eq_ignore_ascii_case("false")
            || s == "0"
        {
            return FsrMode::Off;
        }
        if s.eq_ignore_ascii_case("ultra-quality")
            || s.eq_ignore_ascii_case("ultra_quality")
            || s.eq_ignore_ascii_case("ultraquality")
            || s.eq_ignore_ascii_case("uq")
        {
            return FsrMode::UltraQuality;
        }
        if s.eq_ignore_ascii_case("quality")
            || s.eq_ignore_ascii_case("q")
            || s.eq_ignore_ascii_case("on")
            || s.eq_ignore_ascii_case("true")
            || s == "1"
        {
            return FsrMode::Quality;
        }
        if s.eq_ignore_ascii_case("balanced")
            || s.eq_ignore_ascii_case("bal")
            || s.eq_ignore_ascii_case("b")
        {
            return FsrMode::Balanced;
        }
        if s.eq_ignore_ascii_case("performance")
            || s.eq_ignore_ascii_case("perf")
            || s.eq_ignore_ascii_case("p")
        {
            return FsrMode::Performance;
        }
        if let Ok(scale) = s.parse::<f32>()
            && scale.is_finite()
        {
            if scale <= 1.001 {
                return FsrMode::Off;
            }
            return FsrMode::Custom(scale.clamp(1.0, 4.0));
        }
        FsrMode::Quality
    }

    /// Per-axis spatial upscaling ratio (`out_dim / in_dim`).
    pub fn scale_factor(self) -> f32 {
        match self {
            FsrMode::Off => 1.0,
            FsrMode::UltraQuality => 1.3,
            FsrMode::Quality => 1.5,
            FsrMode::Balanced => 1.7,
            FsrMode::Performance => 2.0,
            FsrMode::Custom(s) => s.clamp(1.0, 4.0),
        }
    }

    /// Short display label for window caption and logs.
    pub fn label(self) -> &'static str {
        match self {
            FsrMode::Off => "Off",
            FsrMode::UltraQuality => "Ultra Quality",
            FsrMode::Quality => "Quality",
            FsrMode::Balanced => "Balanced",
            FsrMode::Performance => "Performance",
            FsrMode::Custom(_) => "Custom",
        }
    }

    /// Canonical config string.
    pub fn as_config_str(self) -> &'static str {
        match self {
            FsrMode::Off => "off",
            FsrMode::UltraQuality => "ultra-quality",
            FsrMode::Quality => "quality",
            FsrMode::Balanced => "balanced",
            FsrMode::Performance => "performance",
            FsrMode::Custom(_) => "quality",
        }
    }

    pub fn is_enabled(self) -> bool {
        !matches!(self, FsrMode::Off)
    }
}

/// Active FSR 1.0 configuration resolved from the environment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FsrConfig {
    pub mode: FsrMode,
    /// RCAS attenuation in stops (`0.0` = maximum sharpness, `2.0` = minimal; default `0.2`).
    pub sharpness_stops: f32,
    /// Optional explicit output resolution `(width, height)`.
    pub target_resolution: Option<(u32, u32)>,
}

impl FsrConfig {
    /// Loads FSR configuration from `VICTORY_FSR`, `VICTORY_FSR_SHARPNESS`,
    /// and `VICTORY_FSR_RESOLUTION`.
    pub fn from_env() -> Self {
        let mode = std::env::var("VICTORY_FSR")
            .ok()
            .map(|v| FsrMode::parse(&v))
            .unwrap_or(FsrMode::Off);
        let sharpness_stops = std::env::var("VICTORY_FSR_SHARPNESS")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.2)
            .clamp(0.0, 2.0);
        let target_resolution = std::env::var("VICTORY_FSR_RESOLUTION")
            .ok()
            .and_then(|v| parse_resolution(&v));
        FsrConfig {
            mode,
            sharpness_stops,
            target_resolution,
        }
    }

    /// Computes the output presentation resolution `(out_width, out_height)`
    /// for a source image of `(in_width, in_height)`.
    pub fn output_dimensions(&self, in_width: u32, in_height: u32) -> (u32, u32) {
        let (in_w, in_h) = (in_width.max(1), in_height.max(1));
        if !self.mode.is_enabled() {
            return (in_w, in_h);
        }
        if let Some((tw, th)) = self.target_resolution
            && tw >= 64
            && th >= 64
        {
            return (tw, th);
        }
        let scale = self.mode.scale_factor();
        // Snap common console internal resolutions to standard desktop targets,
        // and cap automatic scaling at 3840x2160 (4K) so a 1440p/4K source
        // does not allocate an oversized swapchain unless explicitly asked.
        let raw_w = ((in_w as f32 * scale).round() as u32).max(in_w);
        let raw_h = ((in_h as f32 * scale).round() as u32).max(in_h);
        snap_resolution(in_w, in_h, raw_w, raw_h)
    }

    /// Packed root constants `[in_w, in_h, out_w, out_h, rcas_scale, has_over, 0, 0]`
    /// as `u32` words for the EASU and RCAS root signature.
    pub fn constants(
        &self,
        (in_w, in_h): (u32, u32),
        (out_w, out_h): (u32, u32),
        has_over: bool,
    ) -> [u32; 8] {
        // AMD RCAS sharpness factor: `exp2(-stops)`.
        let rcas_linear = (-self.sharpness_stops.clamp(0.0, 2.0)).exp2();
        [
            (in_w.max(1) as f32).to_bits(),
            (in_h.max(1) as f32).to_bits(),
            (out_w.max(1) as f32).to_bits(),
            (out_h.max(1) as f32).to_bits(),
            (1.0f32 / in_w.max(1) as f32).to_bits(),
            (1.0f32 / in_h.max(1) as f32).to_bits(),
            rcas_linear.to_bits(),
            u32::from(has_over),
        ]
    }
}

/// Parses `"WxH"` (e.g. `"2560x1440"` or `"1920X1080"`).
pub fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let s = text.trim();
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    let w = w.trim().parse::<u32>().ok()?;
    let h = h.trim().parse::<u32>().ok()?;
    (w >= 64 && h >= 64 && w <= 16384 && h <= 16384).then_some((w, h))
}

fn snap_resolution(in_w: u32, in_h: u32, raw_w: u32, raw_h: u32) -> (u32, u32) {
    const STANDARDS: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];
    for (sw, sh) in STANDARDS {
        if raw_w.abs_diff(sw) <= 24 && raw_h.abs_diff(sh) <= 24 {
            return (sw, sh);
        }
    }
    if raw_w > 3840 || raw_h > 2160 {
        return (3840.max(in_w), 2160.max(in_h));
    }
    // Keep even dimensions.
    ((raw_w + 1) & !1, (raw_h + 1) & !1)
}

/// Cached process-wide FSR configuration.
pub fn config() -> &'static FsrConfig {
    static CFG: OnceLock<FsrConfig> = OnceLock::new();
    CFG.get_or_init(|| {
        let cfg = FsrConfig::from_env();
        if cfg.mode.is_enabled() {
            tracing::info!(
                mode = cfg.mode.label(),
                scale = cfg.mode.scale_factor(),
                sharpness_stops = cfg.sharpness_stops,
                target = ?cfg.target_resolution,
                "AMD FSR 1.0 (EASU + RCAS) enabled"
            );
        }
        cfg
    })
}

/// Whether FSR 1.0 is active for the current run.
pub fn enabled() -> bool {
    config().mode.is_enabled()
}

/// Caption suffix describing the active FSR preset and resolution scaling.
pub fn caption_tag(in_w: u32, in_h: u32, out_w: u32, out_h: u32) -> String {
    let cfg = config();
    if !cfg.mode.is_enabled() {
        return String::new();
    }
    format!(
        " [FSR {} {in_w}x{in_h}->{out_w}x{out_h}]",
        cfg.mode.label()
    )
}

#[cfg(windows)]
mod d3d12 {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::core::{PCSTR, s};

    use crate::host::{Host, transition};

    /// HLSL implementation of AMD FSR 1.0:
    /// - `ps_easu`: 12-tap Edge-Adaptive Spatial Upsampling (EASU) with gradient
    ///   direction analysis, anisotropic Lanczos2 kernel shaping, and 4-neighbor
    ///   anti-ringing AABB clamping.
    /// - `ps_rcas`: 5-tap Robust Contrast-Adaptive Sharpening (RCAS) on the
    ///   upscaled image, plus optional crisp UI plane compositing (`over`).
    const FSR_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=8, b0, visibility=SHADER_VISIBILITY_PIXEL), DescriptorTable(SRV(t0, numDescriptors=2), visibility=SHADER_VISIBILITY_PIXEL), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"

cbuffer FsrConstants : register(b0) {
    float2 in_size;
    float2 out_size;
    float2 inv_in_size;
    float rcas_sharpness;
    uint has_over;
};

Texture2D<float4> src_tex : register(t0);
Texture2D<float4> over_tex : register(t1);
SamplerState lin_clamp : register(s0);

struct VOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

[RootSignature(RS)]
VOut vs_main(uint id : SV_VertexID) {
    VOut v;
    v.uv = float2((id << 1) & 2, id & 2);
    v.pos = float4(v.uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return v;
}

float fsr_luma(float3 rgb) {
    return dot(rgb, float3(0.299, 0.587, 0.114));
}

// Fast Lanczos2 windowed sinc approximation from AMD FSR 1.0 EASU:
// w(d^2) = (25/16 * (2/5 * d^2 - 1)^2 - 25/16 - 1) * (1/4 * d^2 - 1)^2
float easu_weight(float d2, float lob) {
    d2 = min(d2, 4.0);
    float w_b = (0.4 * d2 - 1.0);
    float w_a = lob * d2 - 1.0;
    w_b *= w_b;
    w_a *= w_a;
    w_b = 1.5625 * w_b - 0.5625;
    return w_b * w_a;
}

void easu_dir(
    inout float2 dir,
    inout float len,
    float w,
    float lA, float lB, float lC, float lD, float lE
) {
    float dc = lD - lC;
    float cb = lC - lB;
    float lenX = max(abs(dc), abs(cb));
    float dirX = lD - lB;
    dir.x += dirX * w;
    if (lenX > 1e-5) {
        lenX = saturate(abs(dirX) / lenX);
        len += lenX * lenX * w;
    }
    float ec = lE - lC;
    float ca = lC - lA;
    float lenY = max(abs(ec), abs(ca));
    float dirY = lE - lA;
    dir.y += dirY * w;
    if (lenY > 1e-5) {
        lenY = saturate(abs(dirY) / lenY);
        len += lenY * lenY * w;
    }
}

void easu_tap(
    inout float3 acc,
    inout float w_sum,
    float2 off,
    float2 dir,
    float2 stretch,
    float lob,
    float3 color
) {
    float2 v = float2(
        off.x * dir.x + off.y * dir.y,
        off.x * (-dir.y) + off.y * dir.x
    );
    v *= stretch;
    float d2 = dot(v, v);
    float w = easu_weight(d2, lob);
    acc += color * w;
    w_sum += w;
}

[RootSignature(RS)]
float4 ps_easu(VOut v) : SV_Target {
    // Map output pixel center to input texel coordinates.
    float2 pp = v.uv * in_size - 0.5;
    float2 fp = floor(pp);
    float2 f = pp - fp;
    int2 base = int2(fp);
    int2 max_c = int2(in_size) - 1;

    #define LOAD_IN(dx, dy) src_tex.Load(int3(clamp(base + int2(dx, dy), int2(0, 0), max_c), 0)).rgb

    // 12-tap neighborhood:
    //       b c
    //     e f g h
    //     i j k l
    //       n o
    float3 b = LOAD_IN( 0, -1);
    float3 c = LOAD_IN( 1, -1);
    float3 e = LOAD_IN(-1,  0);
    float3 f_c = LOAD_IN( 0,  0);
    float3 g = LOAD_IN( 1,  0);
    float3 h = LOAD_IN( 2,  0);
    float3 i = LOAD_IN(-1,  1);
    float3 j = LOAD_IN( 0,  1);
    float3 k = LOAD_IN( 1,  1);
    float3 l = LOAD_IN( 2,  1);
    float3 n = LOAD_IN( 0,  2);
    float3 o = LOAD_IN( 1,  2);

    #undef LOAD_IN

    float lb = fsr_luma(b), lc = fsr_luma(c);
    float le = fsr_luma(e), lf = fsr_luma(f_c), lg = fsr_luma(g), lh = fsr_luma(h);
    float li = fsr_luma(i), lj = fsr_luma(j), lk = fsr_luma(k), ll = fsr_luma(l);
    float ln = fsr_luma(n), lo = fsr_luma(o);

    // Bilinearly blend directional gradients across the 4 inner quad corners.
    float2 dir = float2(0.0, 0.0);
    float len = 0.0;
    easu_dir(dir, len, (1.0 - f.x) * (1.0 - f.y), lb, le, lf, lg, lj);
    easu_dir(dir, len, f.x * (1.0 - f.y),         lc, lf, lg, lh, lk);
    easu_dir(dir, len, (1.0 - f.x) * f.y,         lf, li, lj, lk, ln);
    easu_dir(dir, len, f.x * f.y,                 lg, lj, lk, ll, lo);

    float dir_r = dot(dir, dir);
    if (dir_r < (1.0 / 32768.0)) {
        dir = float2(1.0, 0.0);
    } else {
        dir *= rsqrt(dir_r);
    }

    len = len * 0.5;
    len *= len;
    float stretch_factor = dot(dir, dir) / max(abs(dir.x), abs(dir.y));
    float2 len2 = float2(1.0 + (stretch_factor - 1.0) * len, 1.0 - 0.5 * len);
    float lob = 0.5 - 0.29 * len;

    // Anti-ringing AABB from the 4 nearest taps (f, g, j, k).
    float3 min4 = min(min(f_c, g), min(j, k));
    float3 max4 = max(max(f_c, g), max(j, k));

    float3 acc = float3(0.0, 0.0, 0.0);
    float w_sum = 0.0;
    easu_tap(acc, w_sum, float2( 0.0, -1.0) - f, dir, len2, lob, b);
    easu_tap(acc, w_sum, float2( 1.0, -1.0) - f, dir, len2, lob, c);
    easu_tap(acc, w_sum, float2(-1.0,  0.0) - f, dir, len2, lob, e);
    easu_tap(acc, w_sum, float2( 0.0,  0.0) - f, dir, len2, lob, f_c);
    easu_tap(acc, w_sum, float2( 1.0,  0.0) - f, dir, len2, lob, g);
    easu_tap(acc, w_sum, float2( 2.0,  0.0) - f, dir, len2, lob, h);
    easu_tap(acc, w_sum, float2(-1.0,  1.0) - f, dir, len2, lob, i);
    easu_tap(acc, w_sum, float2( 0.0,  1.0) - f, dir, len2, lob, j);
    easu_tap(acc, w_sum, float2( 1.0,  1.0) - f, dir, len2, lob, k);
    easu_tap(acc, w_sum, float2( 2.0,  1.0) - f, dir, len2, lob, l);
    easu_tap(acc, w_sum, float2( 0.0,  2.0) - f, dir, len2, lob, n);
    easu_tap(acc, w_sum, float2( 1.0,  2.0) - f, dir, len2, lob, o);

    float3 pix = clamp(acc / max(w_sum, 1e-5), min4, max4);
    return float4(saturate(pix), 1.0);
}

[RootSignature(RS)]
float4 ps_rcas(VOut v) : SV_Target {
    int2 sp = int2(v.pos.xy);
    int2 max_c = int2(out_size) - 1;

    // 5-tap cross on the EASU-upscaled texture:
    //     b
    //   d e f
    //     h
    float3 b = src_tex.Load(int3(clamp(sp + int2( 0, -1), int2(0, 0), max_c), 0)).rgb;
    float3 d = src_tex.Load(int3(clamp(sp + int2(-1,  0), int2(0, 0), max_c), 0)).rgb;
    float3 e = src_tex.Load(int3(clamp(sp,                int2(0, 0), max_c), 0)).rgb;
    float3 f = src_tex.Load(int3(clamp(sp + int2( 1,  0), int2(0, 0), max_c), 0)).rgb;
    float3 h = src_tex.Load(int3(clamp(sp + int2( 0,  1), int2(0, 0), max_c), 0)).rgb;

    float lb = fsr_luma(b);
    float ld = fsr_luma(d);
    float le = fsr_luma(e);
    float lf = fsr_luma(f);
    float lh = fsr_luma(h);

    // Local min and max of the 4 neighbors.
    float mn4 = min(min(lb, ld), min(lf, lh));
    float mx4 = max(max(lb, ld), max(lf, lh));

    // AMD RCAS lobe limit so sharpening never clips [0, 1].
    float hit_min = min(mn4, le) / (4.0 * max(mx4, 1e-4));
    float hit_max = (1.0 - max(mx4, le)) / (4.0 * min(mn4, 1.0 - 1e-4) - 4.0);
    float lobe = max(-hit_min, hit_max);
    lobe = max(-0.1875, min(0.0, lobe)) * rcas_sharpness;

    float3 scene = saturate((lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0));
    if (has_over != 0u) {
        float4 o = over_tex.SampleLevel(lin_clamp, v.uv, 0);
        scene = saturate(o.rgb + scene * (1.0 - saturate(o.a)));
    }
    return float4(scene, 1.0);
}
"#;

    const FRAMES: u32 = 16;

    struct FsrPipeline {
        root: ID3D12RootSignature,
        pso_easu: ID3D12PipelineState,
        pso_rcas: ID3D12PipelineState,
        views: ID3D12DescriptorHeap,
        step: u32,
        next: AtomicU32,
        intermediate: Option<(ID3D12Resource, u32, u32, DXGI_FORMAT)>,
    }

    // SAFETY: D3D12 objects are free-threaded; mutable access is synchronized behind `PIPELINE`.
    unsafe impl Send for FsrPipeline {}
    // SAFETY: as above.
    unsafe impl Sync for FsrPipeline {}

    fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
        let mut code: Option<ID3DBlob> = None;
        // SAFETY: static source string and entry/target names live through the call.
        unsafe {
            D3DCompile(
                FSR_HLSL.as_ptr().cast(),
                FSR_HLSL.len(),
                s!("fsr"),
                None,
                None,
                entry,
                target,
                0,
                0,
                &mut code,
                None,
            )?;
        }
        code.ok_or_else(windows::core::Error::empty)
    }

    fn make_pipeline(host: &Host, format: DXGI_FORMAT) -> windows::core::Result<FsrPipeline> {
        let vs = compile(s!("vs_main"), s!("vs_5_1"))?;
        let ps_easu = compile(s!("ps_easu"), s!("ps_5_1"))?;
        let ps_rcas = compile(s!("ps_rcas"), s!("ps_5_1"))?;
        // SAFETY: blobs live through the root signature and PSO creations.
        unsafe {
            let bytes = |b: &ID3DBlob| {
                std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize())
            };
            let device = &host.device;
            let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes(&vs))?;
            let code = |b: &ID3DBlob| D3D12_SHADER_BYTECODE {
                pShaderBytecode: b.GetBufferPointer(),
                BytecodeLength: b.GetBufferSize(),
            };
            let make_pso = |ps: &ID3DBlob| -> windows::core::Result<ID3D12PipelineState> {
                let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    VS: code(&vs),
                    PS: code(ps),
                    SampleMask: u32::MAX,
                    RasterizerState: D3D12_RASTERIZER_DESC {
                        FillMode: D3D12_FILL_MODE_SOLID,
                        CullMode: D3D12_CULL_MODE_NONE,
                        ..Default::default()
                    },
                    PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                    NumRenderTargets: 1,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    ..Default::default()
                };
                desc.RTVFormats[0] = format;
                desc.BlendState.RenderTarget[0].RenderTargetWriteMask =
                    D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
                device.CreateGraphicsPipelineState(&desc)
            };
            let pso_easu = make_pso(&ps_easu)?;
            let pso_rcas = make_pso(&ps_rcas)?;
            // 4 descriptors per frame (2 for EASU pass, 2 for RCAS+overlay pass).
            let views = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 4 * FRAMES,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })?;
            Ok(FsrPipeline {
                root,
                pso_easu,
                pso_rcas,
                views,
                step: device
                    .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
                next: AtomicU32::new(0),
                intermediate: None,
            })
        }
    }

    fn ensure_intermediate(
        host: &Host,
        pipe: &mut FsrPipeline,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Option<ID3D12Resource> {
        if let Some((tex, w, h, f)) = &pipe.intermediate
            && (*w, *h, *f) == (width, height, format)
        {
            return Some(tex.clone());
        }
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: u64::from(width.max(1)),
            Height: height.max(1),
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ..Default::default()
        };
        let mut tex: Option<ID3D12Resource> = None;
        // SAFETY: valid heap and resource descriptions.
        unsafe {
            host.device
                .CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                    None,
                    &mut tex,
                )
                .ok()?;
        }
        let tex = tex?;
        if let Some((old, ..)) = pipe.intermediate.replace((tex.clone(), width, height, format)) {
            crate::host::retire(old);
        }
        Some(tex)
    }

    /// Records the two-pass FSR 1.0 pipeline (EASU spatial upscale followed by
    /// RCAS contrast-adaptive sharpening and optional UI overlay composition)
    /// into `back` (`D3D12_RESOURCE_STATE_RENDER_TARGET`).
    ///
    /// Both `scene` and `over` (when present) must be in
    /// `D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE`.
    pub(crate) fn record(
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        scene: (&ID3D12Resource, DXGI_FORMAT),
        over: Option<(&ID3D12Resource, DXGI_FORMAT)>,
        back: &ID3D12Resource,
        (out_width, out_height, format): (u32, u32, DXGI_FORMAT),
    ) -> bool {
        static PIPELINE: Mutex<Option<(DXGI_FORMAT, Option<FsrPipeline>)>> = Mutex::new(None);
        let mut guard = PIPELINE.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_none_or(|p| p.0 != format) {
            let made = make_pipeline(host, format);
            if let Err(e) = &made {
                tracing::warn!(error = %e, "FSR 1.0 pipeline creation failed");
            }
            *guard = Some((format, made.ok()));
        }
        let Some((_, Some(p))) = &mut *guard else {
            return false;
        };
        let Some(easu_tex) = ensure_intermediate(host, p, out_width, out_height, format) else {
            return false;
        };
        // SAFETY: `scene.0` is a live texture.
        let scene_desc = unsafe { scene.0.GetDesc() };
        let (in_width, in_height) = (scene_desc.Width as u32, scene_desc.Height);
        let consts = super::config().constants(
            (in_width, in_height),
            (out_width, out_height),
            over.is_some(),
        );
        let base_slot = 4 * (p.next.fetch_add(1, Ordering::Relaxed) % FRAMES);
        // SAFETY: `list` is open; `easu_tex`, `scene`, `over`, and `back` are live resources.
        unsafe {
            let (cpu, gpu) = (
                p.views.GetCPUDescriptorHandleForHeapStart(),
                p.views.GetGPUDescriptorHandleForHeapStart(),
            );
            let write_srv = |slot: u32, res: Option<(&ID3D12Resource, DXGI_FORMAT)>| {
                let at = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu.ptr + (slot as usize * p.step as usize),
                };
                let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: res.map_or(format, |r| r.1),
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_SRV {
                            MipLevels: 1,
                            ..Default::default()
                        },
                    },
                };
                host.device
                    .CreateShaderResourceView(res.map(|r| r.0), Some(&desc), at);
            };
            // Pass 1 SRVs: t0 = scene, t1 = null
            write_srv(base_slot, Some(scene));
            write_srv(base_slot + 1, None);
            // Pass 2 SRVs: t0 = easu_tex, t1 = over
            write_srv(base_slot + 2, Some((&easu_tex, format)));
            write_srv(base_slot + 3, over);

            let viewport = D3D12_VIEWPORT {
                Width: out_width as f32,
                Height: out_height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            };
            let scissor = windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: out_width as i32,
                bottom: out_height as i32,
            };

            list.SetDescriptorHeaps(&[Some(p.views.clone())]);
            list.SetGraphicsRootSignature(&p.root);
            list.SetGraphicsRoot32BitConstants(0, 8, consts.as_ptr().cast(), 0);
            list.IASetPrimitiveTopology(
                windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
            );
            list.RSSetViewports(&[viewport]);
            list.RSSetScissorRects(&[scissor]);

            // Pass 1: EASU into `easu_tex`
            list.ResourceBarrier(&[transition(
                &easu_tex,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            )]);
            let easu_rtv = host.rtv(&easu_tex, None);
            list.OMSetRenderTargets(1, Some(&easu_rtv), false, None);
            list.SetPipelineState(&p.pso_easu);
            list.SetGraphicsRootDescriptorTable(
                1,
                D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu.ptr + u64::from(base_slot) * u64::from(p.step),
                },
            );
            list.DrawInstanced(3, 1, 0, 0);

            // Transition `easu_tex` back to SRV for RCAS
            list.ResourceBarrier(&[transition(
                &easu_tex,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            )]);

            // Pass 2: RCAS + optional UI overlay into `back`
            let back_rtv = host.rtv(back, None);
            list.OMSetRenderTargets(1, Some(&back_rtv), false, None);
            list.SetPipelineState(&p.pso_rcas);
            list.SetGraphicsRootDescriptorTable(
                1,
                D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu.ptr + u64::from(base_slot + 2) * u64::from(p.step),
                },
            );
            list.DrawInstanced(3, 1, 0, 0);
        }
        true
    }
}

#[cfg(windows)]
pub(crate) use d3d12::record;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fsr_modes_and_scales() {
        assert_eq!(FsrMode::parse("off"), FsrMode::Off);
        assert_eq!(FsrMode::parse("0"), FsrMode::Off);
        assert_eq!(FsrMode::parse("ultra-quality"), FsrMode::UltraQuality);
        assert_eq!(FsrMode::parse("uq"), FsrMode::UltraQuality);
        assert_eq!(FsrMode::parse("quality"), FsrMode::Quality);
        assert_eq!(FsrMode::parse("1"), FsrMode::Quality);
        assert_eq!(FsrMode::parse("balanced"), FsrMode::Balanced);
        assert_eq!(FsrMode::parse("performance"), FsrMode::Performance);
        assert_eq!(FsrMode::parse("1.6"), FsrMode::Custom(1.6));

        let cfg = FsrConfig {
            mode: FsrMode::Quality,
            sharpness_stops: 0.2,
            target_resolution: None,
        };
        assert_eq!(cfg.output_dimensions(1280, 720), (1920, 1080));
        assert_eq!(cfg.output_dimensions(1706, 960), (2560, 1440));
        assert_eq!(cfg.output_dimensions(2560, 1440), (3840, 2160));
    }

    #[test]
    fn explicit_target_resolution_overrides_auto_scale() {
        assert_eq!(parse_resolution("2560x1440"), Some((2560, 1440)));
        assert_eq!(parse_resolution("3840X2160"), Some((3840, 2160)));
        assert_eq!(parse_resolution("invalid"), None);

        let cfg = FsrConfig {
            mode: FsrMode::Balanced,
            sharpness_stops: 0.2,
            target_resolution: Some((2560, 1440)),
        };
        assert_eq!(cfg.output_dimensions(1280, 720), (2560, 1440));
    }
}
