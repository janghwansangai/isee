//! JPEG 안의 EXIF(TIFF 블록)를 픽셀을 건드리지 않고 고친다.
//! - 회전 값(Orientation)만 바꾸는 무손실 회전
//! - 다시 인코딩할 때 쓸 EXIF 정리: 회전 값 1로, 옛 썸네일 제거, 크기 갱신, GPS 지우기

use anyhow::{Context, bail};

const TAG_ORIENTATION: u16 = 0x0112;
const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_GPS_IFD: u16 = 0x8825;
const TAG_PIXEL_X: u16 = 0xA002;
const TAG_PIXEL_Y: u16 = 0xA003;

struct Seg {
    /// APP1 마커(FF E1)의 위치
    start: usize,
    /// TIFF 블록 시작 ("Exif\0\0" 다음)
    tiff: usize,
    /// 세그먼트 끝
    end: usize,
}

fn be16(d: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?) as usize)
}

/// EXIF가 들어 있는 APP1 세그먼트 찾기. 압축 데이터(SOS) 전까지만 본다.
fn find_exif(jpeg: &[u8]) -> Option<Seg> {
    if jpeg.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = 2;
    while pos + 4 <= jpeg.len() {
        if jpeg[pos] != 0xFF {
            return None;
        }
        let marker = jpeg[pos + 1];
        match marker {
            0xFF => {
                pos += 1;
                continue;
            }
            0xD8 | 0x01 | 0xD0..=0xD7 => {
                pos += 2;
                continue;
            }
            0xDA | 0xD9 => return None,
            _ => {}
        }
        let end = pos + 2 + be16(jpeg, pos + 2)?;
        if marker == 0xE1 && jpeg.get(pos + 4..pos + 10) == Some(b"Exif\0\0") {
            return Some(Seg { start: pos, tiff: pos + 10, end: end.min(jpeg.len()) });
        }
        pos = end;
    }
    None
}

/// JPEG 안의 TIFF 블록 (EXIF 원본). 없으면 None.
pub fn exif_tiff(jpeg: &[u8]) -> Option<&[u8]> {
    let s = find_exif(jpeg)?;
    jpeg.get(s.tiff..s.end)
}

/// TIFF 블록을 읽고 쓰는 도우미 (바이트 순서 처리)
struct Tiff<'a> {
    d: &'a mut [u8],
    be: bool,
}

impl<'a> Tiff<'a> {
    fn new(d: &'a mut [u8]) -> Option<Self> {
        let be = match d.get(..2)? {
            b"MM" => true,
            b"II" => false,
            _ => return None,
        };
        Some(Self { d, be })
    }
    fn u16(&self, o: usize) -> Option<u16> {
        let b: [u8; 2] = self.d.get(o..o + 2)?.try_into().ok()?;
        Some(if self.be { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let b: [u8; 4] = self.d.get(o..o + 4)?.try_into().ok()?;
        Some(if self.be { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    }
    fn put16(&mut self, o: usize, v: u16) -> Option<()> {
        let b = if self.be { v.to_be_bytes() } else { v.to_le_bytes() };
        self.d.get_mut(o..o + 2)?.copy_from_slice(&b);
        Some(())
    }
    fn put32(&mut self, o: usize, v: u32) -> Option<()> {
        let b = if self.be { v.to_be_bytes() } else { v.to_le_bytes() };
        self.d.get_mut(o..o + 4)?.copy_from_slice(&b);
        Some(())
    }
    fn ifd0(&self) -> Option<usize> {
        Some(self.u32(4)? as usize)
    }
    /// IFD 안에서 tag 항목의 위치 (12바이트 항목의 시작)
    fn entry(&self, ifd: usize, tag: u16) -> Option<usize> {
        let n = self.u16(ifd)? as usize;
        (0..n)
            .map(|i| ifd + 2 + i * 12)
            .find(|&e| self.u16(e) == Some(tag))
    }
    /// SHORT/LONG 값 쓰기 (항목의 형식에 맞춰)
    fn set_uint(&mut self, entry: usize, v: u32) -> Option<()> {
        match self.u16(entry + 2)? {
            3 => self.put16(entry + 8, v.min(u16::MAX as u32) as u16),
            4 => self.put32(entry + 8, v),
            _ => None,
        }
    }
}

fn type_size(t: u16) -> usize {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

/// 현재 회전 값 (없으면 1)
pub fn orientation(jpeg: &[u8]) -> u16 {
    let Some(tiff) = exif_tiff(jpeg) else { return 1 };
    let mut buf = tiff.to_vec();
    let Some(t) = Tiff::new(&mut buf) else { return 1 };
    t.ifd0()
        .and_then(|ifd| t.entry(ifd, TAG_ORIENTATION))
        .and_then(|e| t.u16(e + 8))
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1)
}

/// 회전 값만 바꾼 JPEG. 압축된 사진 데이터는 한 바이트도 바뀌지 않는다 (무손실).
pub fn set_orientation(jpeg: &[u8], o: u16) -> anyhow::Result<Vec<u8>> {
    if !(1..=8).contains(&o) {
        bail!("잘못된 회전 값 {o}");
    }
    if jpeg.get(..2) != Some(&[0xFF, 0xD8]) {
        bail!("JPEG가 아님");
    }
    let Some(seg) = find_exif(jpeg) else {
        return Ok(insert_minimal_exif(jpeg, o));
    };
    let mut tiff = jpeg[seg.tiff..seg.end].to_vec();
    let mut t = Tiff::new(&mut tiff).context("EXIF 형식 오류")?;
    let ifd0 = t.ifd0().context("EXIF 형식 오류")?;
    if let Some(e) = t.entry(ifd0, TAG_ORIENTATION) {
        // 제자리 수정: 길이가 그대로이므로 2바이트만 바뀐다
        t.put16(e + 8, o).context("EXIF 형식 오류")?;
        let mut out = jpeg.to_vec();
        out[seg.tiff..seg.end].copy_from_slice(&tiff);
        return Ok(out);
    }
    // 회전 항목이 없으면 IFD0를 새로 만들어 TIFF 블록 끝에 붙인다 (기존 값의 위치는 그대로)
    let tiff = append_ifd0_with(&tiff, TAG_ORIENTATION, o).context("EXIF에 회전 값을 넣을 수 없음")?;
    Ok(replace_app1(jpeg, &seg, &tiff)?)
}

fn replace_app1(jpeg: &[u8], seg: &Seg, tiff: &[u8]) -> anyhow::Result<Vec<u8>> {
    let len = 2 + 6 + tiff.len();
    if len > 0xFFFF {
        bail!("EXIF가 너무 큼");
    }
    let mut out = Vec::with_capacity(jpeg.len() + 64);
    out.extend_from_slice(&jpeg[..seg.start]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(tiff);
    out.extend_from_slice(&jpeg[seg.end..]);
    Ok(out)
}

fn append_ifd0_with(tiff: &[u8], tag: u16, value: u16) -> Option<Vec<u8>> {
    let mut src = tiff.to_vec();
    let t = Tiff::new(&mut src)?;
    let be = t.be;
    let ifd0 = t.ifd0()?;
    let n = t.u16(ifd0)? as usize;
    let next = t.u32(ifd0 + 2 + n * 12)?;
    let mut entries: Vec<[u8; 12]> = (0..n)
        .map(|i| tiff[ifd0 + 2 + i * 12..ifd0 + 14 + i * 12].try_into().unwrap())
        .collect();
    let w16 = |v: u16| if be { v.to_be_bytes() } else { v.to_le_bytes() };
    let w32 = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
    let mut e = [0u8; 12];
    e[0..2].copy_from_slice(&w16(tag));
    e[2..4].copy_from_slice(&w16(3));
    e[4..8].copy_from_slice(&w32(1));
    e[8..10].copy_from_slice(&w16(value));
    entries.push(e);
    let tag_of = |e: &[u8; 12]| {
        let b = [e[0], e[1]];
        if be { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }
    };
    entries.sort_by_key(tag_of);
    let mut out = tiff.to_vec();
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let new_off = out.len() as u32;
    out.extend_from_slice(&w16(entries.len() as u16));
    for e in &entries {
        out.extend_from_slice(e);
    }
    out.extend_from_slice(&w32(next));
    out[4..8].copy_from_slice(&w32(new_off));
    Some(out)
}

/// EXIF가 없는 JPEG에 회전 값 하나만 든 EXIF를 넣는다 (JFIF APP0가 있으면 그 뒤에).
fn insert_minimal_exif(jpeg: &[u8], o: u16) -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"MM\0\x2A");
    tiff.extend_from_slice(&8u32.to_be_bytes());
    tiff.extend_from_slice(&1u16.to_be_bytes());
    tiff.extend_from_slice(&TAG_ORIENTATION.to_be_bytes());
    tiff.extend_from_slice(&3u16.to_be_bytes());
    tiff.extend_from_slice(&1u32.to_be_bytes());
    tiff.extend_from_slice(&o.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0u32.to_be_bytes());
    let mut at = 2;
    if jpeg.get(2..4) == Some(&[0xFF, 0xE0])
        && let Some(len) = be16(jpeg, 4)
    {
        at = 4 + len;
    }
    let mut out = Vec::with_capacity(jpeg.len() + tiff.len() + 10);
    out.extend_from_slice(&jpeg[..at]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    out.extend_from_slice(&jpeg[at..]);
    out
}

/// 다시 인코딩한 사진에 붙일 EXIF로 정리한다.
/// - 회전 값 1 (픽셀을 이미 돌렸으므로)
/// - 옛 썸네일(IFD1) 제거 (방향·크기가 달라졌으므로)
/// - EXIF 안의 가로·세로 갱신
/// - strip_gps면 위치 정보를 지운다 (값이 저장된 바이트까지 0으로)
pub fn tidy_for_reencode(tiff: &[u8], width: u32, height: u32, strip_gps: bool) -> Option<Vec<u8>> {
    let mut buf = tiff.to_vec();
    let mut t = Tiff::new(&mut buf)?;
    let ifd0 = t.ifd0()?;
    if let Some(e) = t.entry(ifd0, TAG_ORIENTATION) {
        t.put16(e + 8, 1)?;
    }
    let n = t.u16(ifd0)? as usize;
    t.put32(ifd0 + 2 + n * 12, 0)?;
    if let Some(e) = t.entry(ifd0, TAG_EXIF_IFD)
        && let Some(exif) = t.u32(e + 8)
    {
        let exif = exif as usize;
        if let Some(x) = t.entry(exif, TAG_PIXEL_X) {
            t.set_uint(x, width);
        }
        if let Some(y) = t.entry(exif, TAG_PIXEL_Y) {
            t.set_uint(y, height);
        }
    }
    if strip_gps && let Some(e) = t.entry(ifd0, TAG_GPS_IFD) {
        let gps = t.u32(e + 8)? as usize;
        let count = t.u16(gps).unwrap_or(0) as usize;
        for i in 0..count {
            let en = gps + 2 + i * 12;
            let (Some(ty), Some(cnt)) = (t.u16(en + 2), t.u32(en + 4)) else { break };
            let size = type_size(ty) * cnt as usize;
            if size > 4
                && let Some(off) = t.u32(en + 8)
                && let Some(data) = t.d.get_mut(off as usize..off as usize + size)
            {
                data.fill(0);
            }
            if let Some(entry) = t.d.get_mut(en..en + 12) {
                entry.fill(0);
            }
        }
        t.put16(gps, 0)?;
    }
    Some(buf)
}

/// 위치 정보가 들어 있는지
pub fn has_gps(tiff: &[u8]) -> bool {
    let mut buf = tiff.to_vec();
    let Some(t) = Tiff::new(&mut buf) else { return false };
    t.ifd0()
        .and_then(|ifd| t.entry(ifd, TAG_GPS_IFD))
        .and_then(|e| t.u32(e + 8))
        .and_then(|gps| t.u16(gps as usize))
        .is_some_and(|n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 가장 작은 JPEG 비슷한 바이트열 (SOI + 선택적 APP1 + SOS + 데이터 + EOI)
    fn fake_jpeg(app1_tiff: Option<&[u8]>) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        j.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        if let Some(t) = app1_tiff {
            j.extend_from_slice(&[0xFF, 0xE1]);
            j.extend_from_slice(&((t.len() + 8) as u16).to_be_bytes());
            j.extend_from_slice(b"Exif\0\0");
            j.extend_from_slice(t);
        }
        j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x12, 0x34, 0x56, 0xFF, 0xD9]);
        j
    }

    /// 리틀 엔디언 TIFF: IFD0에 Make(문자열 오프셋)와, 선택적으로 Orientation
    fn tiff_le(with_orient: Option<u16>) -> Vec<u8> {
        let mut t = b"II\x2A\0".to_vec();
        t.extend_from_slice(&8u32.to_le_bytes());
        let n: u16 = if with_orient.is_some() { 2 } else { 1 };
        t.extend_from_slice(&n.to_le_bytes());
        let data_off = 8 + 2 + n as u32 * 12 + 4;
        // Make (0x010F) ASCII 6 → 오프셋
        t.extend_from_slice(&0x010Fu16.to_le_bytes());
        t.extend_from_slice(&2u16.to_le_bytes());
        t.extend_from_slice(&6u32.to_le_bytes());
        t.extend_from_slice(&data_off.to_le_bytes());
        if let Some(o) = with_orient {
            t.extend_from_slice(&0x0112u16.to_le_bytes());
            t.extend_from_slice(&3u16.to_le_bytes());
            t.extend_from_slice(&1u32.to_le_bytes());
            t.extend_from_slice(&o.to_le_bytes());
            t.extend_from_slice(&[0, 0]);
        }
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(b"Canon\0");
        t
    }

    fn pixel_data(j: &[u8]) -> &[u8] {
        let sos = j.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        &j[sos..]
    }

    #[test]
    fn patch_existing_orientation_in_place() {
        let j = fake_jpeg(Some(&tiff_le(Some(1))));
        let out = set_orientation(&j, 6).unwrap();
        assert_eq!(out.len(), j.len());
        assert_eq!(orientation(&out), 6);
        assert_eq!(pixel_data(&out), pixel_data(&j));
    }

    #[test]
    fn add_orientation_when_missing_keeps_other_tags() {
        let j = fake_jpeg(Some(&tiff_le(None)));
        let out = set_orientation(&j, 8).unwrap();
        assert_eq!(orientation(&out), 8);
        assert_eq!(pixel_data(&out), pixel_data(&j));
        // Make 값이 그대로 읽히는지 (오프셋이 유지됨)
        let exif = exif::Reader::new().read_raw(exif_tiff(&out).unwrap().to_vec()).unwrap();
        let make = exif.get_field(exif::Tag::Make, exif::In::PRIMARY).unwrap();
        assert!(make.display_value().to_string().contains("Canon"));
    }

    #[test]
    fn insert_exif_when_absent() {
        let j = fake_jpeg(None);
        let out = set_orientation(&j, 3).unwrap();
        assert_eq!(orientation(&out), 3);
        assert_eq!(pixel_data(&out), pixel_data(&j));
        // JFIF APP0 다음에 들어가야 한다
        assert_eq!(&out[2..4], &[0xFF, 0xE0]);
    }

    #[test]
    fn tidy_resets_orientation() {
        let t = tiff_le(Some(6));
        let tidy = tidy_for_reencode(&t, 100, 50, true).unwrap();
        let j = fake_jpeg(Some(&tidy));
        assert_eq!(orientation(&j), 1);
    }
}
