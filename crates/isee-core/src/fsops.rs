//! 파일 작업: 휴지통, 이동, 복사, 이름 바꾸기와 각각의 되돌리기.
//! 삭제는 항상 OS 휴지통으로 보내므로 복구할 수 있다.

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trashed {
    pub original: String,
    /// 휴지통 안의 위치 (맥). 윈도우는 OS가 기억하므로 None.
    pub trashed: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Moved {
    pub from: String,
    pub to: String,
}

pub fn trash(path: &Path) -> anyhow::Result<Trashed> {
    trash_impl(path)
}

#[cfg(target_os = "macos")]
fn trash_impl(path: &Path) -> anyhow::Result<Trashed> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let fm = NSFileManager::defaultManager();
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut out = None;
    fm.trashItemAtURL_resultingItemURL_error(&url, Some(&mut out))
        .map_err(|e| anyhow::anyhow!(e.localizedDescription().to_string()))?;
    Ok(Trashed {
        original: path.to_string_lossy().into_owned(),
        trashed: out.and_then(|u| u.path()).map(|s| s.to_string()),
    })
}

#[cfg(not(target_os = "macos"))]
fn trash_impl(path: &Path) -> anyhow::Result<Trashed> {
    trash::delete(path)?;
    Ok(Trashed {
        original: path.to_string_lossy().into_owned(),
        trashed: None,
    })
}

/// 휴지통에서 원래 위치로 되돌린다.
pub fn restore(items: &[Trashed]) -> anyhow::Result<()> {
    restore_impl(items)
}

#[cfg(target_os = "macos")]
fn restore_impl(items: &[Trashed]) -> anyhow::Result<()> {
    for it in items {
        let from = it.trashed.as_deref().context("휴지통 위치를 모름")?;
        let orig = Path::new(&it.original);
        let to = if orig.exists() {
            unique_in(orig.parent().context("상위 폴더 없음")?, &file_name(orig))
        } else {
            orig.to_path_buf()
        };
        move_path(Path::new(from), &to)?;
    }
    Ok(())
}

#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn restore_impl(items: &[Trashed]) -> anyhow::Result<()> {
    let wanted: std::collections::HashSet<PathBuf> =
        items.iter().map(|i| PathBuf::from(&i.original)).collect();
    let mut found: Vec<trash::TrashItem> = trash::os_limited::list()?
        .into_iter()
        .filter(|t| wanted.contains(&t.original_path()))
        .collect();
    // 같은 경로가 여러 번 버려졌으면 가장 최근 것만 되돌린다.
    found.sort_by_key(|t| std::cmp::Reverse(t.time_deleted));
    let mut seen = std::collections::HashSet::new();
    found.retain(|t| seen.insert(t.original_path()));
    trash::os_limited::restore_all(found)?;
    Ok(())
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// dir 안에서 겹치지 않는 이름: "사진.jpg" → "사진 (2).jpg" → "사진 (3).jpg"
pub fn unique_in(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|c| !c.exists())
        .unwrap()
}

/// 같은 디스크면 이름만 바꾸고(즉시), 다른 디스크면 복사 후 원본 삭제.
fn move_path(from: &Path, to: &Path) -> anyhow::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_path(from, to)?;
            if from.is_dir() {
                std::fs::remove_dir_all(from)?;
            } else {
                std::fs::remove_file(from)?;
            }
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn copy_path(from: &Path, to: &Path) -> anyhow::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_path(&e.path(), &to.join(e.file_name()))?;
        }
    } else {
        // 맥 APFS에서는 내부적으로 복제(clone)라서 용량을 거의 쓰지 않고 즉시 끝난다.
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// 파일 여러 개를 dest 폴더로 이동하거나 복사. 이름이 겹치면 " (2)"를 붙인다.
pub fn transfer(path: &Path, dest_dir: &Path, copy: bool) -> anyhow::Result<Moved> {
    if !dest_dir.is_dir() {
        bail!("대상 폴더가 없음: {}", dest_dir.display());
    }
    if path.parent() == Some(dest_dir) && !copy {
        bail!("이미 그 폴더에 있음");
    }
    if path.is_dir() && dest_dir.starts_with(path) {
        bail!("폴더를 자기 안으로 옮길 수 없음");
    }
    let to = unique_in(dest_dir, &file_name(path));
    if copy {
        copy_path(path, &to)?;
    } else {
        move_path(path, &to)?;
    }
    Ok(Moved {
        from: path.to_string_lossy().into_owned(),
        to: to.to_string_lossy().into_owned(),
    })
}

/// 정해 둔 경로로 이동하거나 복사 (이미 있으면 실패). 짝 파일 이름을 맞춰 옮길 때 쓴다.
pub fn transfer_as(path: &Path, to: &Path, copy: bool) -> anyhow::Result<Moved> {
    if to.exists() {
        bail!("같은 이름이 이미 있음: {}", to.display());
    }
    if copy {
        copy_path(path, to)?;
    } else {
        move_path(path, to)?;
    }
    Ok(Moved {
        from: path.to_string_lossy().into_owned(),
        to: to.to_string_lossy().into_owned(),
    })
}

/// 이동을 되돌린다 (to → from).
pub fn undo_move(m: &Moved) -> anyhow::Result<()> {
    let from = Path::new(&m.from);
    let target = if from.exists() {
        unique_in(from.parent().context("상위 폴더 없음")?, &file_name(from))
    } else {
        from.to_path_buf()
    };
    move_path(Path::new(&m.to), &target)
}

pub fn rename(path: &Path, new_name: &str) -> anyhow::Result<Moved> {
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name.contains(['/', '\\', ':']) || new_name.starts_with('.') {
        bail!("쓸 수 없는 이름");
    }
    let to = path.parent().context("상위 폴더 없음")?.join(new_name);
    // 대소문자만 바꾸는 경우(맥·윈도우)는 같은 파일이므로 허용한다.
    let same_file = to.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
    if to.exists() && !same_file {
        bail!("같은 이름이 이미 있음");
    }
    std::fs::rename(path, &to)?;
    Ok(Moved {
        from: path.to_string_lossy().into_owned(),
        to: to.to_string_lossy().into_owned(),
    })
}

pub fn create_dir(parent: &Path, name: &str) -> anyhow::Result<String> {
    let p = unique_in(parent, name);
    std::fs::create_dir(&p)?;
    Ok(p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("isee-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn transfer_renames_on_conflict_and_undo_restores() {
        let d = tmp("transfer");
        let a = d.join("a");
        let b = d.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.jpg"), b"1").unwrap();
        std::fs::write(b.join("x.jpg"), b"2").unwrap();
        let m = transfer(&a.join("x.jpg"), &b, false).unwrap();
        assert!(m.to.ends_with("x (2).jpg"));
        assert!(!a.join("x.jpg").exists());
        undo_move(&m).unwrap();
        assert_eq!(std::fs::read(a.join("x.jpg")).unwrap(), b"1");
        let c = transfer(&a.join("x.jpg"), &b, true).unwrap();
        assert!(a.join("x.jpg").exists() && Path::new(&c.to).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn trash_and_restore_roundtrip() {
        let d = tmp("trash");
        let f = d.join("버릴사진.jpg");
        std::fs::write(&f, b"data").unwrap();
        let t = trash(&f).unwrap();
        assert!(!f.exists());
        restore(&[t]).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"data");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
