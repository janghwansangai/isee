use crate::formats::{Kind, kind_of};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub kind: Kind,
    pub size: u64,
    /// 수정 시각 (Unix ms)
    pub mtime: i64,
    /// 클라우드에만 있고 아직 내려받지 않은 파일 (내용을 읽으면 다운로드가 시작된다)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cloud: bool,
}

/// 온라인 전용 파일인가 (맥: File Provider의 dataless 표시, 윈도우: 클라우드 자리 표시 파일 속성)
pub fn online_only(meta: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const SF_DATALESS: u32 = 0x4000_0000;
        meta.st_flags() & SF_DATALESS != 0
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const OFFLINE: u32 = 0x1000;
        const RECALL_ON_OPEN: u32 = 0x4_0000;
        const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
        meta.file_attributes() & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = meta;
        false
    }
}

/// 경로가 온라인 전용 파일인가 (정보만 읽으므로 다운로드되지 않는다)
pub fn is_online_only(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| online_only(&m))
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name == "Icon\r" || name.eq_ignore_ascii_case("desktop.ini")
        || name.eq_ignore_ascii_case("thumbs.db")
}

/// 폴더 하나의 내용. 하위 폴더 먼저, 그다음 파일을 자연 정렬(1, 2, 10 순서)로 돌려준다.
pub fn list_dir(dir: &Path, show_hidden: bool) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for de in std::fs::read_dir(dir)? {
        let Ok(de) = de else { continue };
        let name = de.file_name().to_string_lossy().into_owned();
        if !show_hidden && is_hidden(&name) {
            continue;
        }
        let path = de.path();
        // 심볼릭 링크는 따라가서 실제 대상의 정보를 쓴다.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let kind = if meta.is_dir() {
            if is_bundle(&path) {
                Kind::Other
            } else {
                Kind::Dir
            }
        } else {
            kind_of(&path)
        };
        out.push(Entry {
            name,
            path: path.to_string_lossy().into_owned(),
            kind,
            size: if meta.is_dir() { 0 } else { meta.len() },
            mtime: mtime_ms(&meta),
            cloud: !meta.is_dir() && online_only(&meta),
        });
    }
    out.sort_by(|a, b| {
        (a.kind != Kind::Dir)
            .cmp(&(b.kind != Kind::Dir))
            .then_with(|| natord::compare_ignore_case(&a.name, &b.name))
    });
    Ok(out)
}

/// 맥의 .app, .photoslibrary 같은 패키지는 폴더처럼 들어가지 않는다.
fn is_bundle(path: &Path) -> bool {
    cfg!(target_os = "macos")
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "app" | "photoslibrary" | "bundle" | "framework" | "pkg" | "lrdata" | "lrlibrary"
                )
            })
}

#[derive(Clone, Debug, Serialize)]
pub struct DirNode {
    pub name: String,
    pub path: String,
    pub has_children: bool,
}

/// 폴더 트리용: 하위 폴더만. 하위의 하위가 있는지는 펼침 화살표 표시용으로 대충 확인한다.
pub fn list_subdirs(dir: &Path, show_hidden: bool) -> std::io::Result<Vec<DirNode>> {
    let mut out: Vec<DirNode> = std::fs::read_dir(dir)?
        .filter_map(|de| de.ok())
        .filter_map(|de| {
            let name = de.file_name().to_string_lossy().into_owned();
            if !show_hidden && is_hidden(&name) {
                return None;
            }
            let path = de.path();
            let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
            if !is_dir || is_bundle(&path) {
                return None;
            }
            let has_children = has_subdir(&path, show_hidden);
            Some(DirNode {
                name,
                path: path.to_string_lossy().into_owned(),
                has_children,
            })
        })
        .collect();
    out.sort_by(|a, b| natord::compare_ignore_case(&a.name, &b.name));
    Ok(out)
}

fn has_subdir(path: &Path, show_hidden: bool) -> bool {
    let Ok(rd) = std::fs::read_dir(path) else {
        return false;
    };
    // 파일이 아주 많은 폴더에서 오래 걸리지 않도록 앞부분만 본다.
    rd.take(400).filter_map(|e| e.ok()).any(|de| {
        let name = de.file_name();
        let name = name.to_string_lossy();
        (show_hidden || !is_hidden(&name))
            && de.file_type().map(|t| t.is_dir()).unwrap_or(false)
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct Place {
    pub name: String,
    pub path: String,
    /// "home" | "desktop" | "pictures" | "downloads" | "documents" | "volume" | "cloud"
    pub icon: &'static str,
}

/// 왼쪽 사이드바의 기본 위치: 자주 쓰는 폴더, 드라이브, 클라우드 동기화 폴더.
pub fn places() -> Vec<Place> {
    let mut out = Vec::new();
    let home = home_dir();
    if let Some(home) = &home {
        out.push(Place {
            name: "홈".into(),
            path: home.to_string_lossy().into_owned(),
            icon: "home",
        });
        for (sub, name, icon) in [
            ("Desktop", "데스크탑", "desktop"),
            ("Pictures", "사진", "pictures"),
            ("Downloads", "다운로드", "downloads"),
            ("Documents", "문서", "documents"),
        ] {
            let p = home.join(sub);
            if p.is_dir() {
                out.push(Place {
                    name: name.into(),
                    path: p.to_string_lossy().into_owned(),
                    icon,
                });
            }
        }
    }
    out.extend(volumes());
    out.extend(cloud_folders(home.as_deref()));
    out
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn volumes() -> Vec<Place> {
    let Ok(rd) = std::fs::read_dir("/Volumes") else {
        return vec![];
    };
    let mut v: Vec<Place> = rd
        .filter_map(|e| e.ok())
        .filter(|e| {
            // 숨김 볼륨(.timemachine 등)과, "/Volumes/Macintosh HD -> /" 링크인 시동 디스크는 뺀다.
            !e.file_name().to_string_lossy().starts_with('.')
                && std::fs::read_link(e.path()).map(|t| t != Path::new("/")).unwrap_or(true)
        })
        .map(|e| Place {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path().to_string_lossy().into_owned(),
            icon: "volume",
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

#[cfg(windows)]
fn volumes() -> Vec<Place> {
    (b'A'..=b'Z')
        .map(|c| format!("{}:\\", c as char))
        .filter(|p| Path::new(p).exists())
        .map(|p| Place {
            name: p.trim_end_matches('\\').to_string(),
            path: p,
            icon: "volume",
        })
        .collect()
}

#[cfg(not(any(target_os = "macos", windows)))]
fn volumes() -> Vec<Place> {
    vec![]
}

/// 구글 드라이브, 원드라이브, 드롭박스, iCloud 동기화 폴더.
fn cloud_folders(home: Option<&Path>) -> Vec<Place> {
    let mut out = Vec::new();
    let Some(home) = home else { return out };
    if cfg!(target_os = "macos") {
        // 최신 맥의 클라우드 앱은 모두 ~/Library/CloudStorage 아래에 폴더를 만든다.
        if let Ok(rd) = std::fs::read_dir(home.join("Library/CloudStorage")) {
            for e in rd.filter_map(|e| e.ok()) {
                let raw = e.file_name().to_string_lossy().into_owned();
                if is_hidden(&raw) || !e.path().is_dir() {
                    continue;
                }
                let name = if let Some(rest) = raw.strip_prefix("GoogleDrive-") {
                    format!("Google Drive ({rest})")
                } else if let Some(rest) = raw.strip_prefix("OneDrive-") {
                    format!("OneDrive ({rest})")
                } else {
                    raw
                };
                out.push(Place {
                    name,
                    path: e.path().to_string_lossy().into_owned(),
                    icon: "cloud",
                });
            }
        }
        let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
        if icloud.is_dir() {
            out.push(Place {
                name: "iCloud Drive".into(),
                path: icloud.to_string_lossy().into_owned(),
                icon: "cloud",
            });
        }
    } else {
        for (sub, name) in [
            ("Google Drive", "Google Drive"),
            ("OneDrive", "OneDrive"),
            ("Dropbox", "Dropbox"),
            ("iCloudDrive", "iCloud Drive"),
        ] {
            let p = home.join(sub);
            if p.is_dir() {
                out.push(Place {
                    name: name.into(),
                    path: p.to_string_lossy().into_owned(),
                    icon: "cloud",
                });
            }
        }
    }
    out
}
