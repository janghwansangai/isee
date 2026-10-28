//! 디코딩 진입점. 형식마다 측정해서 더 빠른 쪽을 쓴다 (docs/BENCH.md).
//! - JPEG 축소, HEIC, RAW, PSD, AVIF: macOS ImageIO (1/2·1/4·1/8 축소 디코딩, OS 코덱)
//! - PNG, TIFF, BMP, 화면보다 작게 줄일 수 없는 JPEG: 순수 Rust 디코더 (zune-jpeg 등)
//! 윈도우는 지금은 Rust 디코더만 쓴다 (WIC는 이후 단계).

pub mod generic;
#[cfg(target_os = "macos")]
pub mod imageio;
#[cfg(target_os = "macos")]
pub mod quicklook;

use std::path::Path;

/// 화면에 보낼 수 있게 인코딩된 이미지.
#[derive(Clone, Debug)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub mime: &'static str,
}

/// 압축하지 않은 RGBA 픽셀 (알파는 곱해지지 않은 값). 뷰어가 그대로 화면에 올린다.
#[derive(Clone, Debug)]
pub struct Pixels {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Rust 디코더가 OS보다 빠르다고 측정된 형식.
fn generic_first(path: &Path) -> bool {
    matches!(ext(path).as_str(), "png" | "tif" | "tiff" | "bmp")
}

/// 깨진 파일 때문에 디코더가 멈추면(패닉) 그 파일만 실패로 돌려준다.
/// 사진 한 장 때문에 프로그램 전체가 꺼지지 않게 한다 (보통 때는 비용 없음).
fn guarded<T>(f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err(anyhow::anyhow!("파일을 읽다가 오류가 났습니다 (깨진 파일일 수 있음)")))
}

/// 그리드용 썸네일 (긴 변 max_px 이하, JPEG/PNG로 인코딩).
pub fn thumbnail(path: &Path, max_px: u32) -> anyhow::Result<Encoded> {
    guarded(|| thumbnail_inner(path, max_px))
}

fn thumbnail_inner(path: &Path, max_px: u32) -> anyhow::Result<Encoded> {
    use crate::formats::{Kind, kind_of};
    let kind = kind_of(path);
    // 동영상·PDF·문서, 그리고 아직 내려받지 않은 클라우드 파일은 OS 썸네일 엔진으로 (내용을 읽지 않는다)
    if kind != Kind::Image || crate::listing::is_online_only(path) {
        #[cfg(target_os = "macos")]
        return quicklook::thumbnail(path, max_px, std::time::Duration::from_secs(8));
        #[cfg(not(target_os = "macos"))]
        anyhow::bail!("이 종류의 썸네일은 아직 지원하지 않음");
    }
    #[cfg(target_os = "macos")]
    if !generic_first(path)
        && let Ok(e) = imageio::thumbnail(path, max_px)
    {
        return Ok(e);
    }
    match generic::thumbnail(path, max_px) {
        Ok(e) => Ok(e),
        #[cfg(target_os = "macos")]
        Err(_) if generic_first(path) => imageio::thumbnail(path, max_px),
        Err(e) => Err(e),
    }
}

/// 뷰어용 픽셀: 긴 변 max_px 이하 (0이면 원본 크기). 회전 방향은 적용되어 있다.
pub fn screen_rgba(path: &Path, max_px: u32) -> anyhow::Result<Pixels> {
    guarded(|| screen_rgba_inner(path, max_px))
}

fn screen_rgba_inner(path: &Path, max_px: u32) -> anyhow::Result<Pixels> {
    #[cfg(target_os = "macos")]
    if let Some(request) = imageio_screen_request(path, max_px)
        && let Ok(p) = imageio::screen_rgba(path, request)
    {
        // 축소 디코딩 결과가 목표보다 크면 마지막 크기 맞춤은 멀티코어 리사이저로 (ImageIO 내부 축소보다 빠름)
        return if max_px > 0 && p.width.max(p.height) > max_px + max_px / 50 {
            generic::resize_pixels(p, max_px)
        } else {
            Ok(p)
        };
    }
    match generic::screen_rgba(path, max_px) {
        Ok(p) => Ok(p),
        #[cfg(target_os = "macos")]
        Err(_) => imageio::screen_rgba(path, max_px),
        #[cfg(not(target_os = "macos"))]
        Err(e) => Err(e),
    }
}

/// JPEG는 1/2 이하로 줄여 디코딩할 수 있을 때만 ImageIO가 빠르다.
/// 1/2로 줄인 결과가 목표의 90% 이상이면 축소 디코딩을 쓴다 (화면에서 구분되지 않는 차이).
/// 그보다 작으면 원본 크기로 풀어야 하는데, 그때는 zune-jpeg가 더 빠르다.
/// ImageIO에 요청할 긴 변 크기. None이면 Rust 디코더를 쓴다.
/// JPEG는 1/2·1/4·1/8 축소 디코딩 크기와 정확히 같게 요청해야 ImageIO가 내부에서 다시 줄이지 않는다.
/// 축소 결과가 목표의 90% 이상이면 그 배율을 쓴다 (화면에서 구분되지 않는 차이).
/// 1/2도 못 쓰면 원본 크기로 풀어야 하는데, 그때는 zune-jpeg가 더 빠르다.
#[cfg(target_os = "macos")]
fn imageio_screen_request(path: &Path, max_px: u32) -> Option<u32> {
    if generic_first(path) {
        return None;
    }
    if !crate::formats::is_jpeg(path) {
        return Some(max_px);
    }
    if max_px == 0 {
        return None;
    }
    let (w, h) = imageio::dimensions(path).ok()?;
    let long = w.max(h);
    [8, 4, 2]
        .into_iter()
        .map(|f| long.div_ceil(f))
        .find(|&scaled| scaled * 10 >= max_px * 9)
}

/// 회전 방향을 적용한 뒤의 가로·세로 픽셀 수. 파일 머리만 읽는다.
pub fn dimensions(path: &Path) -> anyhow::Result<(u32, u32)> {
    #[cfg(target_os = "macos")]
    if let Ok(d) = imageio::dimensions(path) {
        return Ok(d);
    }
    generic::dimensions(path)
}

/// 곱해진 알파(premultiplied)를 원래 값으로 되돌린다.
pub(crate) fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}
