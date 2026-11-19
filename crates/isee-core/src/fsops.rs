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

/// 휴지통 기록과 비교할 경로: 상위 폴더를 실제 경로로 풀고(윈도우 짧은 이름 RUNNER~1 등) 대소문자를 무시한다
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn same_path_key(p: &Path) -> String {
    let parent = p.parent().map(|d| std::fs::canonicalize(d).unwrap_or_else(|_| d.to_path_buf())).unwrap_or_default();
    let key = parent.join(p.file_name().unwrap_or_default()).to_string_lossy().into_owned();
    if cfg!(windows) { key.to_lowercase() } else { key }
}

#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn restore_impl(items: &[Trashed]) -> anyhow::Result<()> {
    let wanted: std::collections::HashSet<String> =
        items.iter().map(|i| same_path_key(Path::new(&i.original))).collect();
    let all = trash::os_limited::list()?;
    // 못 찾았을 때 원인을 알 수 있게: 이름이 같은 휴지통 항목의 경로
    let names: Vec<String> = items.iter().filter_map(|i| Path::new(&i.original).file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
    let similar: Vec<String> = all
        .iter()
        .filter(|t| names.contains(&t.name.to_string_lossy().into_owned()))
        .map(|t| same_path_key(&t.original_path()))
        .take(3)
        .collect();
    let total = all.len();
    let mut found: Vec<trash::TrashItem> = all.into_iter().filter(|t| wanted.contains(&same_path_key(&t.original_path()))).collect();
    // 같은 경로가 여러 번 버려졌으면 가장 최근 것만 되돌린다.
    found.sort_by_key(|t| std::cmp::Reverse(t.time_deleted));
    let mut seen = std::collections::HashSet::new();
    found.retain(|t| seen.insert(same_path_key(&t.original_path())));
    let n = found.len();
    trash::os_limited::restore_all(found)?;
    // 못 찾은 것이 있으면 조용히 넘어가지 않고 알린다 (사용자가 휴지통을 비운 경우 등)
    if n < wanted.len() {
        bail!(
            "휴지통에서 {}개를 찾지 못했습니다 (찾던 경로 {:?}, 휴지통 항목 {total}개, 이름이 같은 항목 {similar:?})",
            wanted.len() - n,
            wanted
        );
    }
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

// ───────────── 붙여넣기 (⌘C·⌘X 후 ⌘V, 탐색기처럼) ─────────────

/// 붙여넣기를 사용자가 멈췄다 (anyhow 오류 안에 들어 있으면 `e.is::<Cancelled>()`)
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("멈춤")
    }
}

impl std::error::Error for Cancelled {}

/// 이보다 큰 파일은 조금씩 복사해 진행을 알리고 멈출 수 있게 한다 (작은 파일은 한 번에)
const CHUNKED_FROM: u64 = 32 << 20;
const CHUNK: usize = 4 << 20;

/// 파일·폴더의 파일 수와 전체 크기. 정보만 읽으므로 클라우드 파일도 내려받지 않는다.
pub fn tree_size(path: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, 0);
    };
    if !meta.is_dir() {
        return (1, meta.len());
    }
    let mut total = (0, 0);
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            let (n, b) = tree_size(&e.path());
            total.0 += n;
            total.1 += b;
        }
    }
    total
}

/// 같은 폴더인가 (맥 NFC/NFD, 윈도우 대소문자·짧은 이름이 달라도)
fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// 복사 진행: 이 항목에서 지금까지 복사한 바이트를 알리고, 멈춤을 확인한다.
struct Progress<'a> {
    cancel: &'a std::sync::atomic::AtomicBool,
    report: &'a mut dyn FnMut(u64),
    done: u64,
}

impl Progress<'_> {
    fn add(&mut self, n: u64) {
        self.done += n;
        (self.report)(self.done);
    }
    fn check(&self) -> anyhow::Result<()> {
        if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        Ok(())
    }
}

/// 같은 디스크인가 (같으면 맥 APFS는 복제(clone)라 큰 파일도 즉시 끝난다)
#[cfg(unix)]
fn same_device(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_device(_: &Path, _: &Path) -> bool {
    false
}

fn copy_file(from: &Path, to: &Path, len: u64, chunked_from: u64, p: &mut Progress) -> anyhow::Result<()> {
    p.check()?;
    let to_dir = to.parent().unwrap_or(to);
    // chunked_from이 0이면 늘 조금씩 복사한다 (시험용)
    if chunked_from > 0 && (len < chunked_from || same_device(from, to_dir)) {
        // 운영체제 복사: 날짜·권한(맥은 태그 등 확장 속성도)을 그대로 옮긴다
        std::fs::copy(from, to)?;
        p.add(len);
        return Ok(());
    }
    use std::io::{Read, Write};
    let mut src = std::fs::File::open(from)?;
    let meta = src.metadata()?;
    let mut dst = std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        p.check()?;
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        dst.write_all(&buf[..n])?;
        p.add(n as u64);
    }
    dst.flush()?;
    // 수정한 날짜를 원본과 같게 (날짜순 정렬·날짜별 정리가 그대로 되게)
    if let Ok(t) = meta.modified() {
        let _ = dst.set_modified(t);
    }
    let _ = dst.set_permissions(meta.permissions());
    Ok(())
}

fn copy_tree(from: &Path, to: &Path, chunked_from: u64, p: &mut Progress) -> anyhow::Result<()> {
    p.check()?;
    let meta = std::fs::metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()), chunked_from, p)?;
        }
        Ok(())
    } else {
        copy_file(from, to, meta.len(), chunked_from, p)
    }
}

/// 만들다 만 사본을 지운다 (멈췄거나 실패했을 때. 새로 만든 이름이라 다른 파일이 아니다)
fn remove_partial(to: &Path) {
    if to.is_dir() {
        let _ = std::fs::remove_dir_all(to);
    } else {
        let _ = std::fs::remove_file(to);
    }
}

/// 붙여넣기 하나: path를 dest_dir 안으로 복사하거나(copy) 옮긴다. 이름이 겹치면 " (2)".
/// 같은 폴더에 복사하면 사본을 만들고, 같은 폴더로 옮기면 할 일이 없어 None.
/// report에는 이 항목에서 지금까지 복사한 바이트가 온다. cancel이 켜지면 Cancelled로 멈추고 만들던 사본을 지운다.
pub fn paste(
    path: &Path,
    dest_dir: &Path,
    copy: bool,
    cancel: &std::sync::atomic::AtomicBool,
    report: &mut dyn FnMut(u64),
) -> anyhow::Result<Option<Moved>> {
    paste_with(path, dest_dir, copy, cancel, report, CHUNKED_FROM)
}

fn paste_with(
    path: &Path,
    dest_dir: &Path,
    copy: bool,
    cancel: &std::sync::atomic::AtomicBool,
    report: &mut dyn FnMut(u64),
    chunked_from: u64,
) -> anyhow::Result<Option<Moved>> {
    if !dest_dir.is_dir() {
        bail!("붙여넣을 폴더가 없음: {}", dest_dir.display());
    }
    if std::fs::symlink_metadata(path).is_err() {
        bail!("원본이 없음 (이미 옮겼거나 지웠을 수 있음)");
    }
    if !copy && path.parent().is_some_and(|p| same_dir(p, dest_dir)) {
        return Ok(None);
    }
    if path.is_dir() {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let dest_real = std::fs::canonicalize(dest_dir).unwrap_or_else(|_| dest_dir.to_path_buf());
        if dest_real.starts_with(&real) {
            bail!("폴더를 자기 안에 넣을 수 없음");
        }
    }
    let to = unique_in(dest_dir, &file_name(path));
    let mut p = Progress { cancel, report, done: 0 };
    p.check()?;
    if !copy {
        match std::fs::rename(path, &to) {
            Ok(()) => {
                p.add(tree_size(&to).1);
                return Ok(Some(Moved {
                    from: path.to_string_lossy().into_owned(),
                    to: to.to_string_lossy().into_owned(),
                }));
            }
            // 다른 디스크면 복사한 뒤 원본을 지운다 (아래)
            Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {}
            Err(e) => return Err(e.into()),
        }
    }
    if let Err(e) = copy_tree(path, &to, chunked_from, &mut p) {
        remove_partial(&to);
        return Err(e);
    }
    if !copy {
        // 사본을 다 만든 다음에만 원본을 지운다 (도중에 멈추면 원본은 그대로)
        if path.is_dir() {
            // 폴더는 일부만 지워졌을 수 있으니 사본을 남겨 둔다 (지우면 사진을 잃을 수 있다)
            if let Err(e) = std::fs::remove_dir_all(path) {
                bail!("'{}'에 복사는 했지만 원래 폴더를 다 지우지 못함: {e}", file_name(&to));
            }
        } else if let Err(e) = std::fs::remove_file(path) {
            remove_partial(&to);
            bail!("원본을 지우지 못해 옮기지 않음: {e}");
        }
    }
    Ok(Some(Moved {
        from: path.to_string_lossy().into_owned(),
        to: to.to_string_lossy().into_owned(),
    }))
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
    // "../x", "a/b" 같은 이름으로 다른 폴더에 만들지 않게 한다 (이름 바꾸기와 같은 규칙)
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
        bail!("쓸 수 없는 이름");
    }
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
    fn paste_copies_moves_and_keeps_both_names() {
        use std::sync::atomic::AtomicBool;
        let d = tmp("paste");
        let a = d.join("가");
        let b = d.join("나");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.jpg"), b"1").unwrap();
        std::fs::write(b.join("x.jpg"), b"2").unwrap();
        let no = AtomicBool::new(false);
        let mut sink = |_| {};
        // 같은 폴더에 복사하면 사본, 다른 폴더에 같은 이름이 있으면 " (2)" (덮어쓰지 않음)
        let same = paste(&a.join("x.jpg"), &a, true, &no, &mut sink).unwrap().unwrap();
        assert!(same.to.ends_with("x (2).jpg"));
        let other = paste(&a.join("x.jpg"), &b, true, &no, &mut sink).unwrap().unwrap();
        assert!(other.to.ends_with("x (2).jpg"));
        assert_eq!(std::fs::read(b.join("x.jpg")).unwrap(), b"2");
        assert_eq!(std::fs::read(b.join("x (2).jpg")).unwrap(), b"1");
        // 같은 폴더로 옮기기는 할 일 없음, 다른 폴더로 옮기면 원본이 사라지고 되돌릴 수 있다
        assert!(paste(&a.join("x.jpg"), &a, false, &no, &mut sink).unwrap().is_none());
        let m = paste(&a.join("x.jpg"), &b, false, &no, &mut sink).unwrap().unwrap();
        assert!(!a.join("x.jpg").exists() && m.to.ends_with("x (3).jpg"));
        undo_move(&m).unwrap();
        assert_eq!(std::fs::read(a.join("x.jpg")).unwrap(), b"1");
        // 없는 원본, 폴더를 자기 안에 넣기는 실패
        assert!(paste(&a.join("없음.jpg"), &b, true, &no, &mut sink).is_err());
        assert!(paste(&a, &a, true, &no, &mut sink).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn paste_folder_reports_bytes_and_keeps_dates() {
        use std::sync::atomic::AtomicBool;
        let d = tmp("paste-tree");
        let src = d.join("앨범");
        std::fs::create_dir_all(src.join("속")).unwrap();
        std::fs::write(src.join("a.jpg"), vec![7u8; 10_000]).unwrap();
        std::fs::write(src.join("속").join("b.jpg"), vec![8u8; 25_000]).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::options().write(true).open(src.join("a.jpg")).unwrap().set_modified(old).unwrap();
        let dest = d.join("받는곳");
        std::fs::create_dir_all(&dest).unwrap();
        assert_eq!(tree_size(&src), (2, 35_000));
        let no = AtomicBool::new(false);
        let mut last = 0;
        // 조금씩 복사하는 길(큰 파일용)도 시험한다
        let m = paste_with(&src, &dest, true, &no, &mut |b| last = b, 0).unwrap().unwrap();
        assert_eq!(last, 35_000);
        let to = Path::new(&m.to);
        assert_eq!(std::fs::read(to.join("속").join("b.jpg")).unwrap(), vec![8u8; 25_000]);
        assert_eq!(std::fs::metadata(to.join("a.jpg")).unwrap().modified().unwrap(), old);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn paste_stops_when_cancelled_and_leaves_nothing_half_done() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let d = tmp("paste-cancel");
        let a = d.join("a");
        let b = d.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("big.mov"), vec![1u8; 3 * CHUNK]).unwrap();
        let stop = AtomicBool::new(false);
        // 첫 조각을 복사한 뒤 멈춤을 누른 것처럼
        let r = paste_with(&a.join("big.mov"), &b, true, &stop, &mut |_| stop.store(true, Ordering::Relaxed), 0);
        assert!(r.unwrap_err().is::<Cancelled>());
        assert!(a.join("big.mov").exists());
        assert_eq!(std::fs::read_dir(&b).unwrap().count(), 0, "만들던 사본은 지운다");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn trash_and_restore_roundtrip() {
        let d = tmp("trash");
        let f = d.join("버릴사진.jpg");
        std::fs::write(&f, b"data").unwrap();
        let t = trash(&f).unwrap();
        assert!(!f.exists());
        // GitHub의 윈도우 빌드 서버처럼 휴지통이 없는 환경이면 확인할 수 없다 (실제 PC에서 확인)
        #[cfg(windows)]
        if !trash::os_limited::list().unwrap().iter().any(|i| i.name == "버릴사진.jpg") {
            eprintln!("이 환경에는 휴지통이 없어 되돌리기 확인을 건너뜀");
            return;
        }
        restore(&[t]).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"data");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn new_folder_stays_in_its_parent() {
        let d = tmp("mkdir");
        let inner = d.join("in");
        std::fs::create_dir_all(&inner).unwrap();
        for bad in ["../밖", "a/b", "..", " ", "a\\b"] {
            assert!(create_dir(&inner, bad).is_err(), "{bad}");
        }
        assert!(!d.join("밖").exists());
        let made = create_dir(&inner, " 새 폴더 ").unwrap();
        assert_eq!(made, inner.join("새 폴더").to_string_lossy());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
