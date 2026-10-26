//! 빠른 변환 (알씨 우클릭 메뉴): 회전, 크기, 형식, 용량, 화질, 공유용, 워터마크.
//! - 회전만 하면: JPEG는 회전 값만 바꿔 무손실·즉시, HEIC·TIFF는 맥 OS가 무손실 복사
//! - 그 밖에는 다시 저장: 맥은 OS(ImageIO) 경로로 색 공간과 촬영 정보를 보존하고,
//!   WebP와 윈도우는 Rust 경로(zune-jpeg, jpeg-encoder, libwebp)
//! - 여러 장은 코어 여러 개로 동시에 처리 (한 장에 수백 MB를 쓸 수 있어 동시 4장까지)

use crate::decode::{self, Pixels, generic};
use crate::formats;
use crate::fsops::unique_in;
use crate::jpegmeta;
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Turn {
    Cw90,
    Ccw90,
    R180,
    FlipH,
    FlipV,
}

impl Turn {
    /// 같은 변환을 뜻하는 EXIF 회전 값
    pub fn orientation(self) -> u16 {
        match self {
            Turn::Cw90 => 6,
            Turn::Ccw90 => 8,
            Turn::R180 => 3,
            Turn::FlipH => 2,
            Turn::FlipV => 4,
        }
    }
}

/// EXIF 회전 값 o(1~8)를 2×2 행렬로 (화면 좌표 x→오른쪽, y→아래, 중심 기준)
fn mat(o: u16) -> [i8; 4] {
    match o {
        2 => [-1, 0, 0, 1],
        3 => [-1, 0, 0, -1],
        4 => [1, 0, 0, -1],
        5 => [0, 1, 1, 0],
        6 => [0, -1, 1, 0],
        7 => [0, -1, -1, 0],
        8 => [0, 1, -1, 0],
        _ => [1, 0, 0, 1],
    }
}

/// 지금 회전 값 o로 보이는 사진에 변환 u를 더 했을 때의 새 회전 값
pub fn compose(o: u16, u: u16) -> u16 {
    let (a, b) = (mat(u), mat(o));
    let m = [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
    ];
    (1..=8).find(|&k| mat(k) == m).unwrap_or(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutFormat {
    Jpeg,
    Png,
    Webp,
    Heic,
    Avif,
    Tiff,
    Bmp,
}

impl OutFormat {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Heic => "heic",
            Self::Avif => "avif",
            Self::Tiff => "tif",
            Self::Bmp => "bmp",
        }
    }

    #[cfg(target_os = "macos")]
    fn uti(self) -> Option<&'static str> {
        Some(match self {
            Self::Jpeg => "public.jpeg",
            Self::Heic => "public.heic",
            Self::Avif => "public.avif",
            Self::Tiff => "public.tiff",
            Self::Bmp => "com.microsoft.bmp",
            // PNG는 OS가 Rust 빠른 압축보다 3배 느리지만(24MP 1.2초) 크기가 원본과 같다 (빠른 압축은 +38%)
            Self::Png => "public.png",
            Self::Webp => return None, // OS가 WebP를 쓰지 못함 → Rust 경로
        })
    }

    pub fn lossy(self) -> bool {
        matches!(self, Self::Jpeg | Self::Webp | Self::Heic | Self::Avif)
    }

    /// 이 컴퓨터에서 저장할 수 있는 형식인지
    pub fn available(self) -> bool {
        cfg!(target_os = "macos") || !matches!(self, Self::Heic | Self::Avif)
    }

    pub fn of_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "jpg" | "jpeg" | "jpe" | "jfif" => Self::Jpeg,
            "png" => Self::Png,
            "webp" => Self::Webp,
            "heic" | "heif" => Self::Heic,
            "avif" => Self::Avif,
            "tif" | "tiff" => Self::Tiff,
            "bmp" => Self::Bmp,
            _ => return None,
        })
    }

    /// 형식을 고르지 않았을 때의 기본 화질
    fn default_quality(self) -> u8 {
        match self {
            Self::Jpeg => 90,
            Self::Webp => 85,
            Self::Heic => 80,
            Self::Avif => 70,
            _ => 100,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Resize {
    /// 원본의 몇 %
    Percent(u32),
    /// 긴 변 픽셀 수 (원본보다 크게 키우지는 않음)
    LongSide(u32),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "mode", content = "path", rename_all = "snake_case")]
pub enum SaveMode {
    /// 같은 폴더에 새 이름으로 (원본 보존)
    #[default]
    NewFile,
    /// 원본 자리에 (원본은 되돌리기용으로 따로 보관)
    Overwrite,
    /// 지정한 폴더에 같은 이름으로
    Folder(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Watermark {
    /// 화면에서 그린 글자 이미지 (투명 PNG)
    pub png: Vec<u8>,
    /// 위치 0~8 (왼쪽 위부터 가로로)
    pub anchor: u8,
    /// 사진 가로 대비 워터마크 가로 비율 (0~1)
    pub width_ratio: f32,
    /// 가장자리 여백 (사진 짧은 변 대비)
    pub margin_ratio: f32,
    pub opacity: f32,
}

impl Watermark {
    /// 사진(w×h)에 놓을 자리 (x, y, 가로, 세로), 위에서부터 잰 좌표
    fn place(&self, w: u32, h: u32, ow: u32, oh: u32) -> (f64, f64, f64, f64) {
        let (w, h) = (w as f64, h as f64);
        let dw = w * self.width_ratio.clamp(0.02, 1.0) as f64;
        let dh = dw * oh as f64 / ow.max(1) as f64;
        let m = w.min(h) * self.margin_ratio.clamp(0.0, 0.3) as f64;
        let x = match self.anchor % 3 {
            0 => m,
            1 => (w - dw) / 2.0,
            _ => w - dw - m,
        };
        let y = match self.anchor / 3 {
            0 => m,
            1 => (h - dh) / 2.0,
            _ => h - dh - m,
        };
        (x, y, dw, dh)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Spec {
    pub turn: Option<Turn>,
    pub resize: Option<Resize>,
    pub format: Option<OutFormat>,
    /// 1~100
    pub quality: Option<u8>,
    /// 이 용량(바이트) 이하가 되도록 화질을 자동으로 찾는다
    pub max_bytes: Option<u64>,
    pub strip_gps: bool,
    pub watermark: Option<Watermark>,
    pub save: SaveMode,
    /// 새 파일 이름 꼬리 (없으면 작업 내용으로 자동: _1920px, _1MB …)
    pub suffix: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Done {
    pub source: String,
    pub output: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    /// 화질 손실 없이 처리했는지 (회전 값만 바꿈)
    pub lossless: bool,
    /// 덮어쓰기 전 원본을 보관한 곳 (되돌리기용)
    pub backup: Option<String>,
    /// 형식이 바뀌어 원본 파일이 없어졌는지 (덮어쓰기 + 다른 확장자)
    pub replaced_source: bool,
    /// 용량 맞추기에서 고른 화질
    pub quality: Option<u8>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Failed {
    pub path: String,
    pub error: String,
}

fn human_bytes(b: u64) -> String {
    if b >= 1_000_000 && b % 100_000 == 0 {
        format!("{}MB", b as f64 / 1e6)
    } else {
        format!("{}KB", b / 1000)
    }
}

/// 새 파일 이름 꼬리: 작업 내용이 이름만 봐도 보이게
fn auto_suffix(spec: &Spec) -> String {
    if let Some(s) = &spec.suffix {
        return s.clone();
    }
    let mut parts = Vec::new();
    if spec.turn.is_some() {
        parts.push("회전".to_string());
    }
    match &spec.resize {
        Some(Resize::Percent(p)) => parts.push(format!("{p}%")),
        Some(Resize::LongSide(v)) => parts.push(format!("{v}px")),
        None => {}
    }
    if let Some(b) = spec.max_bytes {
        parts.push(human_bytes(b));
    }
    if let Some(q) = spec.quality {
        parts.push(format!("q{q}"));
    }
    if spec.watermark.is_some() {
        parts.push("워터마크".into());
    }
    if spec.strip_gps && parts.is_empty() {
        parts.push("위치삭제".into());
    }
    parts.join("_")
}

fn file_stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// 저장할 경로. 형식이 그대로면 원래 확장자 표기(JPG, jpeg 등)를 유지한다.
fn output_path(src: &Path, ext: &str, spec: &Spec) -> anyhow::Result<PathBuf> {
    let dir = src.parent().context("상위 폴더 없음")?;
    let stem = file_stem(src);
    Ok(match &spec.save {
        SaveMode::Overwrite => {
            let p = dir.join(format!("{stem}.{ext}"));
            if p != src && p.exists() { unique_in(dir, &format!("{stem}.{ext}")) } else { p }
        }
        SaveMode::NewFile => {
            let suffix = auto_suffix(spec);
            let name = if suffix.is_empty() { format!("{stem}.{ext}") } else { format!("{stem}_{suffix}.{ext}") };
            unique_in(dir, &name)
        }
        SaveMode::Folder(folder) => {
            let folder = Path::new(folder);
            if !folder.is_dir() {
                bail!("저장할 폴더가 없음: {}", folder.display());
            }
            unique_in(folder, &format!("{stem}.{ext}"))
        }
    })
}

/// 같은 폴더의 임시 파일에 다 쓴 다음 이름을 바꾼다 (중간에 끊겨도 원본이 망가지지 않게).
fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().context("상위 폴더 없음")?;
    let tmp = dir.join(format!(
        ".{}.isee-{}.tmp",
        path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// 결과를 저장한다. 덮어쓰기면 원본을 먼저 backup_dir에 보관한다.
fn save(src: &Path, ext: &str, spec: &Spec, backup_dir: Option<&Path>, write: impl FnOnce(&Path) -> anyhow::Result<()>) -> anyhow::Result<(PathBuf, Option<String>, bool)> {
    let out = output_path(src, ext, spec)?;
    let mut backup = None;
    if matches!(spec.save, SaveMode::Overwrite) {
        let dir = backup_dir.context("되돌리기용 보관 폴더가 없음")?;
        std::fs::create_dir_all(dir)?;
        let b = unique_in(dir, &src.file_name().unwrap_or_default().to_string_lossy());
        // 맥 APFS는 복제(clone)라서 즉시 끝나고 공간도 거의 쓰지 않는다
        std::fs::copy(src, &b)?;
        backup = Some(b.to_string_lossy().into_owned());
    }
    write(&out)?;
    let replaced = matches!(spec.save, SaveMode::Overwrite) && out != src;
    if replaced {
        std::fs::remove_file(src)?;
    }
    Ok((out, backup, replaced))
}

fn only_turn(spec: &Spec, src_fmt: Option<OutFormat>) -> bool {
    spec.turn.is_some()
        && spec.resize.is_none()
        && spec.quality.is_none()
        && spec.max_bytes.is_none()
        && spec.watermark.is_none()
        && !spec.strip_gps
        && (spec.format.is_none() || spec.format == src_fmt)
}

/// 편집기에서 그린 결과(원본 해상도, 회전 적용됨)를 저장한다. 크기·형식·화질·용량·저장 방식은 변환과 같다.
/// 원본의 촬영 정보(날짜·위치·카메라)는 이어받는다.
pub fn run_pixels(src: &Path, px: Pixels, spec: &Spec, backup_dir: Option<&Path>) -> anyhow::Result<Done> {
    let mut spec = spec.clone();
    spec.turn = None;
    reencode(src, &spec, backup_dir, Some(px))
}

/// 사진 한 장 변환
pub fn run_one(src: &Path, spec: &Spec, backup_dir: Option<&Path>) -> anyhow::Result<Done> {
    let src_fmt = OutFormat::of_path(src);
    let ext_same = src.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();

    // 1) 무손실 회전
    if only_turn(spec, src_fmt) {
        let u = spec.turn.unwrap().orientation();
        if src_fmt == Some(OutFormat::Jpeg) {
            let bytes = std::fs::read(src)?;
            let o = compose(jpegmeta::orientation(&bytes), u);
            let out_bytes = jpegmeta::set_orientation(&bytes, o)?;
            let (out, backup, replaced) = save(src, &ext_same, spec, backup_dir, |p| write_atomic(p, &out_bytes))?;
            let (width, height) = decode::dimensions(&out).unwrap_or((0, 0));
            return Ok(Done {
                source: src.to_string_lossy().into_owned(),
                output: out.to_string_lossy().into_owned(),
                bytes: out_bytes.len() as u64,
                width,
                height,
                lossless: true,
                backup,
                replaced_source: replaced,
                quality: None,
            });
        }
        #[cfg(target_os = "macos")]
        if matches!(src_fmt, Some(OutFormat::Heic) | Some(OutFormat::Tiff)) {
            let o = compose(decode::imageio::raw_orientation(src), u);
            let attempt = save(src, &ext_same, spec, backup_dir, |p| {
                let tmp = p.with_extension(format!("isee-{}.{}", std::process::id(), ext_same));
                decode::imageio::copy_with_orientation(src, &tmp, o)?;
                std::fs::rename(&tmp, p)?;
                Ok(())
            });
            if let Ok((out, backup, replaced)) = attempt {
                let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                let (width, height) = decode::dimensions(&out).unwrap_or((0, 0));
                return Ok(Done {
                    source: src.to_string_lossy().into_owned(),
                    output: out.to_string_lossy().into_owned(),
                    bytes,
                    width,
                    height,
                    lossless: true,
                    backup,
                    replaced_source: replaced,
                    quality: None,
                });
            }
        }
    }

    reencode(src, spec, backup_dir, None)
}

/// 디코딩(또는 받은 픽셀) → 크기 → 형식·화질(·용량 맞추기) → 저장
fn reencode(src: &Path, spec: &Spec, backup_dir: Option<&Path>, pixels: Option<Pixels>) -> anyhow::Result<Done> {
    let src_fmt = OutFormat::of_path(src);
    let ext_same = src.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let mut fmt = spec
        .format
        .or(src_fmt.filter(|f| f.available()))
        .unwrap_or(OutFormat::Jpeg);
    // 용량을 맞추려면 손실 압축 형식이어야 한다 (PNG·TIFF·BMP 원본은 JPEG로)
    if spec.max_bytes.is_some() && !fmt.lossy() && spec.format.is_none() {
        fmt = OutFormat::Jpeg;
    }
    if !fmt.available() {
        bail!("이 컴퓨터에서는 {} 형식으로 저장할 수 없습니다", fmt.ext().to_uppercase());
    }
    let (fw, fh) = match &pixels {
        Some(p) => (p.width, p.height),
        None => decode::dimensions(src).context("사진 크기를 읽을 수 없음")?,
    };
    let long = fw.max(fh);
    let mut target = match spec.resize {
        Some(Resize::Percent(p)) => ((long as u64 * p.clamp(1, 100) as u64) / 100) as u32,
        Some(Resize::LongSide(v)) => v.min(long),
        None => long,
    }
    .max(1);
    let turn = spec.turn.map(|t| t.orientation()).unwrap_or(1);
    let quality = spec.quality.unwrap_or(fmt.default_quality());
    let ext = if Some(fmt) == src_fmt { ext_same.clone() } else { fmt.ext().to_string() };

    // 용량 맞추기: 화질을 이분 탐색하고, 가장 낮은 화질로도 크면 크기를 줄여 다시
    let mut encoder = Encoder::new(src, fmt, turn, spec, pixels)?;
    let (bytes, width, height, used_q) = match spec.max_bytes {
        None => {
            let (b, w, h) = encoder.encode(target, quality)?;
            (b, w, h, None)
        }
        Some(max) => {
            let mut result = None;
            for _ in 0..4 {
                let (mut lo, mut hi) = (20u8, 95u8);
                let mut best = None;
                let mut smallest = u64::MAX;
                while lo <= hi {
                    let q = (lo + hi) / 2;
                    let (b, w, h) = encoder.encode(target, q)?;
                    smallest = smallest.min(b.len() as u64);
                    if b.len() as u64 <= max {
                        best = Some((b, w, h, Some(q)));
                        lo = q + 1;
                    } else {
                        hi = q - 1;
                    }
                }
                if best.is_some() {
                    result = best;
                    break;
                }
                // 화질 20으로도 크다 → 크기를 용량 비율만큼 줄인다
                let ratio = (max as f64 / smallest as f64).sqrt() * 0.92;
                target = ((target as f64) * ratio).max(64.0) as u32;
            }
            result.context("목표 용량까지 줄이지 못했습니다")?
        }
    };
    let (out, backup, replaced) = save(src, &ext, spec, backup_dir, |p| write_atomic(p, &bytes))?;
    Ok(Done {
        source: src.to_string_lossy().into_owned(),
        output: out.to_string_lossy().into_owned(),
        bytes: bytes.len() as u64,
        width,
        height,
        lossless: false,
        backup,
        replaced_source: replaced,
        quality: used_q,
    })
}

/// 같은 사진을 여러 화질·크기로 저장해 볼 때 디코딩을 반복하지 않도록 준비 결과를 들고 있는다.
enum Encoder<'a> {
    #[cfg(target_os = "macos")]
    Os {
        src: &'a Path,
        spec: &'a Spec,
        uti: &'static str,
        turn: u16,
        /// 편집기에서 받은 픽셀 (없으면 파일에서 읽음)
        pixels: Option<Pixels>,
        prepared: Option<(u32, decode::imageio::Prepared)>,
    },
    Rust {
        full: Pixels,
        icc: Option<Vec<u8>>,
        exif: Option<Vec<u8>>,
        fmt: OutFormat,
        spec: &'a Spec,
        cached: Option<(u32, Pixels)>,
    },
}

impl<'a> Encoder<'a> {
    fn new(src: &'a Path, fmt: OutFormat, turn: u16, spec: &'a Spec, pixels: Option<Pixels>) -> anyhow::Result<Self> {
        #[cfg(target_os = "macos")]
        if let Some(uti) = fmt.uti() {
            return Ok(Self::Os { src, spec, uti, turn, pixels, prepared: None });
        }
        if let Some(full) = pixels {
            // 편집 결과: 원본의 EXIF(촬영 날짜·위치)를 JPEG에 이어 붙인다
            let exif = (fmt == OutFormat::Jpeg).then(|| exif_of(src)).flatten();
            return Ok(Self::Rust { full, icc: None, exif, fmt, spec, cached: None });
        }
        // Rust 경로: 원본 해상도로 한 번 디코딩 (맥에서 Rust가 못 읽는 HEIC 등은 OS로 읽음)
        let (full, icc) = match generic::decode_for_convert(src) {
            Ok(x) => x,
            #[cfg(target_os = "macos")]
            Err(_) => (decode::imageio::screen_rgba(src, 0)?, None),
            #[cfg(not(target_os = "macos"))]
            Err(e) => return Err(e),
        };
        let (rgba, w, h) = generic::orient_rgba(full.rgba, full.width, full.height, turn as u32);
        let exif = if fmt == OutFormat::Jpeg && formats::is_jpeg(src) {
            std::fs::read(src).ok().and_then(|b| jpegmeta::exif_tiff(&b).map(|t| t.to_vec()))
        } else {
            None
        };
        Ok(Self::Rust {
            full: Pixels { rgba, width: w, height: h },
            icc,
            exif,
            fmt,
            spec,
            cached: None,
        })
    }

    /// 긴 변 target 픽셀, 화질 q(1~100)로 저장한 바이트와 가로·세로
    fn encode(&mut self, target: u32, q: u8) -> anyhow::Result<(Vec<u8>, u32, u32)> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Os { src, spec, uti, turn, pixels: Some(full), prepared } => {
                if prepared.as_ref().map(|p| p.0) != Some(target) {
                    let copy = Pixels { rgba: full.rgba.clone(), width: full.width, height: full.height };
                    let px = generic::resize_quality(copy, target)?;
                    let _ = turn;
                    *prepared = Some((target, decode::imageio::prepare_pixels(src, &px)?));
                }
                let p = &prepared.as_ref().unwrap().1;
                let bytes = decode::imageio::encode_prepared(p, uti, q as f64 / 100.0, spec.strip_gps)?;
                Ok((bytes, p.width, p.height))
            }
            #[cfg(target_os = "macos")]
            Self::Os { src, spec, uti, turn, pixels: None, prepared } => {
                if prepared.as_ref().map(|p| p.0) != Some(target) {
                    let place = |w: u32, h: u32, ow: u32, oh: u32| {
                        spec.watermark.as_ref().map(|wm| wm.place(w, h, ow, oh)).unwrap_or_default()
                    };
                    let overlay = spec.watermark.as_ref().map(|wm| decode::imageio::Overlay {
                        png: &wm.png,
                        place: &place,
                        opacity: wm.opacity as f64,
                    });
                    *prepared = Some((target, decode::imageio::prepare(src, target, *turn, overlay.as_ref())?));
                }
                let p = &prepared.as_ref().unwrap().1;
                let bytes = decode::imageio::encode_prepared(p, uti, q as f64 / 100.0, spec.strip_gps)?;
                Ok((bytes, p.width, p.height))
            }
            Self::Rust { full, icc, exif, fmt, spec, cached } => {
                if cached.as_ref().map(|c| c.0) != Some(target) {
                    let copy = Pixels { rgba: full.rgba.clone(), width: full.width, height: full.height };
                    let mut px = generic::resize_quality(copy, target)?;
                    if let Some(wm) = &spec.watermark {
                        let size = image::load_from_memory(&wm.png)
                            .map(|i| (i.width(), i.height()))
                            .unwrap_or((1, 1));
                        let rect = wm.place(px.width, px.height, size.0, size.1);
                        generic::composite(&mut px, &wm.png, rect, wm.opacity)?;
                    }
                    *cached = Some((target, px));
                }
                let px = &cached.as_ref().unwrap().1;
                let exif = exif
                    .as_deref()
                    .and_then(|t| jpegmeta::tidy_for_reencode(t, px.width, px.height, spec.strip_gps));
                let bytes = generic::encode_rgba(px, *fmt, q, exif.as_deref(), icc.as_deref())?;
                Ok((bytes, px.width, px.height))
            }
        }
    }
}

/// 원본 파일의 EXIF(TIFF 형식 바이트). JPEG·HEIC·PNG 등 어디서든.
fn exif_of(src: &Path) -> Option<Vec<u8>> {
    if formats::is_jpeg(src) {
        return std::fs::read(src).ok().and_then(|b| jpegmeta::exif_tiff(&b).map(|t| t.to_vec()));
    }
    let mut r = std::io::BufReader::new(std::fs::File::open(src).ok()?);
    exif::Reader::new().read_from_container(&mut r).ok().map(|e| e.buf().to_vec())
}

/// 여러 장 변환. progress(끝난 수, 전체)는 한 장 끝날 때마다 불린다.
pub fn run_batch(
    paths: &[String],
    spec: &Spec,
    backup_dir: Option<&Path>,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Vec<Result<Done, Failed>> {
    use rayon::prelude::*;
    let total = paths.len();
    let done = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .saturating_sub(2)
        .clamp(1, 4);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build();
    let work = || {
        paths
            .par_iter()
            .map(|p| {
                let r = run_one(Path::new(p), spec, backup_dir).map_err(|e| Failed {
                    path: p.clone(),
                    error: e.to_string(),
                });
                progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                r
            })
            .collect()
    };
    match pool {
        Ok(pool) => pool.install(work),
        Err(_) => work(),
    }
}

/// 변환을 되돌린다: 덮어쓴 것은 보관본으로 복구, 새로 만든 것은 휴지통으로.
pub fn undo(d: &Done) -> anyhow::Result<()> {
    match &d.backup {
        Some(b) => {
            if d.replaced_source || d.output != d.source {
                let _ = crate::fsops::trash(Path::new(&d.output));
            }
            std::fs::rename(b, &d.source).or_else(|_| std::fs::copy(b, &d.source).map(|_| ()))?;
        }
        None => {
            crate::fsops::trash(Path::new(&d.output))?;
        }
    }
    Ok(())
}

/// 저장할 형식 목록 (메뉴용): 이 컴퓨터에서 쓸 수 있는 것만
pub fn available_formats() -> Vec<OutFormat> {
    [
        OutFormat::Jpeg,
        OutFormat::Png,
        OutFormat::Webp,
        OutFormat::Heic,
        OutFormat::Avif,
        OutFormat::Tiff,
        OutFormat::Bmp,
    ]
    .into_iter()
    .filter(|f| f.available())
    .collect()
}

#[cfg(test)]
mod tests {

    /// 편집 결과를 저장해도 원본의 촬영 날짜·위치가 남는다 (날짜·장소 정리가 계속 맞게)
    #[cfg(target_os = "macos")]
    #[test]
    fn edited_pixels_keep_shooting_info() {
        use exif::experimental::Writer;
        use exif::{Field, In, Rational, Tag, Value};
        let dir = std::env::temp_dir().join(format!("isee-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("photo.jpg");
        let mut px = vec![200u8; 320 * 240 * 4];
        crate::decode::imageio::write_rgbx(&src, &mut px, 320, 240, "public.jpeg", 0.8, false, 1).unwrap();
        let date = Field { tag: Tag::DateTimeOriginal, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"2024:05:12 14:30:22".to_vec()]) };
        let lat_ref = Field { tag: Tag::GPSLatitudeRef, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"N".to_vec()]) };
        let r = |n| Rational { num: n, denom: 1 };
        let lat = Field { tag: Tag::GPSLatitude, ifd_num: In::PRIMARY, value: Value::Rational(vec![r(33), r(15), r(0)]) };
        let lon_ref = Field { tag: Tag::GPSLongitudeRef, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"E".to_vec()]) };
        let lon = Field { tag: Tag::GPSLongitude, ifd_num: In::PRIMARY, value: Value::Rational(vec![r(126), r(24), r(0)]) };
        let mut w = Writer::new();
        for f in [&date, &lat_ref, &lat, &lon_ref, &lon] {
            w.push_field(f);
        }
        let mut tiff = std::io::Cursor::new(Vec::new());
        w.write(&mut tiff, false).unwrap();
        let tiff = tiff.into_inner();
        let jpeg = std::fs::read(&src).unwrap();
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        out.extend_from_slice(b"Exif\0\0");
        out.extend_from_slice(&tiff);
        out.extend_from_slice(&jpeg[2..]);
        std::fs::write(&src, out).unwrap();

        // 잘라 낸 결과 (가로 200 × 세로 100)
        let edited = Pixels { rgba: vec![90u8; 200 * 100 * 4], width: 200, height: 100 };
        for fmt in [OutFormat::Jpeg, OutFormat::Heic, OutFormat::Png] {
            let spec = Spec { format: Some(fmt), suffix: Some("편집".into()), ..Default::default() };
            let d = run_pixels(&src, Pixels { rgba: edited.rgba.clone(), ..edited }, &spec, None).unwrap();
            assert_eq!((d.width, d.height), (200, 100), "{fmt:?}");
            assert!(d.output.contains("photo_편집"), "{}", d.output);
            let m = crate::meta::exif_meta(Path::new(&d.output)).expect("EXIF 남아야 함");
            let (y, mo, day) = crate::meta::civil(m.0.expect("촬영 날짜"));
            assert_eq!((y, mo, day), (2024, 5, 12), "{fmt:?}");
            let (la, lo) = m.1.expect("GPS");
            assert!((la - 33.25).abs() < 1e-3 && (lo - 126.4).abs() < 1e-3, "{fmt:?}");
        }
        // 크기 줄이기도 같은 길로
        let spec = Spec { resize: Some(Resize::LongSide(100)), suffix: Some("편집".into()), ..Default::default() };
        let d = run_pixels(&src, Pixels { rgba: edited.rgba.clone(), ..edited }, &spec, None).unwrap();
        assert_eq!((d.width, d.height), (100, 50));
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    #[test]
    fn orientation_composition() {
        assert_eq!(compose(1, 6), 6);
        assert_eq!(compose(6, 6), 3);
        assert_eq!(compose(6, 8), 1);
        assert_eq!(compose(3, 6), 8);
        assert_eq!(compose(1, 2), 2);
        assert_eq!(compose(6, 2), 5);
        // 네 번 돌리면 제자리
        let mut o = 1;
        for _ in 0..4 {
            o = compose(o, 6);
        }
        assert_eq!(o, 1);
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("isee-conv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 왼쪽 위만 빨간 120×80 JPEG
    fn marker_jpeg(path: &Path) {
        let (w, h) = (120u32, 80u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let red = x < 30 && y < 20;
                rgba[i..i + 3].copy_from_slice(if red { &[255, 0, 0] } else { &[0, 0, 255] });
            }
        }
        let px = Pixels { rgba, width: w, height: h };
        let b = generic::encode_rgba(&px, OutFormat::Jpeg, 95, None, None).unwrap();
        std::fs::write(path, b).unwrap();
    }

    /// 화면에 보이는 방향으로 디코딩했을 때 빨간 점이 어느 구석에 있는지 (0=왼위,1=오위,2=왼아래,3=오아래)
    fn red_corner(path: &Path) -> (u32, u32, usize) {
        let (p, _) = generic::decode_for_convert(path).unwrap();
        let px = |x: u32, y: u32| {
            let i = ((y * p.width + x) * 4) as usize;
            p.rgba[i] > 200 && p.rgba[i + 2] < 80
        };
        let corners = [(2, 2), (p.width - 3, 2), (2, p.height - 3), (p.width - 3, p.height - 3)];
        let c = corners.iter().position(|&(x, y)| px(x, y)).unwrap_or(9);
        (p.width, p.height, c)
    }

    #[test]
    fn lossless_rotate_jpeg_then_reencode_matches() {
        let d = tmpdir("rot");
        let src = d.join("a.jpg");
        marker_jpeg(&src);
        // 무손실: 회전 값만 바뀜 → 보이는 방향은 오른쪽으로 90°, 빨간 점은 오른쪽 위
        let spec = Spec { turn: Some(Turn::Cw90), save: SaveMode::NewFile, ..Default::default() };
        let r = run_one(&src, &spec, None).unwrap();
        assert!(r.lossless);
        assert_eq!(red_corner(Path::new(&r.output)), (80, 120, 1));
        // 다시 저장 경로(크기 변경과 함께 회전)도 같은 방향이어야 한다
        let spec2 = Spec {
            turn: Some(Turn::Cw90),
            resize: Some(Resize::Percent(50)),
            save: SaveMode::NewFile,
            ..Default::default()
        };
        let r2 = run_one(&src, &spec2, None).unwrap();
        assert!(!r2.lossless);
        assert_eq!(red_corner(Path::new(&r2.output)), (40, 60, 1));
        // 좌우 뒤집기 → 빨간 점은 오른쪽 위
        let spec3 = Spec { turn: Some(Turn::FlipH), resize: Some(Resize::LongSide(60)), ..Default::default() };
        let r3 = run_one(&src, &spec3, None).unwrap();
        assert_eq!(red_corner(Path::new(&r3.output)), (60, 40, 1));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 회전 5종 × 두 경로(맥 OS 경로=JPEG, Rust 경로=WebP)에서 빨간 점이 가야 할 구석
    #[test]
    fn all_turns_both_paths() {
        let d = tmpdir("turns");
        let src = d.join("m.jpg");
        marker_jpeg(&src);
        let cases = [
            (Turn::Cw90, (40, 60, 1)),
            (Turn::Ccw90, (40, 60, 2)),
            (Turn::R180, (60, 40, 3)),
            (Turn::FlipH, (60, 40, 1)),
            (Turn::FlipV, (60, 40, 2)),
        ];
        for fmt in [OutFormat::Jpeg, OutFormat::Webp, OutFormat::Png] {
            for (turn, want) in cases {
                let spec = Spec {
                    turn: Some(turn),
                    resize: Some(Resize::Percent(50)),
                    format: Some(fmt),
                    ..Default::default()
                };
                let r = run_one(&src, &spec, None).unwrap();
                assert_eq!(red_corner(Path::new(&r.output)), want, "{fmt:?} {turn:?}");
            }
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 반투명 아닌 초록 사각형 워터마크를 오른쪽 아래에 → 그 자리가 초록이어야 한다 (두 경로 모두)
    #[test]
    fn watermark_bottom_right() {
        let d = tmpdir("wm");
        let src = d.join("w.jpg");
        marker_jpeg(&src);
        let mut png = Vec::new();
        let green = image::RgbaImage::from_pixel(40, 20, image::Rgba([0, 255, 0, 255]));
        image::DynamicImage::ImageRgba8(green)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        for fmt in [OutFormat::Jpeg, OutFormat::Webp] {
            let spec = Spec {
                format: Some(fmt),
                watermark: Some(Watermark { png: png.clone(), anchor: 8, width_ratio: 0.25, margin_ratio: 0.0, opacity: 1.0 }),
                ..Default::default()
            };
            let r = run_one(&src, &spec, None).unwrap_or_else(|e| panic!("{fmt:?}: {e:#}"));
            let (p, _) = generic::decode_for_convert(Path::new(&r.output)).unwrap();
            let at = |x: u32, y: u32| {
                let i = ((y * p.width + x) * 4) as usize;
                (p.rgba[i], p.rgba[i + 1], p.rgba[i + 2])
            };
            let (r_, g, b) = at(p.width - 5, p.height - 4);
            assert!(g > 200 && r_ < 80 && b < 80, "{fmt:?} 오른쪽 아래가 초록이 아님: {:?}", (r_, g, b));
            let (_, g2, b2) = at(5, 40);
            assert!(b2 > 200 && g2 < 80, "{fmt:?} 다른 곳이 바뀜");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn overwrite_keeps_backup_and_undo_restores() {
        let d = tmpdir("ow");
        let src = d.join("b.jpg");
        marker_jpeg(&src);
        let before = std::fs::read(&src).unwrap();
        let spec = Spec { format: Some(OutFormat::Png), save: SaveMode::Overwrite, ..Default::default() };
        let r = run_one(&src, &spec, Some(&d.join("undo"))).unwrap();
        assert!(r.replaced_source && !src.exists() && r.output.ends_with("b.png"));
        undo(&r).unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), before);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn fit_to_size() {
        let d = tmpdir("size");
        let src = d.join("c.jpg");
        // 노이즈가 많아 큰 JPEG
        let (w, h) = (800u32, 600u32);
        let mut seed = 1u32;
        let rgba: Vec<u8> = (0..w * h * 4)
            .map(|i| {
                if i % 4 == 3 {
                    255
                } else {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (seed >> 16) as u8
                }
            })
            .collect();
        let b = generic::encode_rgba(&Pixels { rgba, width: w, height: h }, OutFormat::Jpeg, 98, None, None).unwrap();
        std::fs::write(&src, &b).unwrap();
        let spec = Spec { max_bytes: Some(60_000), ..Default::default() };
        let r = run_one(&src, &spec, None).unwrap();
        assert!(r.bytes <= 60_000, "{} bytes", r.bytes);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
