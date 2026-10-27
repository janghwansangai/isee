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
    /// 여러 폴더를 함께 볼 때 이 파일이 있는 폴더 (보여 주는 이름. 예: "1026세계박람회/오전")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
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
            folder: None,
        });
    }
    out.sort_by(|a, b| {
        (a.kind != Kind::Dir)
            .cmp(&(b.kind != Kind::Dir))
            .then_with(|| natord::compare_ignore_case(&a.name, &b.name))
    });
    Ok(out)
}

/// 여러 폴더(그리고 하위 폴더)의 파일을 한 목록으로 모을 때의 한도.
/// 너무 큰 폴더(예: 홈 폴더 전체)를 고르면 끝없이 읽거나 메모리가 모자라지 않게 멈춘다.
#[derive(Clone, Copy, Debug)]
pub struct TreeLimits {
    /// 모을 파일 수
    pub max_files: usize,
    /// 읽어 볼 항목 수 (폴더·사진 아닌 파일 포함)
    pub max_scanned: usize,
    /// 하위 폴더 깊이
    pub max_depth: usize,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self { max_files: 50_000, max_scanned: 400_000, max_depth: 16 }
    }
}

/// 모은 결과
#[derive(Debug, Default)]
pub struct Tree {
    /// 파일만 (폴더는 넣지 않는다). folder에 보여 줄 폴더 이름이 들어 있다.
    pub files: Vec<Entry>,
    /// 읽은 폴더 수
    pub dirs: usize,
    /// 한도에 걸려 일부만 모았다
    pub truncated: bool,
}

/// roots의 파일을 모은다. recursive면 하위 폴더까지 (패키지·숨김 폴더는 들어가지 않음).
/// 폴더 이름은 roots가 하나면 그 안의 상대 경로, 여럿이면 각 root 이름부터 붙인다.
pub fn list_tree(roots: &[PathBuf], recursive: bool, show_hidden: bool, lim: TreeLimits) -> Tree {
    let mut out = Tree::default();
    let mut scanned = 0usize;
    let many = roots.len() > 1;
    for root in roots {
        let base = if many { root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { String::new() };
        // (폴더, 보여 줄 이름, 깊이)
        let mut stack = vec![(root.clone(), base, 0usize)];
        while let Some((dir, label, depth)) = stack.pop() {
            let Ok(entries) = list_dir(&dir, show_hidden) else { continue };
            out.dirs += 1;
            scanned += entries.len();
            let mut subdirs = Vec::new();
            for mut e in entries {
                if e.kind == Kind::Dir {
                    if recursive && depth < lim.max_depth {
                        let name = e.name.clone();
                        let sub = if label.is_empty() { name } else { format!("{label}/{name}") };
                        subdirs.push((PathBuf::from(&e.path), sub, depth + 1));
                    }
                    continue;
                }
                if out.files.len() >= lim.max_files {
                    out.truncated = true;
                    return out;
                }
                e.folder = (!label.is_empty()).then(|| label.clone());
                out.files.push(e);
            }
            if scanned >= lim.max_scanned {
                out.truncated = true;
                return out;
            }
            // 이름 순서대로 들어가도록 거꾸로 쌓는다
            stack.extend(subdirs.into_iter().rev());
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_collects_subfolders_and_stops_at_the_limit() {
        let d = std::env::temp_dir().join(format!("isee-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for sub in ["a", "a/b", "c", ".hidden"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        for f in ["1.jpg", "a/2.jpg", "a/b/3.jpg", "c/4.jpg", ".hidden/5.jpg"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let other = std::env::temp_dir().join(format!("isee-tree2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&other);
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("9.jpg"), b"x").unwrap();

        // 하위 폴더 포함: 숨김 폴더는 빼고, 폴더 이름이 붙는다
        let t = list_tree(&[d.clone()], true, false, TreeLimits::default());
        let mut got: Vec<(Option<String>, String)> = t.files.iter().map(|e| (e.folder.clone(), e.name.clone())).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (None, "1.jpg".into()),
                (Some("a".into()), "2.jpg".into()),
                (Some("a/b".into()), "3.jpg".into()),
                (Some("c".into()), "4.jpg".into()),
            ]
        );
        assert!(!t.truncated);
        // 다른 폴더와 함께 (하위 폴더 없이): 각 폴더 이름이 붙는다
        let t = list_tree(&[d.clone(), other.clone()], false, false, TreeLimits::default());
        assert_eq!(t.files.len(), 2);
        assert!(t.files.iter().all(|e| e.folder.is_some()));
        // 한도: 파일 2개까지만
        let t = list_tree(&[d.clone()], true, false, TreeLimits { max_files: 2, ..Default::default() });
        assert_eq!(t.files.len(), 2);
        assert!(t.truncated);
        std::fs::remove_dir_all(&d).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
    }
}
