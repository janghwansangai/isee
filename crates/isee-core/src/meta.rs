//! 사진·동영상의 촬영 날짜와 위치 (날짜·장소별 정리용).
//! 날짜를 찾는 순서: EXIF 촬영 시각 → 동영상 생성 시각 → 파일 이름 속 날짜 → 파일 수정 시각.
//! 시각은 모두 "찍은 곳의 현지 시각"을 1970년부터의 초로 센다 (시간대 없이, 폴더 이름에 쓸 날짜 그대로).

use serde::Serialize;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DateSource {
    Exif,
    Video,
    FileName,
    /// 마지막 수단 (화면에 따로 표시)
    Modified,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    /// 현지 시각 (초)
    pub time: i64,
    pub source: DateSource,
    /// (위도, 경도)
    pub gps: Option<(f64, f64)>,
}

/// 사진·동영상 하나의 날짜와 위치. mtime_ms: 파일 수정 시각.
pub fn read(path: &Path, mtime_ms: i64) -> Meta {
    let video = crate::formats::kind_of(path) == crate::formats::Kind::Video;
    let (mut time, mut gps, mut source) = (None, None, DateSource::Modified);
    // 아직 내려받지 않은 클라우드 파일은 내용을 읽지 않는다 (파일 이름 → 수정 시각으로만)
    if crate::listing::is_online_only(path) {
    } else if video {
        if let Some(v) = mp4_meta(path) {
            time = v.0;
            gps = v.1;
            source = DateSource::Video;
        }
    } else if let Some(e) = exif_meta(path) {
        time = e.0;
        gps = e.1;
        source = DateSource::Exif;
    }
    if time.is_none() {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        time = name_date(&stem);
        source = DateSource::FileName;
    }
    let time = time.unwrap_or_else(|| {
        source = DateSource::Modified;
        utc_to_local(mtime_ms.div_euclid(1000))
    });
    Meta { time, source, gps }
}

// ───────────────────────── 날짜 계산 ─────────────────────────

/// 날짜 → 1970-01-01부터 날 수 (Howard Hinnant의 days_from_civil)
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 초 → (년, 월, 일)
pub fn civil(secs: i64) -> (i64, u32, u32) {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m as u32, d as u32)
}

fn local_secs(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    let ok = (1990..=2099).contains(&y)
        && (1..=12).contains(&mo)
        && (1..=31).contains(&d)
        && (0..24).contains(&h)
        && (0..60).contains(&mi)
        && (0..61).contains(&s);
    ok.then(|| days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// UTC 초 → 이 컴퓨터 시간대의 현지 시각 초
fn utc_to_local(secs: i64) -> i64 {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(secs, 0).single() {
        Some(t) => t.naive_local().and_utc().timestamp(),
        None => secs,
    }
}

/// "2024:05:12 14:03:22", "2024-05-12T14:30:22+0900" 등 → 현지 시각
fn parse_datetime(s: &str) -> Option<i64> {
    let n: Vec<i64> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .take(6)
        .filter_map(|p| p.parse().ok())
        .collect();
    if n.len() < 3 {
        return None;
    }
    let g = |i: usize| n.get(i).copied().unwrap_or(0);
    local_secs(g(0), g(1), g(2), g(3), g(4), g(5))
}

// ───────────────────────── 사진 EXIF ─────────────────────────

/// EXIF 촬영 시각과 GPS. 둘 다 없으면 None.
pub fn exif_meta(path: &Path) -> Option<(Option<i64>, Option<(f64, f64)>)> {
    let mut r = BufReader::new(File::open(path).ok()?);
    let exif = exif::Reader::new().read_from_container(&mut r).ok()?;
    let ascii = |tag| match &exif.get_field(tag, exif::In::PRIMARY)?.value {
        exif::Value::Ascii(v) => v.first().map(|b| String::from_utf8_lossy(b).trim().to_string()),
        _ => None,
    };
    // 촬영 시각 → 디지털화 시각 → 마지막으로 저장한 시각
    let time = [exif::Tag::DateTimeOriginal, exif::Tag::DateTimeDigitized, exif::Tag::DateTime]
        .into_iter()
        .find_map(|t| ascii(t).and_then(|s| parse_datetime(&s)));
    let coord = |tag, ref_tag, neg: u8| -> Option<f64> {
        let exif::Value::Rational(v) = &exif.get_field(tag, exif::In::PRIMARY)?.value else {
            return None;
        };
        let part = |i: usize| v.get(i).map(|r| r.to_f64()).filter(|x| x.is_finite()).unwrap_or(0.0);
        let deg = part(0) + part(1) / 60.0 + part(2) / 3600.0;
        let r = ascii(ref_tag).and_then(|s| s.bytes().next());
        Some(if r == Some(neg) { -deg } else { deg })
    };
    let gps = match (
        coord(exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef, b'S'),
        coord(exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef, b'W'),
    ) {
        (Some(la), Some(lo)) if !(la == 0.0 && lo == 0.0) => Some((la, lo)),
        _ => None,
    };
    (time.is_some() || gps.is_some()).then_some((time, gps))
}

// ───────────────────────── 동영상 (MP4·MOV) ─────────────────────────

/// "+37.5665+126.9780+050.000/" → (37.5665, 126.978)
fn iso6709(s: &str) -> Option<(f64, f64)> {
    let mut nums = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && nums.len() < 2 {
        if b[i] == b'+' || b[i] == b'-' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                j += 1;
            }
            nums.push(s[i..j].parse::<f64>().ok()?);
            i = j;
        } else {
            i += 1;
        }
    }
    match nums[..] {
        [la, lo] if la.abs() <= 90.0 && lo.abs() <= 180.0 && !(la == 0.0 && lo == 0.0) => Some((la, lo)),
        _ => None,
    }
}

/// 상자(box) 목록: (종류, 내용)
fn boxes(d: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 8 <= d.len() {
        let mut size = u32::from_be_bytes(d[p..p + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = d[p + 4..p + 8].try_into().unwrap();
        let mut head = 8;
        if size == 1 && p + 16 <= d.len() {
            size = u64::from_be_bytes(d[p + 8..p + 16].try_into().unwrap()) as usize;
            head = 16;
        } else if size == 0 {
            size = d.len() - p;
        }
        if size < head || p + size > d.len() {
            break;
        }
        out.push((kind, &d[p + head..p + size]));
        p += size;
    }
    out
}

fn child<'a>(d: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(d).into_iter().find(|(k, _)| k == kind).map(|(_, v)| v)
}

/// QuickTime·MP4의 meta 상자: 애플 형식은 버전 4바이트가 없고, ISO 형식은 있다
fn meta_children(meta: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let plain = boxes(meta);
    if plain.iter().any(|(k, _)| k == b"hdlr" || k == b"keys") {
        return plain;
    }
    boxes(meta.get(4..).unwrap_or_default())
}

/// moov 상자 내용에서 (현지 촬영 시각, GPS)
fn parse_moov(moov: &[u8]) -> (Option<i64>, Option<(f64, f64)>) {
    let (mut time, mut gps) = (None, None);
    // 1) 애플 메타데이터: 현지 시각과 위치가 그대로 들어 있다
    if let Some(meta) = child(moov, b"meta").or_else(|| child(moov, b"udta").and_then(|u| child(u, b"meta"))) {
        let kids = meta_children(meta);
        let keys: Vec<String> = kids
            .iter()
            .find(|(k, _)| k == b"keys")
            .map(|(_, v)| {
                let mut out = Vec::new();
                let mut p = 8; // 버전·플래그 4 + 개수 4
                while p + 8 <= v.len() {
                    let size = u32::from_be_bytes(v[p..p + 4].try_into().unwrap()) as usize;
                    if size < 8 || p + size > v.len() {
                        break;
                    }
                    out.push(String::from_utf8_lossy(&v[p + 8..p + size]).into_owned());
                    p += size;
                }
                out
            })
            .unwrap_or_default();
        if let Some((_, ilst)) = kids.iter().find(|(k, _)| k == b"ilst") {
            for (k, item) in boxes(ilst) {
                let idx = u32::from_be_bytes(k) as usize;
                let Some(key) = idx.checked_sub(1).and_then(|i| keys.get(i)) else { continue };
                let Some(data) = child(item, b"data") else { continue };
                let text = String::from_utf8_lossy(data.get(8..).unwrap_or_default());
                match key.as_str() {
                    "com.apple.quicktime.creationdate" => time = time.or_else(|| parse_datetime(&text)),
                    "com.apple.quicktime.location.ISO6709" => gps = gps.or_else(|| iso6709(&text)),
                    _ => {}
                }
            }
        }
    }
    // 2) udta/©xyz (안드로이드·여러 카메라)
    if gps.is_none()
        && let Some(xyz) = child(moov, b"udta").and_then(|u| child(u, &[0xA9, b'x', b'y', b'z']))
    {
        gps = iso6709(&String::from_utf8_lossy(xyz.get(4..).unwrap_or_default()));
    }
    // 3) mvhd 생성 시각 (1904년부터 초, UTC)
    if time.is_none()
        && let Some(mvhd) = child(moov, b"mvhd")
    {
        let secs = match mvhd.first() {
            Some(1) if mvhd.len() >= 12 => u64::from_be_bytes(mvhd[4..12].try_into().unwrap()) as i64,
            Some(0) if mvhd.len() >= 8 => u32::from_be_bytes(mvhd[4..8].try_into().unwrap()) as i64,
            _ => 0,
        };
        let unix = secs - 2_082_844_800;
        // 0이나 2000년 이전은 카메라가 시각을 비워 둔 것
        if unix > 946_684_800 {
            time = Some(utc_to_local(unix));
        }
    }
    (time, gps)
}

/// MP4·MOV의 (현지 촬영 시각, GPS). moov 상자만 읽는다 (파일 끝에 있어도 건너뛰어 찾음).
pub fn mp4_meta(path: &Path) -> Option<(Option<i64>, Option<(f64, f64)>)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut size = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
        let mut head = 8;
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
            head = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < head {
            return None;
        }
        if &h[4..8] == b"moov" {
            // moov는 보통 수 MB 이하. 비정상적으로 크면 읽지 않는다
            if size > 64 << 20 {
                return None;
            }
            let mut moov = vec![0u8; (size - head) as usize];
            f.read_exact(&mut moov).ok()?;
            let r = parse_moov(&moov);
            return (r.0.is_some() || r.1.is_some()).then_some(r);
        }
        pos += size;
    }
    None
}

// ───────────────────────── 파일 이름 속 날짜 ─────────────────────────

/// IMG_20240512_143022, KakaoTalk_20240512_143022123, Screenshot_2024-05-12-14-30-22,
/// 스크린샷 2024-05-12 오후 2.30.22, FB_IMG_1715501422123 (밀리초) 등 → 현지 시각
pub fn name_date(stem: &str) -> Option<i64> {
    // 숫자 덩어리: (시작 위치, 숫자열)
    let mut runs: Vec<(usize, &str)> = Vec::new();
    let mut start = None;
    for (i, c) in stem.char_indices().chain(std::iter::once((stem.len(), ' '))) {
        match (c.is_ascii_digit(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push((s, &stem[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let pm = stem.contains("오후") || stem.to_ascii_lowercase().contains(" pm");
    let am = stem.contains("오전") || stem.to_ascii_lowercase().contains(" am");
    let hour = |h: i64| if pm && h < 12 { h + 12 } else if am && h == 12 { 0 } else { h };
    for (i, &(_, run)) in runs.iter().enumerate() {
        // 1) 20240512 (뒤에 143022가 붙거나 따로 오면 시각)
        if run.len() >= 8 && run.len() != 13 {
            let (y, mo, d) = (num(&run[0..4])?, num(&run[4..6])?, num(&run[6..8])?);
            let time = if run.len() >= 14 {
                Some(&run[8..14])
            } else {
                runs.get(i + 1).map(|r| r.1).filter(|r| r.len() >= 6).map(|r| &r[..6])
            };
            let (h, mi, s) = time
                .and_then(|t| Some((num(&t[0..2])?, num(&t[2..4])?, num(&t[4..6])?)))
                .filter(|&(h, mi, s)| h < 24 && mi < 60 && s < 60)
                .unwrap_or((0, 0, 0));
            if let Some(t) = local_secs(y, mo, d, h, mi, s) {
                return Some(t);
            }
        }
        // 2) 2024-05-12 (뒤에 14.30.22 또는 143022)
        if run.len() == 4
            && let (Some(m), Some(d)) = (runs.get(i + 1), runs.get(i + 2))
            && (1..=2).contains(&m.1.len())
            && (1..=2).contains(&d.1.len())
            && m.0 - (runs[i].0 + 4) <= 1
            && d.0 - (m.0 + m.1.len()) <= 1
        {
            let rest: Vec<i64> = runs[i + 3..].iter().take(3).filter_map(|r| num(r.1)).collect();
            let (h, mi, s) = match (runs.get(i + 3).map(|r| r.1.len()), &rest[..]) {
                (Some(6), [t, ..]) => (t / 10_000, t / 100 % 100, t % 100),
                (_, [h, mi, s, ..]) => (*h, *mi, *s),
                (_, [h, mi]) => (*h, *mi, 0),
                _ => (0, 0, 0),
            };
            let (h, mi, s) = if h < 24 && mi < 60 && s < 60 { (hour(h), mi, s) } else { (0, 0, 0) };
            if let Some(t) = local_secs(num(run)?, num(m.1)?, num(d.1)?, h, mi, s) {
                return Some(t);
            }
        }
        // 3) 1715501422123 (유닉스 밀리초)
        if run.len() == 13
            && let Some(ms) = num(run)
            && (946_684_800_000..4_102_444_800_000).contains(&ms)
        {
            return Some(utc_to_local(ms / 1000));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymdhms(t: i64) -> String {
        let (y, m, d) = civil(t);
        let r = t.rem_euclid(86_400);
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", r / 3600, r % 3600 / 60, r % 60)
    }

    #[test]
    fn names() {
        let n = |s: &str| name_date(s).map(ymdhms);
        assert_eq!(n("IMG_20240512_143022").as_deref(), Some("2024-05-12 14:30:22"));
        assert_eq!(n("KakaoTalk_20240512_143022123").as_deref(), Some("2024-05-12 14:30:22"));
        assert_eq!(n("KakaoTalk_Photo_2024-05-12-14-30-22").as_deref(), Some("2024-05-12 14:30:22"));
        assert_eq!(n("Screenshot_2024-05-12-14-30-22-123_com.app").as_deref(), Some("2024-05-12 14:30:22"));
        assert_eq!(n("스크린샷 2024-05-12 오후 2.30.22").as_deref(), Some("2024-05-12 14:30:22"));
        assert_eq!(n("Screenshot 2024-05-12 at 09.05.01").as_deref(), Some("2024-05-12 09:05:01"));
        assert_eq!(n("PXL_20240512_053022123.MP").as_deref(), Some("2024-05-12 05:30:22"));
        assert_eq!(n("20240512143022").as_deref(), Some("2024-05-12 14:30:22"));
        assert_eq!(n("photo_2024.05.12").as_deref(), Some("2024-05-12 00:00:00"));
        assert!(n("FB_IMG_1715501422123").is_some_and(|s| s.starts_with("2024-05-1")));
        assert_eq!(n("IMG_1234"), None);
        assert_eq!(n("DSC09876"), None);
        assert_eq!(n("12345678"), None); // 1234년은 날짜가 아님
    }

    #[test]
    fn datetimes() {
        assert_eq!(parse_datetime("2024:05:12 14:03:22").map(ymdhms).as_deref(), Some("2024-05-12 14:03:22"));
        assert_eq!(parse_datetime("2024-05-12T14:30:22+0900").map(ymdhms).as_deref(), Some("2024-05-12 14:30:22"));
        assert_eq!(parse_datetime("0000:00:00 00:00:00"), None);
        assert_eq!(civil(days_from_civil(2024, 2, 29) * 86_400), (2024, 2, 29));
    }

    #[test]
    fn iso6709_strings() {
        assert_eq!(iso6709("+37.5665+126.9780+050.000/"), Some((37.5665, 126.978)));
        assert_eq!(iso6709("-33.8568+151.2153/"), Some((-33.8568, 151.2153)));
        assert_eq!(iso6709("+00.0000+000.0000/"), None);
    }

    fn bx(kind: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn mp4_apple_and_mvhd() {
        // 아이폰 형식: moov/meta(keys, ilst) + 다른 동영상: moov/mvhd + udta/©xyz
        let key = |s: &str| bx(b"mdta", s.as_bytes());
        let mut keys = vec![0u8; 4];
        keys.extend_from_slice(&2u32.to_be_bytes());
        keys.extend(key("com.apple.quicktime.location.ISO6709"));
        keys.extend(key("com.apple.quicktime.creationdate"));
        let data = |s: &str| bx(b"data", &[&[0, 0, 0, 1, 0, 0, 0, 0][..], s.as_bytes()].concat());
        let ilst = [bx(&1u32.to_be_bytes(), &data("+33.2489+126.4106+010.000/")), bx(&2u32.to_be_bytes(), &data("2024-05-12T14:30:22+0900"))].concat();
        let meta = [bx(b"hdlr", &[0; 24]), bx(b"keys", &keys), bx(b"ilst", &ilst)].concat();
        let moov = bx(b"meta", &meta);
        let (t, g) = parse_moov(&moov);
        assert_eq!(t.map(ymdhms).as_deref(), Some("2024-05-12 14:30:22"));
        assert_eq!(g, Some((33.2489, 126.4106)));

        let mut mvhd = vec![0u8; 4];
        mvhd.extend_from_slice(&((1_715_522_602i64 + 2_082_844_800) as u32).to_be_bytes());
        mvhd.extend_from_slice(&[0; 8]);
        let xyz = [&[0u8, 18, 0x15, 0xc7][..], b"+48.8584+002.2945/"].concat();
        let moov = [bx(b"mvhd", &mvhd), bx(b"udta", &bx(&[0xA9, b'x', b'y', b'z'], &xyz))].concat();
        let (t, g) = parse_moov(&moov);
        assert!(t.is_some());
        assert_eq!(g, Some((48.8584, 2.2945)));
    }
}
