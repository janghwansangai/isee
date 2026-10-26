//! 순수 Rust 디코딩 (윈도우 기본 경로이자 맥의 예비 경로).
//! JPEG는 image 크레이트 안의 zune-jpeg(SIMD)로 디코딩한다.

use super::{Encoded, Pixels};
use anyhow::Context;
use fast_image_resize::images::Image as FirImage;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::codecs::jpeg::JpegEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;

/// EXIF 안의 작은 JPEG 썸네일(보통 160×120) 바이트. 파일 앞부분만 읽는다.
pub fn exif_thumbnail(path: &Path) -> Option<Vec<u8>> {
    let mut r = BufReader::new(File::open(path).ok()?);
    let exif = exif::Reader::new().read_from_container(&mut r).ok()?;
    let off = exif
        .get_field(exif::Tag::JPEGInterchangeFormat, exif::In::THUMBNAIL)?
        .value
        .get_uint(0)? as usize;
    let len = exif
        .get_field(exif::Tag::JPEGInterchangeFormatLength, exif::In::THUMBNAIL)?
        .value
        .get_uint(0)? as usize;
    exif.buf().get(off..off + len).map(|s| s.to_vec())
}

fn exif_orientation(path: &Path) -> Orientation {
    let read = || -> Option<u8> {
        let mut r = BufReader::new(File::open(path).ok()?);
        let exif = exif::Reader::new().read_from_container(&mut r).ok()?;
        let v = exif
            .get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
            .value
            .get_uint(0)?;
        Some(v as u8)
    };
    read()
        .and_then(Orientation::from_exif)
        .unwrap_or(Orientation::NoTransforms)
}

/// 원본 해상도 전체 디코딩 + 회전 방향 적용.
pub fn decode_full(path: &Path) -> anyhow::Result<DynamicImage> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut dec = reader.into_decoder().context("지원하지 않는 형식")?;
    let orient = dec.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec)?;
    img.apply_orientation(orient);
    Ok(img)
}

/// 긴 변 max_px 이하로 축소 (이미 작으면 그대로). 픽셀 형식(RGB/RGBA)은 유지한다.
fn resize_raw(
    buf: Vec<u8>,
    w: u32,
    h: u32,
    pt: PixelType,
    max_px: u32,
) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    resize_raw_alpha(buf, w, h, pt, max_px, true)
}

/// use_alpha = false: 불투명 RGBA는 알파 곱셈 과정을 건너뛰어 더 빠르게 줄인다.
fn resize_raw_alpha(
    buf: Vec<u8>,
    w: u32,
    h: u32,
    pt: PixelType,
    max_px: u32,
    use_alpha: bool,
) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let scale = (max_px as f64 / w.max(h) as f64).min(1.0);
    let (nw, nh) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    if (nw, nh) == (w, h) {
        return Ok((buf, w, h));
    }
    let src = FirImage::from_vec_u8(w, h, buf, pt)?;
    let mut dst = FirImage::new(nw, nh, pt);
    Resizer::new().resize(
        &src,
        &mut dst,
        &ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
            .use_alpha(use_alpha),
    )?;
    Ok((dst.into_vec(), nw, nh))
}

/// 이미 디코딩된 RGBA를 긴 변 max_px 이하로 줄인다.
pub fn resize_pixels(p: Pixels, max_px: u32) -> anyhow::Result<Pixels> {
    let opaque = p.rgba.chunks_exact(4).all(|c| c[3] == 255);
    let (rgba, width, height) =
        resize_raw_alpha(p.rgba, p.width, p.height, PixelType::U8x4, max_px, !opaque)?;
    Ok(Pixels { rgba, width, height })
}

/// 긴 변 max_px 이하 RGB8.
pub fn resize_rgb(img: DynamicImage, max_px: u32) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let (w, h) = (img.width(), img.height());
    resize_raw(img.into_rgb8().into_raw(), w, h, PixelType::U8x3, max_px)
}

pub fn encode_jpeg(rgb: &[u8], w: u32, h: u32, quality: u8) -> anyhow::Result<Encoded> {
    let mut out = Vec::with_capacity(rgb.len() / 8);
    JpegEncoder::new_with_quality(&mut out, quality).encode(
        rgb,
        w,
        h,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(Encoded {
        bytes: out,
        width: w,
        height: h,
        mime: "image/jpeg",
    })
}

pub fn thumbnail(path: &Path, max_px: u32) -> anyhow::Result<Encoded> {
    // 내장 썸네일이 충분히 크면 원본을 디코딩하지 않는다.
    if let Some(bytes) = exif_thumbnail(path)
        && let Ok(mut small) = image::load_from_memory(&bytes)
        && small.width().max(small.height()) * 4 >= max_px * 3
    {
        small.apply_orientation(exif_orientation(path));
        let (rgb, w, h) = resize_rgb(small, max_px)?;
        return encode_jpeg(&rgb, w, h, 80);
    }
    let img = decode_full(path)?;
    let (rgb, w, h) = resize_rgb(img, max_px)?;
    encode_jpeg(&rgb, w, h, 80)
}

/// JPEG 전용 빠른 경로: zune-jpeg로 RGBA를 바로 풀고(중간 변환 없음), 줄인 다음에 회전한다.
pub fn jpeg_rgba(path: &Path, max_px: u32) -> anyhow::Result<Pixels> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let data = std::fs::read(path)?;
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(1 << 16)
        .set_max_height(1 << 16);
    let mut dec = JpegDecoder::new_with_options(ZCursor::new(&data[..]), opts);
    let px = dec.decode().map_err(|e| anyhow::anyhow!("JPEG 디코딩 실패: {e:?}"))?;
    let info = dec.info().context("JPEG 헤더 없음")?;
    let (w, h) = (info.width as u32, info.height as u32);
    let orientation = dec
        .exif()
        .and_then(|raw| exif::Reader::new().read_raw(raw.clone()).ok())
        .and_then(|e| {
            e.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
                .value
                .get_uint(0)
        })
        .unwrap_or(1);
    let target = if max_px == 0 { w.max(h) } else { max_px };
    let (rgba, w, h) = resize_raw_alpha(px, w, h, PixelType::U8x4, target, false)?;
    let (rgba, width, height) = orient_rgba(rgba, w, h, orientation);
    Ok(Pixels { rgba, width, height })
}

/// EXIF 회전 값(1~8)대로 RGBA 픽셀을 돌리거나 뒤집는다.
pub fn orient_rgba(rgba: Vec<u8>, w: u32, h: u32, orientation: u32) -> (Vec<u8>, u32, u32) {
    if !(2..=8).contains(&orientation) {
        return (rgba, w, h);
    }
    let (w, h) = (w as usize, h as usize);
    let src: Vec<u32> = rgba
        .chunks_exact(4)
        .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let swap = orientation >= 5;
    let (nw, nh) = if swap { (h, w) } else { (w, h) };
    let mut dst = vec![0u32; w * h];
    for y in 0..nh {
        let row = &mut dst[y * nw..(y + 1) * nw];
        for (x, out) in row.iter_mut().enumerate() {
            // 결과 좌표 (x, y)가 원본의 어느 픽셀인지
            let (sx, sy) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                _ => (w - 1 - y, x), // 8
            };
            *out = src[sy * w + sx];
        }
    }
    let bytes = dst.iter().flat_map(|p| p.to_ne_bytes()).collect();
    (bytes, nw as u32, nh as u32)
}

/// 뷰어용 RGBA. max_px = 0 이면 원본 크기.
pub fn screen_rgba(path: &Path, max_px: u32) -> anyhow::Result<Pixels> {
    if crate::formats::is_jpeg(path)
        && let Ok(p) = jpeg_rgba(path, max_px)
    {
        return Ok(p);
    }
    let img = decode_full(path)?;
    let (w, h) = (img.width(), img.height());
    let target = if max_px == 0 { w.max(h) } else { max_px };
    if img.color().has_alpha() {
        let (rgba, width, height) =
            resize_raw(img.into_rgba8().into_raw(), w, h, PixelType::U8x4, target)?;
        return Ok(Pixels { rgba, width, height });
    }
    // 불투명 이미지는 RGB로 줄인 뒤에 RGBA로 늘려야 덜 복사한다.
    let (rgb, width, height) = resize_raw(img.into_rgb8().into_raw(), w, h, PixelType::U8x3, target)?;
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    Ok(Pixels { rgba, width, height })
}

pub fn dimensions(path: &Path) -> anyhow::Result<(u32, u32)> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut dec = reader.into_decoder()?;
    let (w, h) = dec.dimensions();
    let swap = matches!(
        dec.orientation().unwrap_or(Orientation::NoTransforms),
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    Ok(if swap { (h, w) } else { (w, h) })
}

/// 메모리의 JPEG 바이트 디코딩 (측정용).
pub fn decode_bytes(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    Ok(ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?)
}

#[cfg(test)]
mod tests {
    use super::orient_rgba;

    /// 2×1 이미지 [A B]를 회전 값마다 돌렸을 때의 결과
    #[test]
    fn orientation_transforms() {
        let a = [1u8, 0, 0, 255];
        let b = [2u8, 0, 0, 255];
        let src: Vec<u8> = [a, b].concat();
        let px = |v: &[u8]| v.chunks(4).map(|c| c[0]).collect::<Vec<_>>();
        assert_eq!(orient_rgba(src.clone(), 2, 1, 1).0, src);
        let (r, w, h) = orient_rgba(src.clone(), 2, 1, 2);
        assert_eq!((px(&r), w, h), (vec![2, 1], 2, 1));
        let (r, w, h) = orient_rgba(src.clone(), 2, 1, 3);
        assert_eq!((px(&r), w, h), (vec![2, 1], 2, 1));
        // 6: 시계 방향 90° → 세로 2칸, 위가 A
        let (r, w, h) = orient_rgba(src.clone(), 2, 1, 6);
        assert_eq!((px(&r), w, h), (vec![1, 2], 1, 2));
        // 8: 반시계 90° → 위가 B
        let (r, w, h) = orient_rgba(src.clone(), 2, 1, 8);
        assert_eq!((px(&r), w, h), (vec![2, 1], 1, 2));
    }
}

// ───────────────────────── 변환 (2단계) ─────────────────────────

/// 변환용 원본 해상도 RGBA (회전 방향 적용됨)와 색 프로필.
/// JPEG는 zune-jpeg, 그 밖은 image 크레이트. 색은 바꾸지 않으므로 ICC 프로필을 함께 돌려준다.
pub fn decode_for_convert(path: &Path) -> anyhow::Result<(Pixels, Option<Vec<u8>>)> {
    if crate::formats::is_jpeg(path) {
        use zune_jpeg::JpegDecoder;
        use zune_jpeg::zune_core::bytestream::ZCursor;
        use zune_jpeg::zune_core::colorspace::ColorSpace;
        use zune_jpeg::zune_core::options::DecoderOptions;
        let data = std::fs::read(path)?;
        let opts = DecoderOptions::default()
            .jpeg_set_out_colorspace(ColorSpace::RGBA)
            .set_max_width(1 << 16)
            .set_max_height(1 << 16);
        let mut dec = JpegDecoder::new_with_options(ZCursor::new(&data[..]), opts);
        let px = dec.decode().map_err(|e| anyhow::anyhow!("JPEG 디코딩 실패: {e:?}"))?;
        let info = dec.info().context("JPEG 헤더 없음")?;
        let icc = dec.icc_profile();
        let o = crate::jpegmeta::orientation(&data) as u32;
        let (rgba, width, height) = orient_rgba(px, info.width as u32, info.height as u32, o);
        return Ok((Pixels { rgba, width, height }, icc));
    }
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut dec = reader.into_decoder().context("지원하지 않는 형식")?;
    let orient = dec.orientation().unwrap_or(Orientation::NoTransforms);
    let icc = dec.icc_profile().ok().flatten();
    let mut img = DynamicImage::from_decoder(dec)?;
    img.apply_orientation(orient);
    let (width, height) = (img.width(), img.height());
    Ok((Pixels { rgba: img.into_rgba8().into_raw(), width, height }, icc))
}

/// 저장용 고품질 축소 (Lanczos3)
pub fn resize_quality(p: Pixels, max_px: u32) -> anyhow::Result<Pixels> {
    if max_px == 0 || p.width.max(p.height) <= max_px {
        return Ok(p);
    }
    let opaque = p.rgba.chunks_exact(4).all(|c| c[3] == 255);
    let scale = max_px as f64 / p.width.max(p.height) as f64;
    let (nw, nh) = (
        ((p.width as f64 * scale).round() as u32).max(1),
        ((p.height as f64 * scale).round() as u32).max(1),
    );
    let src = FirImage::from_vec_u8(p.width, p.height, p.rgba, PixelType::U8x4)?;
    let mut dst = FirImage::new(nw, nh, PixelType::U8x4);
    Resizer::new().resize(
        &src,
        &mut dst,
        &ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
            .use_alpha(!opaque),
    )?;
    Ok(Pixels { rgba: dst.into_vec(), width: nw, height: nh })
}

/// PNG 워터마크를 (x, y, 가로, 세로) 자리에 opacity만큼 비치게 합성한다.
pub fn composite(p: &mut Pixels, png: &[u8], rect: (f64, f64, f64, f64), opacity: f32) -> anyhow::Result<()> {
    let wm = image::load_from_memory(png)?.into_rgba8();
    let (dw, dh) = (rect.2.round().max(1.0) as u32, rect.3.round().max(1.0) as u32);
    let src = FirImage::from_vec_u8(wm.width(), wm.height(), wm.into_raw(), PixelType::U8x4)?;
    let mut dst = FirImage::new(dw, dh, PixelType::U8x4);
    Resizer::new().resize(
        &src,
        &mut dst,
        &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
    )?;
    let ov = dst.into_vec();
    let (x0, y0) = (rect.0.round() as i64, rect.1.round() as i64);
    for oy in 0..dh as i64 {
        let y = y0 + oy;
        if y < 0 || y >= p.height as i64 {
            continue;
        }
        for ox in 0..dw as i64 {
            let x = x0 + ox;
            if x < 0 || x >= p.width as i64 {
                continue;
            }
            let o = &ov[((oy * dw as i64 + ox) * 4) as usize..][..4];
            let a = o[3] as f32 / 255.0 * opacity;
            if a <= 0.0 {
                continue;
            }
            let d = &mut p.rgba[((y * p.width as i64 + x) * 4) as usize..][..4];
            for c in 0..3 {
                d[c] = (o[c] as f32 * a + d[c] as f32 * (1.0 - a)).round() as u8;
            }
            d[3] = d[3].max((a * 255.0) as u8);
        }
    }
    Ok(())
}

fn rgb_of(p: &Pixels) -> Vec<u8> {
    p.rgba.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]).collect()
}

/// RGBA를 형식에 맞게 저장. exif는 TIFF 블록(JPEG에만 붙임), quality는 1~100.
pub fn encode_rgba(
    p: &Pixels,
    format: crate::convert::OutFormat,
    quality: u8,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
) -> anyhow::Result<Vec<u8>> {
    use crate::convert::OutFormat as F;
    use image::ImageEncoder;
    let opaque = p.rgba.chunks_exact(4).all(|c| c[3] == 255);
    let mut out = Vec::new();
    match format {
        F::Jpeg => {
            let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
            enc.set_sampling_factor(if quality >= 90 {
                jpeg_encoder::SamplingFactor::R_4_4_4
            } else {
                jpeg_encoder::SamplingFactor::R_4_2_0
            });
            enc.set_optimized_huffman_tables(true);
            if let Some(icc) = icc {
                let _ = enc.add_icc_profile(icc);
            }
            if let Some(exif) = exif {
                let _ = enc.add_exif_metadata(exif);
            }
            if p.width > u16::MAX as u32 || p.height > u16::MAX as u32 {
                anyhow::bail!("JPEG로 저장하기에 너무 큼");
            }
            enc.encode(&p.rgba, p.width as u16, p.height as u16, jpeg_encoder::ColorType::Rgba)?;
        }
        F::Png => {
            let mut enc = image::codecs::png::PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Adaptive,
            );
            if let Some(icc) = icc {
                let _ = enc.set_icc_profile(icc.to_vec());
            }
            if opaque {
                enc.write_image(&rgb_of(p), p.width, p.height, image::ExtendedColorType::Rgb8)?;
            } else {
                enc.write_image(&p.rgba, p.width, p.height, image::ExtendedColorType::Rgba8)?;
            }
        }
        F::Webp => {
            // 기본 설정(method 4)은 12MP에 0.7초가 걸려서, 크기 차이가 작은 빠른 설정을 쓴다
            let mut cfg = webp::WebPConfig::new().map_err(|_| anyhow::anyhow!("WebP 설정 오류"))?;
            cfg.quality = quality as f32;
            cfg.method = 2;
            cfg.thread_level = 1;
            let rgb;
            let enc = if opaque {
                rgb = rgb_of(p);
                webp::Encoder::from_rgb(&rgb, p.width, p.height)
            } else {
                webp::Encoder::from_rgba(&p.rgba, p.width, p.height)
            };
            out = enc
                .encode_advanced(&cfg)
                .map_err(|e| anyhow::anyhow!("WebP 저장 실패: {e:?}"))?
                .to_vec();
        }
        F::Bmp => {
            let mut enc = image::codecs::bmp::BmpEncoder::new(&mut out);
            if opaque {
                enc.encode(&rgb_of(p), p.width, p.height, image::ExtendedColorType::Rgb8)?;
            } else {
                enc.encode(&p.rgba, p.width, p.height, image::ExtendedColorType::Rgba8)?;
            }
        }
        F::Tiff => {
            let enc = image::codecs::tiff::TiffEncoder::new(Cursor::new(&mut out));
            enc.write_image(&p.rgba, p.width, p.height, image::ExtendedColorType::Rgba8)?;
        }
        F::Heic | F::Avif => anyhow::bail!("이 형식은 지금은 맥에서만 저장할 수 있습니다"),
    }
    Ok(out)
}
