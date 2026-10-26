//! 정보 패널용 파일 정보와 EXIF.

use serde::Serialize;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Debug, Default, Serialize)]
pub struct Info {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mtime: i64,
    pub format: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    pub exposure: Option<String>,
    pub fnumber: Option<String>,
    pub focal: Option<String>,
    pub gps: Option<(f64, f64)>,
    /// 클라우드에만 있는 파일 (내용을 읽지 않았으므로 크기·EXIF 없음)
    pub cloud: bool,
}

pub fn file_info(path: &Path) -> anyhow::Result<Info> {
    let meta = std::fs::metadata(path)?;
    let mut info = Info {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: meta.len(),
        mtime: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
        format: path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default(),
        ..Default::default()
    };
    // 아직 내려받지 않은 클라우드 파일은 내용을 읽지 않는다 (고르기만 해도 다운로드되지 않게)
    if crate::listing::online_only(&meta) {
        info.cloud = true;
        return Ok(info);
    }
    if crate::formats::kind_of(path) != crate::formats::Kind::Image {
        return Ok(info);
    }
    if let Ok((w, h)) = crate::decode::dimensions(path) {
        info.width = Some(w);
        info.height = Some(h);
    }
    if let Ok(exif) = File::open(path)
        .map(BufReader::new)
        .map_err(anyhow::Error::from)
        .and_then(|mut r| Ok(exif::Reader::new().read_from_container(&mut r)?))
    {
        fill_exif(&mut info, &exif);
    }
    Ok(info)
}

fn text(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let f = exif.get_field(tag, exif::In::PRIMARY)?;
    let s = match &f.value {
        exif::Value::Ascii(v) => v
            .iter()
            .map(|b| String::from_utf8_lossy(b).trim().to_string())
            .collect::<Vec<_>>()
            .join(" "),
        _ => f.display_value().to_string(),
    };
    let s = s.trim().trim_matches('"').trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn rational(exif: &exif::Exif, tag: exif::Tag) -> Option<f64> {
    match &exif.get_field(tag, exif::In::PRIMARY)?.value {
        exif::Value::Rational(v) => v.first().map(|r| r.to_f64()),
        exif::Value::SRational(v) => v.first().map(|r| r.to_f64()),
        _ => None,
    }
}

fn gps_coord(exif: &exif::Exif, tag: exif::Tag, ref_tag: exif::Tag) -> Option<f64> {
    let exif::Value::Rational(v) = &exif.get_field(tag, exif::In::PRIMARY)?.value else {
        return None;
    };
    if v.len() < 3 {
        return None;
    }
    let deg = v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0;
    let r = text(exif, ref_tag).unwrap_or_default();
    Some(if r.starts_with('S') || r.starts_with('W') { -deg } else { deg })
}

fn fill_exif(info: &mut Info, exif: &exif::Exif) {
    use exif::Tag;
    info.taken = text(exif, Tag::DateTimeOriginal).or_else(|| text(exif, Tag::DateTime));
    let make = text(exif, Tag::Make).unwrap_or_default();
    let model = text(exif, Tag::Model).unwrap_or_default();
    let camera = if model.to_lowercase().starts_with(&make.to_lowercase()) {
        model
    } else {
        format!("{make} {model}").trim().to_string()
    };
    info.camera = (!camera.is_empty()).then_some(camera);
    info.lens = text(exif, Tag::LensModel);
    info.iso = exif
        .get_field(Tag::PhotographicSensitivity, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0));
    info.exposure = rational(exif, Tag::ExposureTime).map(|t| {
        if t > 0.0 && t < 1.0 {
            format!("1/{}초", (1.0 / t).round())
        } else {
            format!("{t}초")
        }
    });
    info.fnumber = rational(exif, Tag::FNumber).map(|f| format!("f/{f:.1}"));
    info.focal = rational(exif, Tag::FocalLength).map(|f| format!("{f:.0}mm"));
    if let (Some(lat), Some(lon)) = (
        gps_coord(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef),
        gps_coord(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef),
    ) {
        info.gps = Some((lat, lon));
    }
}
