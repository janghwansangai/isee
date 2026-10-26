use serde::Serialize;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Dir,
    Image,
    Video,
    Pdf,
    Doc,
    Other,
}

const IMAGE_EXT: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "webp", "bmp", "tif", "tiff", "heic", "heif",
    "avif", "ico", "psd", "tga", "jxl", // RAW
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef",
    "srw", "x3f", "rwl", "mrw", "erf", "3fr", "iiq",
];
const VIDEO_EXT: &[&str] = &[
    "mp4", "mov", "m4v", "avi", "mkv", "wmv", "webm", "mts", "m2ts", "3gp", "flv",
];
const DOC_EXT: &[&str] = &[
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "hwp", "hwpx", "txt", "md", "rtf", "pages",
    "numbers", "key", "csv",
];
const RAW_EXT: &[&str] = &[
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef",
    "srw", "x3f", "rwl", "mrw", "erf", "3fr", "iiq",
];

fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

pub fn kind_of(path: &Path) -> Kind {
    let Some(ext) = ext_lower(path) else {
        return Kind::Other;
    };
    let ext = ext.as_str();
    if IMAGE_EXT.contains(&ext) {
        Kind::Image
    } else if VIDEO_EXT.contains(&ext) {
        Kind::Video
    } else if ext == "pdf" {
        Kind::Pdf
    } else if DOC_EXT.contains(&ext) {
        Kind::Doc
    } else {
        Kind::Other
    }
}

pub fn is_jpeg(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some("jpg" | "jpeg" | "jpe" | "jfif")
    )
}

pub fn is_raw(path: &Path) -> bool {
    ext_lower(path).is_some_and(|e| RAW_EXT.contains(&e.as_str()))
}

/// 웹뷰가 원본 파일을 직접 그릴 수 있는 형식인지와 그때의 MIME.
pub fn web_mime(path: &Path) -> Option<&'static str> {
    Some(match ext_lower(path)?.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => return None,
    })
}
