//! macOS ImageIO 직접 호출. JPEG는 축소 디코딩과 하드웨어 디코더를 쓰고,
//! HEIC, RAW, PSD, TIFF, WebP, AVIF도 OS가 처리한다.
#![allow(non_upper_case_globals)]

use super::Encoded;
use anyhow::{Context, bail};
use std::ffi::{c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null;

type Ref = *const c_void;
type CFIndex = isize;

#[repr(C)]
struct Opaque {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFAllocatorDefault: Ref;
    static kCFBooleanTrue: Ref;
    static kCFBooleanFalse: Ref;
    static kCFTypeDictionaryKeyCallBacks: Opaque;
    static kCFTypeDictionaryValueCallBacks: Opaque;
    fn CFRelease(cf: Ref);
    fn CFURLCreateFromFileSystemRepresentation(
        alloc: Ref,
        buf: *const u8,
        len: CFIndex,
        is_dir: u8,
    ) -> Ref;
    fn CFDictionaryCreate(
        alloc: Ref,
        keys: *const Ref,
        values: *const Ref,
        n: CFIndex,
        key_cb: *const Opaque,
        val_cb: *const Opaque,
    ) -> Ref;
    fn CFDictionaryGetValue(d: Ref, key: Ref) -> Ref;
    fn CFNumberCreate(alloc: Ref, ty: CFIndex, value: *const c_void) -> Ref;
    fn CFNumberGetValue(n: Ref, ty: CFIndex, out: *mut c_void) -> u8;
    fn CFDataCreateMutable(alloc: Ref, cap: CFIndex) -> Ref;
    fn CFDataGetBytePtr(d: Ref) -> *const u8;
    fn CFDataGetLength(d: Ref) -> CFIndex;
    fn CFStringCreateWithCString(alloc: Ref, s: *const c_char, enc: u32) -> Ref;
    fn CFDictionaryCreateMutable(alloc: Ref, cap: CFIndex, key_cb: *const Opaque, val_cb: *const Opaque) -> Ref;
    fn CFDictionaryCreateMutableCopy(alloc: Ref, cap: CFIndex, d: Ref) -> Ref;
    fn CFDictionarySetValue(d: Ref, k: Ref, v: Ref);
    fn CFDictionaryRemoveValue(d: Ref, k: Ref);
    fn CFDataCreate(alloc: Ref, bytes: *const u8, len: CFIndex) -> Ref;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    static kCGImageSourceCreateThumbnailFromImageAlways: Ref;
    static kCGImageSourceThumbnailMaxPixelSize: Ref;
    static kCGImageSourceCreateThumbnailWithTransform: Ref;
    static kCGImageSourceShouldCache: Ref;
    static kCGImageSourceShouldCacheImmediately: Ref;
    static kCGImagePropertyPixelWidth: Ref;
    static kCGImagePropertyPixelHeight: Ref;
    static kCGImagePropertyOrientation: Ref;
    static kCGImageDestinationLossyCompressionQuality: Ref;
    static kCGImageDestinationEmbedThumbnail: Ref;
    static kCGImageDestinationOrientation: Ref;
    static kCGImagePropertyGPSDictionary: Ref;
    static kCGImagePropertyTIFFDictionary: Ref;
    static kCGImagePropertyTIFFOrientation: Ref;
    static kCGImagePropertyExifDictionary: Ref;
    static kCGImagePropertyExifPixelXDimension: Ref;
    static kCGImagePropertyExifPixelYDimension: Ref;
    fn CGImageSourceCreateWithData(data: Ref, opts: Ref) -> Ref;
    fn CGImageSourceCreateImageAtIndex(src: Ref, idx: usize, opts: Ref) -> Ref;
    fn CGImageSourceGetType(src: Ref) -> Ref;
    fn CGImageSourceGetCount(src: Ref) -> usize;
    fn CGImageSourceGetStatusAtIndex(src: Ref, index: usize) -> i32;
    fn CGImageDestinationCopyImageSource(dest: Ref, src: Ref, opts: Ref, err: *mut Ref) -> bool;
    fn CGImageSourceCreateWithURL(url: Ref, opts: Ref) -> Ref;
    fn CGImageDestinationCreateWithURL(url: Ref, ty: Ref, count: usize, opts: Ref) -> Ref;
    fn CGImageSourceCreateThumbnailAtIndex(src: Ref, idx: usize, opts: Ref) -> Ref;
    fn CGImageSourceCopyPropertiesAtIndex(src: Ref, idx: usize, opts: Ref) -> Ref;
    fn CGImageDestinationCreateWithData(data: Ref, ty: Ref, count: usize, opts: Ref) -> Ref;
    fn CGImageDestinationAddImage(dest: Ref, img: Ref, props: Ref);
    fn CGImageDestinationFinalize(dest: Ref) -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: Ref;
    fn CGImageGetWidth(img: Ref) -> usize;
    fn CGImageGetHeight(img: Ref) -> usize;
    fn CGImageGetAlphaInfo(img: Ref) -> u32;
    fn CGColorSpaceCreateWithName(name: Ref) -> Ref;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        w: usize,
        h: usize,
        bpc: usize,
        bpr: usize,
        cs: Ref,
        info: u32,
    ) -> Ref;
    fn CGContextDrawImage(ctx: Ref, rect: CGRect, img: Ref);
    fn CGBitmapContextCreateImage(ctx: Ref) -> Ref;
    fn CGImageGetColorSpace(img: Ref) -> Ref;
    fn CGColorSpaceGetModel(cs: Ref) -> i32;
    fn CGContextConcatCTM(ctx: Ref, t: CGAffineTransform);
    fn CGContextSetInterpolationQuality(ctx: Ref, q: i32);
    fn CGContextSetAlpha(ctx: Ref, a: f64);
    fn CGContextSaveGState(ctx: Ref);
    fn CGContextRestoreGState(ctx: Ref);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

const kCFNumberSInt64Type: CFIndex = 4;
const kCFNumberFloat64Type: CFIndex = 6;
const kCFStringEncodingUTF8: u32 = 0x0800_0100;
const kCGImageAlphaPremultipliedLast: u32 = 1;
const kCGImageAlphaNoneSkipLast: u32 = 5;
const kCGBitmapByteOrder32Big: u32 = 4 << 12;

/// 소유한 CoreFoundation 객체. 범위를 벗어나면 해제한다.
struct Cf(Ref);

impl Cf {
    fn new(r: Ref) -> Option<Self> {
        (!r.is_null()).then_some(Self(r))
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}

fn num_i64(v: i64) -> Cf {
    Cf(unsafe { CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt64Type, &v as *const _ as _) })
}

fn num_f64(v: f64) -> Cf {
    Cf(unsafe { CFNumberCreate(kCFAllocatorDefault, kCFNumberFloat64Type, &v as *const _ as _) })
}

fn cfstr(s: &str) -> Cf {
    let c = std::ffi::CString::new(s).unwrap();
    Cf(unsafe { CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), kCFStringEncodingUTF8) })
}

fn dict(pairs: &[(Ref, Ref)]) -> Cf {
    let keys: Vec<Ref> = pairs.iter().map(|p| p.0).collect();
    let vals: Vec<Ref> = pairs.iter().map(|p| p.1).collect();
    Cf(unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr(),
            vals.as_ptr(),
            pairs.len() as CFIndex,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    })
}

fn get_i64(d: Ref, key: Ref) -> Option<i64> {
    let v = unsafe { CFDictionaryGetValue(d, key) };
    if v.is_null() {
        return None;
    }
    let mut out: i64 = 0;
    (unsafe { CFNumberGetValue(v, kCFNumberSInt64Type, &mut out as *mut _ as _) } != 0)
        .then_some(out)
}

fn source(path: &Path) -> anyhow::Result<Cf> {
    let bytes = path.as_os_str().as_bytes();
    let url = Cf::new(unsafe {
        CFURLCreateFromFileSystemRepresentation(
            kCFAllocatorDefault,
            bytes.as_ptr(),
            bytes.len() as CFIndex,
            0,
        )
    })
    .context("파일 경로를 열 수 없음")?;
    // 원본 전체를 디코딩해서 메모리에 붙잡아 두지 않게 한다.
    let opts = dict(&[(unsafe { kCGImageSourceShouldCache }, unsafe { kCFBooleanFalse })]);
    let src = Cf::new(unsafe { CGImageSourceCreateWithURL(url.0, opts.0) }).context("이미지 소스를 만들 수 없음")?;
    // 형식을 알 수 없거나 사진이 없는 파일(깨진 파일, 확장자만 RAW인 파일 등)은 여기서 멈춘다.
    // 그대로 썸네일을 요청하면 ImageIO 안에서 앱이 강제 종료될 수 있다.
    if unsafe { CGImageSourceGetType(src.0) }.is_null() || unsafe { CGImageSourceGetCount(src.0) } == 0 {
        bail!("읽을 수 없는 이미지");
    }
    // 머리 부분부터 잘린 파일 등: 상태가 오류(-2 이하)면 디코딩하지 않는다. 뒷부분만 잘린 것(-1)은 보이는 데까지 그린다.
    let status = unsafe { CGImageSourceGetStatusAtIndex(src.0, 0) };
    if status < -1 {
        bail!("손상된 이미지 ({status})");
    }
    // 머리 정보(가로·세로)까지는 읽혀야 한다. 머리부터 잘린 JPEG는 ImageIO 안에서 강제 종료된다.
    let props = Cf::new(unsafe { CGImageSourceCopyPropertiesAtIndex(src.0, 0, null()) }).context("손상된 이미지")?;
    let w = get_i64(props.0, unsafe { kCGImagePropertyPixelWidth }).unwrap_or(0);
    let h = get_i64(props.0, unsafe { kCGImagePropertyPixelHeight }).unwrap_or(0);
    if w <= 0 || h <= 0 {
        bail!("손상된 이미지 (크기 없음)");
    }
    Ok(src)
}

/// 긴 변 max_px 이하로 줄인 이미지. 회전 방향 적용됨.
/// from_image=false면 파일에 들어 있는 썸네일만 쓰고, 없으면 None.
fn create_scaled(src: &Cf, max_px: u32, from_image: bool) -> Option<Cf> {
    let max = num_i64(max_px as i64);
    let t = unsafe { kCFBooleanTrue };
    let mut pairs = vec![
        (unsafe { kCGImageSourceThumbnailMaxPixelSize }, max.0),
        (unsafe { kCGImageSourceCreateThumbnailWithTransform }, t),
        (unsafe { kCGImageSourceShouldCacheImmediately }, t),
    ];
    if from_image {
        pairs.push((unsafe { kCGImageSourceCreateThumbnailFromImageAlways }, t));
    }
    let opts = dict(&pairs);
    Cf::new(unsafe { CGImageSourceCreateThumbnailAtIndex(src.0, 0, opts.0) })
}

fn size_of(img: &Cf) -> (u32, u32) {
    unsafe { (CGImageGetWidth(img.0) as u32, CGImageGetHeight(img.0) as u32) }
}

fn has_alpha(img: &Cf) -> bool {
    matches!(unsafe { CGImageGetAlphaInfo(img.0) }, 1..=4 | 7)
}

/// 알파가 있으면 PNG, 없으면 JPEG로 인코딩.
/// 소유권을 받은 CGImage(+1)를 JPEG·PNG로 (Quick Look 썸네일용)
/// 투명한 곳은 흰 바탕으로 채워 JPEG로 만든다 (PDF 페이지·동영상 프레임은 투명할 이유가 없고, PNG는 몇 배 크다).
pub(crate) fn encode_owned(img: Ref, quality: f64) -> anyhow::Result<Encoded> {
    let img = Cf::new(img).context("이미지 없음")?;
    if !has_alpha(&img) {
        return encode(&img, quality);
    }
    let (w, h) = size_of(&img);
    let mut buf = vec![255u8; w as usize * h as usize * 4];
    unsafe {
        let cs = Cf::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB)).context("색 공간")?;
        let ctx = Cf::new(CGBitmapContextCreate(
            buf.as_mut_ptr() as _,
            w as usize,
            h as usize,
            8,
            w as usize * 4,
            cs.0,
            kCGImageAlphaNoneSkipLast | kCGBitmapByteOrder32Big,
        ))
        .context("비트맵")?;
        CGContextDrawImage(ctx.0, CGRect { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }, img.0);
        let flat = Cf::new(CGBitmapContextCreateImage(ctx.0)).context("이미지")?;
        encode(&flat, quality)
    }
}

fn encode(img: &Cf, quality: f64) -> anyhow::Result<Encoded> {
    let alpha = has_alpha(img);
    let (uti, mime) = if alpha {
        ("public.png", "image/png")
    } else {
        ("public.jpeg", "image/jpeg")
    };
    let data = Cf::new(unsafe { CFDataCreateMutable(kCFAllocatorDefault, 0) }).context("메모리")?;
    let ty = cfstr(uti);
    let dest = Cf::new(unsafe { CGImageDestinationCreateWithData(data.0, ty.0, 1, null()) })
        .context("인코더를 만들 수 없음")?;
    let q = num_f64(quality);
    let props = dict(&[(unsafe { kCGImageDestinationLossyCompressionQuality }, q.0)]);
    unsafe { CGImageDestinationAddImage(dest.0, img.0, props.0) };
    if !unsafe { CGImageDestinationFinalize(dest.0) } {
        bail!("인코딩 실패");
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(CFDataGetBytePtr(data.0), CFDataGetLength(data.0) as usize)
    }
    .to_vec();
    let (width, height) = size_of(img);
    Ok(Encoded { bytes, width, height, mime })
}

/// 파일에 들어 있는 썸네일이 목표의 75% 이상이면 그대로 쓰고(가장 빠름),
/// 아니면 원본에서 줄여서 디코딩한다 (JPEG는 1/2·1/4·1/8 축소 디코딩).
pub fn thumbnail(path: &Path, max_px: u32) -> anyhow::Result<Encoded> {
    let src = source(path)?;
    let img = match create_scaled(&src, max_px, false) {
        Some(emb) if size_of(&emb).0.max(size_of(&emb).1) * 4 >= max_px * 3 => emb,
        _ => create_scaled(&src, max_px, true).context("디코딩 실패")?,
    };
    encode(&img, 0.8)
}

/// 파일 속 썸네일만 (측정용). 없으면 에러.
pub fn embedded_thumbnail(path: &Path, max_px: u32) -> anyhow::Result<Encoded> {
    let src = source(path)?;
    let img = create_scaled(&src, max_px, false).context("내장 썸네일 없음")?;
    encode(&img, 0.8)
}

/// 항상 원본에서 줄여 디코딩 (측정용).
pub fn decoded_thumbnail(path: &Path, max_px: u32) -> anyhow::Result<Encoded> {
    let src = source(path)?;
    let img = create_scaled(&src, max_px, true).context("디코딩 실패")?;
    encode(&img, 0.8)
}

pub fn screen(path: &Path, max_px: u32) -> anyhow::Result<Encoded> {
    let src = source(path)?;
    let img = create_scaled(&src, max_px, true).context("디코딩 실패")?;
    encode(&img, 0.9)
}

/// 디코딩만 하고 인코딩은 하지 않는다 (측정용: 디코딩과 인코딩 시간을 나눠 보기 위해).
pub fn decode_only(path: &Path, max_px: u32) -> anyhow::Result<(u32, u32)> {
    let src = source(path)?;
    let img = create_scaled(&src, max_px, true).context("디코딩 실패")?;
    Ok(size_of(&img))
}

/// 뷰어용 RGBA. max_px = 0 이면 원본 크기.
pub fn screen_rgba(path: &Path, max_px: u32) -> anyhow::Result<super::Pixels> {
    let max_px = if max_px == 0 {
        let (w, h) = dimensions(path)?;
        w.max(h)
    } else {
        max_px
    };
    let src = source(path)?;
    let img = create_scaled(&src, max_px, true).context("디코딩 실패")?;
    let alpha = has_alpha(&img);
    let (mut rgba, width, height) = draw_rgba(&img)?;
    if alpha {
        super::unpremultiply(&mut rgba);
    }
    Ok(super::Pixels { rgba, width, height })
}

/// sRGB RGBA 픽셀 (알파 곱해진 값, 분석용).
pub fn rgba(path: &Path, max_px: u32) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let src = source(path)?;
    let img = create_scaled(&src, max_px, true).context("디코딩 실패")?;
    draw_rgba(&img)
}

fn draw_rgba(img: &Cf) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let (w, h) = size_of(img);
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    unsafe {
        let cs = Cf::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB)).context("색 공간")?;
        let ctx = Cf::new(CGBitmapContextCreate(
            buf.as_mut_ptr() as _,
            w as usize,
            h as usize,
            8,
            w as usize * 4,
            cs.0,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
        ))
        .context("비트맵")?;
        CGContextDrawImage(
            ctx.0,
            CGRect {
                x: 0.0,
                y: 0.0,
                w: w as f64,
                h: h as f64,
            },
            img.0,
        );
    }
    Ok((buf, w, h))
}

/// RGBX 픽셀을 파일로 저장 (uti 예: "public.jpeg", "public.heic", "public.png", "public.tiff").
/// embed_thumb면 카메라처럼 작은 썸네일을 파일 안에 넣는다. orientation은 EXIF 회전 값(1~8).
pub fn write_rgbx(
    path: &Path,
    rgbx: &mut [u8],
    w: u32,
    h: u32,
    uti: &str,
    quality: f64,
    embed_thumb: bool,
    orientation: u32,
) -> anyhow::Result<()> {
    unsafe {
        let cs = Cf::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB)).context("색 공간")?;
        let ctx = Cf::new(CGBitmapContextCreate(
            rgbx.as_mut_ptr() as _,
            w as usize,
            h as usize,
            8,
            w as usize * 4,
            cs.0,
            kCGImageAlphaNoneSkipLast | kCGBitmapByteOrder32Big,
        ))
        .context("비트맵")?;
        let img = Cf::new(CGBitmapContextCreateImage(ctx.0)).context("이미지")?;
        let bytes = path.as_os_str().as_bytes();
        let url = Cf::new(CFURLCreateFromFileSystemRepresentation(
            kCFAllocatorDefault,
            bytes.as_ptr(),
            bytes.len() as CFIndex,
            0,
        ))
        .context("경로")?;
        let ty = cfstr(uti);
        let dest = Cf::new(CGImageDestinationCreateWithURL(url.0, ty.0, 1, null()))
            .context("이 형식으로 저장할 수 없음")?;
        let q = num_f64(quality);
        let o = num_i64(orientation as i64);
        let thumb = if embed_thumb { kCFBooleanTrue } else { kCFBooleanFalse };
        let props = dict(&[
            (kCGImageDestinationLossyCompressionQuality, q.0),
            (kCGImageDestinationEmbedThumbnail, thumb),
            (kCGImagePropertyOrientation, o.0),
        ]);
        CGImageDestinationAddImage(dest.0, img.0, props.0);
        if !CGImageDestinationFinalize(dest.0) {
            bail!("저장 실패");
        }
    }
    Ok(())
}

// ───────────────────────── 변환 (2단계) ─────────────────────────

/// 디코딩·축소·회전·워터마크까지 끝나고 저장만 남은 사진. 같은 사진을 여러 화질로 저장해 볼 때 다시 디코딩하지 않는다.
pub struct Prepared {
    img: Cf,
    props: Option<Cf>,
    pub width: u32,
    pub height: u32,
}

/// 워터마크를 사진 위에 놓을 위치 (위에서부터 잰 좌표)
pub struct Overlay<'a> {
    pub png: &'a [u8],
    /// 사진 크기를 받아 (x, y, 가로, 세로)를 돌려준다
    pub place: &'a dyn Fn(u32, u32, u32, u32) -> (f64, f64, f64, f64),
    pub opacity: f64,
}

/// 원본의 회전 값 (EXIF Orientation, 없으면 1)
pub fn raw_orientation(path: &Path) -> u16 {
    let Ok(src) = source(path) else { return 1 };
    let Some(props) = Cf::new(unsafe { CGImageSourceCopyPropertiesAtIndex(src.0, 0, null()) }) else {
        return 1;
    };
    get_i64(props.0, unsafe { kCGImagePropertyOrientation }).unwrap_or(1) as u16
}

/// target_long: 긴 변 픽셀 수 (0이면 원본 크기). turn: 추가로 돌릴 회전 값(1이면 그대로).
pub fn prepare(path: &Path, target_long: u32, turn: u16, overlay: Option<&Overlay>) -> anyhow::Result<Prepared> {
    let src = source(path)?;
    let props = Cf::new(unsafe { CGImageSourceCopyPropertiesAtIndex(src.0, 0, null()) });
    let (fw, fh) = dimensions(path)?;
    let long = fw.max(fh);
    let target = if target_long == 0 || target_long >= long { long } else { target_long };
    let mut img = create_scaled(&src, target, true).context("디코딩 실패")?;
    if turn != 1 || overlay.is_some() {
        img = redraw(&img, turn, overlay)?;
    }
    let (width, height) = size_of(&img);
    Ok(Prepared { img, props, width, height })
}

/// 회전 값(2~8)에 해당하는 변환을 위로 향한 좌표계(y가 위로)에서 표현한 행렬.
/// 원본 크기 w×h를 기준으로 하며, 5~8은 결과 크기가 h×w가 된다.
fn turn_transform(turn: u16, w: f64, h: f64) -> CGAffineTransform {
    let t = |a, b, c, d, tx, ty| CGAffineTransform { a, b, c, d, tx, ty };
    match turn {
        2 => t(-1.0, 0.0, 0.0, 1.0, w, 0.0),
        3 => t(-1.0, 0.0, 0.0, -1.0, w, h),
        4 => t(1.0, 0.0, 0.0, -1.0, 0.0, h),
        5 => t(0.0, -1.0, -1.0, 0.0, h, w),
        6 => t(0.0, -1.0, 1.0, 0.0, 0.0, w),
        7 => t(0.0, 1.0, 1.0, 0.0, 0.0, 0.0),
        8 => t(0.0, 1.0, -1.0, 0.0, h, 0.0),
        _ => t(1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
    }
}

fn redraw(img: &Cf, turn: u16, overlay: Option<&Overlay>) -> anyhow::Result<Cf> {
    let (w, h) = size_of(img);
    let (nw, nh) = if turn >= 5 { (h, w) } else { (w, h) };
    unsafe {
        // 원본 색 공간을 그대로 쓴다 (아이폰 Display P3 유지). RGB가 아니면 sRGB.
        let own_srgb;
        let mut cs = CGImageGetColorSpace(img.0);
        if cs.is_null() || CGColorSpaceGetModel(cs) != 1 {
            own_srgb = Cf::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB)).context("색 공간")?;
            cs = own_srgb.0;
        }
        let info = if has_alpha(img) {
            kCGImageAlphaPremultipliedLast
        } else {
            kCGImageAlphaNoneSkipLast
        } | kCGBitmapByteOrder32Big;
        let ctx = Cf::new(CGBitmapContextCreate(std::ptr::null_mut(), nw as usize, nh as usize, 8, 0, cs, info))
            .context("비트맵을 만들 수 없음")?;
        CGContextSetInterpolationQuality(ctx.0, 3);
        CGContextSaveGState(ctx.0);
        CGContextConcatCTM(ctx.0, turn_transform(turn, w as f64, h as f64));
        CGContextDrawImage(ctx.0, CGRect { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }, img.0);
        CGContextRestoreGState(ctx.0);
        if let Some(o) = overlay {
            let data = Cf::new(CFDataCreate(kCFAllocatorDefault, o.png.as_ptr(), o.png.len() as CFIndex)).context("메모리")?;
            let osrc = Cf::new(CGImageSourceCreateWithData(data.0, null())).context("워터마크 이미지 오류")?;
            let oimg = Cf::new(CGImageSourceCreateImageAtIndex(osrc.0, 0, null())).context("워터마크 이미지 오류")?;
            let (ow, oh) = size_of(&oimg);
            let (x, y_top, dw, dh) = (o.place)(nw, nh, ow, oh);
            CGContextSetAlpha(ctx.0, o.opacity);
            // CG는 아래에서부터 재므로 위 기준 좌표를 뒤집는다
            CGContextDrawImage(ctx.0, CGRect { x, y: nh as f64 - y_top - dh, w: dw, h: dh }, oimg.0);
        }
        Cf::new(CGBitmapContextCreateImage(ctx.0)).context("이미지를 만들 수 없음")
    }
}

/// 편집기에서 그린 결과(원본 해상도 RGBA, 회전 적용됨, sRGB)를 원본의 촬영 정보와 함께 저장할 준비.
pub fn prepare_pixels(src_path: &Path, px: &super::Pixels) -> anyhow::Result<Prepared> {
    let props = source(src_path)
        .ok()
        .and_then(|src| Cf::new(unsafe { CGImageSourceCopyPropertiesAtIndex(src.0, 0, null()) }));
    let (w, h) = (px.width, px.height);
    anyhow::ensure!(px.rgba.len() == w as usize * h as usize * 4, "픽셀 크기가 맞지 않음");
    let mut buf = px.rgba.clone();
    // 투명한 곳이 있으면 CG가 요구하는 미리 곱한 알파로 바꾼다
    let alpha = buf.chunks_exact(4).any(|p| p[3] != 255);
    if alpha {
        for p in buf.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    let info = if alpha { kCGImageAlphaPremultipliedLast } else { kCGImageAlphaNoneSkipLast } | kCGBitmapByteOrder32Big;
    unsafe {
        let cs = Cf::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB)).context("색 공간")?;
        let ctx = Cf::new(CGBitmapContextCreate(buf.as_mut_ptr() as _, w as usize, h as usize, 8, w as usize * 4, cs.0, info))
            .context("비트맵")?;
        // CGBitmapContextCreateImage는 내용을 복사하므로 buf는 여기서 놓아도 된다
        let img = Cf::new(CGBitmapContextCreateImage(ctx.0)).context("이미지")?;
        Ok(Prepared { img, props, width: w, height: h })
    }
}

/// 원본 메타데이터(촬영 정보 등)를 이어받되, 회전 값은 1로(이미 돌렸으므로), 크기는 새 값으로.
fn make_props(p: &Prepared, quality: f64, strip_gps: bool) -> Cf {
    unsafe {
        let m = match &p.props {
            Some(src) => Cf(CFDictionaryCreateMutableCopy(kCFAllocatorDefault, 0, src.0)),
            None => Cf(CFDictionaryCreateMutable(
                kCFAllocatorDefault,
                0,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )),
        };
        let one = num_i64(1);
        CFDictionarySetValue(m.0, kCGImagePropertyOrientation, one.0);
        let tiff = CFDictionaryGetValue(m.0, kCGImagePropertyTIFFDictionary);
        if !tiff.is_null() {
            let t = Cf(CFDictionaryCreateMutableCopy(kCFAllocatorDefault, 0, tiff));
            CFDictionarySetValue(t.0, kCGImagePropertyTIFFOrientation, one.0);
            CFDictionarySetValue(m.0, kCGImagePropertyTIFFDictionary, t.0);
        }
        let exif = CFDictionaryGetValue(m.0, kCGImagePropertyExifDictionary);
        if !exif.is_null() {
            let e = Cf(CFDictionaryCreateMutableCopy(kCFAllocatorDefault, 0, exif));
            let (w, h) = (num_i64(p.width as i64), num_i64(p.height as i64));
            CFDictionarySetValue(e.0, kCGImagePropertyExifPixelXDimension, w.0);
            CFDictionarySetValue(e.0, kCGImagePropertyExifPixelYDimension, h.0);
            CFDictionarySetValue(m.0, kCGImagePropertyExifDictionary, e.0);
        }
        if strip_gps {
            CFDictionaryRemoveValue(m.0, kCGImagePropertyGPSDictionary);
        }
        let q = num_f64(quality);
        CFDictionarySetValue(m.0, kCGImageDestinationLossyCompressionQuality, q.0);
        m
    }
}

/// uti 형식으로 메모리에 저장 (quality 0~1).
pub fn encode_prepared(p: &Prepared, uti: &str, quality: f64, strip_gps: bool) -> anyhow::Result<Vec<u8>> {
    let data = Cf::new(unsafe { CFDataCreateMutable(kCFAllocatorDefault, 0) }).context("메모리")?;
    let ty = cfstr(uti);
    let dest = Cf::new(unsafe { CGImageDestinationCreateWithData(data.0, ty.0, 1, null()) })
        .context("이 형식으로 저장할 수 없음")?;
    let props = make_props(p, quality, strip_gps);
    unsafe { CGImageDestinationAddImage(dest.0, p.img.0, props.0) };
    if !unsafe { CGImageDestinationFinalize(dest.0) } {
        bail!("저장 실패");
    }
    Ok(unsafe { std::slice::from_raw_parts(CFDataGetBytePtr(data.0), CFDataGetLength(data.0) as usize) }.to_vec())
}

/// 압축된 사진 데이터는 그대로 두고 회전 값만 바꿔 복사 (HEIC·TIFF 무손실 회전).
pub fn copy_with_orientation(src_path: &Path, dst_path: &Path, orientation: u16) -> anyhow::Result<()> {
    let src = source(src_path)?;
    let ty = unsafe { CGImageSourceGetType(src.0) };
    if ty.is_null() {
        bail!("형식을 알 수 없음");
    }
    let bytes = dst_path.as_os_str().as_bytes();
    let url = Cf::new(unsafe {
        CFURLCreateFromFileSystemRepresentation(kCFAllocatorDefault, bytes.as_ptr(), bytes.len() as CFIndex, 0)
    })
    .context("경로")?;
    let dest = Cf::new(unsafe { CGImageDestinationCreateWithURL(url.0, ty, 1, null()) }).context("저장할 수 없음")?;
    let o = num_i64(orientation as i64);
    let opts = dict(&[(unsafe { kCGImageDestinationOrientation }, o.0)]);
    if !unsafe { CGImageDestinationCopyImageSource(dest.0, src.0, opts.0, std::ptr::null_mut()) } {
        let _ = std::fs::remove_file(dst_path);
        bail!("무손실 회전을 지원하지 않는 파일");
    }
    Ok(())
}

pub fn dimensions(path: &Path) -> anyhow::Result<(u32, u32)> {
    let src = source(path)?;
    let props = Cf::new(unsafe { CGImageSourceCopyPropertiesAtIndex(src.0, 0, null()) })
        .context("속성 없음")?;
    let w = get_i64(props.0, unsafe { kCGImagePropertyPixelWidth }).context("너비 없음")?;
    let h = get_i64(props.0, unsafe { kCGImagePropertyPixelHeight }).context("높이 없음")?;
    let o = get_i64(props.0, unsafe { kCGImagePropertyOrientation }).unwrap_or(1);
    Ok(if (5..=8).contains(&o) {
        (h as u32, w as u32)
    } else {
        (w as u32, h as u32)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 깨진 파일·가짜 확장자는 에러로 끝나야 한다 (앱이 꺼지면 안 됨)
    #[test]
    fn broken_files_do_not_crash() {
        let dir = std::env::temp_dir().join(format!("isee-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases: [(&str, &[u8]); 6] = [
            ("fake.arw", b"RAW"),
            ("empty.jpg", b""),
            ("text.png", b"hello, not a png"),
            ("trunc.jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0]),
            ("fake.heic", b"\0\0\0\x18ftypheic\0\0\0\0mif1heic"),
            ("fake.cr2", b"II*\0\x08\0\0\0"),
        ];
        // 실제 JPEG를 앞부분만 남기고 자른 것 (다운로드가 덜 된 파일)
        let full = dir.join("full.jpg");
        let mut px = vec![128u8; 640 * 480 * 4];
        write_rgbx(&full, &mut px, 640, 480, "public.jpeg", 0.8, false, 1).unwrap();
        let bytes = std::fs::read(&full).unwrap();
        for cut in [20, 200, bytes.len() / 2] {
            let p = dir.join(format!("cut{cut}.jpg"));
            std::fs::write(&p, &bytes[..cut]).unwrap();
            eprintln!("cut {cut}/{}", bytes.len());
            // 절반 남은 것은 보이는 데까지 그려도 되고 에러여도 된다. 꺼지지만 않으면 된다.
            let _ = thumbnail(&p, 400);
            let _ = screen_rgba(&p, 1024);
            let _ = crate::decode::thumbnail(&p, 400);
            let _ = crate::decode::screen_rgba(&p, 1024);
            let _ = crate::viewer::prepare(&p, 1024, 768);
        }
        for (name, bytes) in cases {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            eprintln!("{name}");
            assert!(thumbnail(&p, 400).is_err(), "{name}");
            assert!(screen_rgba(&p, 1024).is_err(), "{name}");
            // 화면이 실제로 쓰는 경로 (형식별 디코더 선택, 뷰어 준비, 비슷한 사진 분석, 날짜 읽기)
            assert!(crate::decode::thumbnail(&p, 400).is_err(), "{name}");
            assert!(crate::decode::screen_rgba(&p, 1024).is_err(), "{name}");
            assert!(crate::viewer::prepare(&p, 1024, 768).is_err(), "{name}");
            assert!(crate::analysis::compute(&p, 0).is_err(), "{name}");
            let _ = crate::meta::read(&p, 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
