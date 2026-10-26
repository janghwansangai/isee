//! 날짜·장소별 폴더 정리 (5단계).
//!   1) scan: 사진·동영상의 날짜와 위치를 읽고, 함께 옮길 짝 파일(RAW+JPEG, 라이브 포토 HEIC+MOV, .xmp·.aae)을 묶는다.
//!   2) plan: 폴더 이름 규칙대로 어느 폴더에 몇 장이 갈지 계산한다 (파일은 건드리지 않음, 규칙을 바꾸면 즉시 다시 계산).
//!   3) apply: 실제로 옮기거나 복사하고, 한 번에 되돌릴 수 있게 기록을 돌려준다.

use crate::formats::{self, Kind};
use crate::fsops::{self, Moved};
use crate::meta::{self, DateSource, Meta};
use crate::places::{self, Level};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// 정리 대상 한 장 (짝 파일 포함)
#[derive(Clone, Debug)]
pub struct Item {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    /// 함께 옮길 파일 (같은 폴더, 같은 이름)
    pub companions: Vec<String>,
    pub meta: Meta,
}

// ───────────────────────── 1) 읽기 ─────────────────────────

const SIDECAR_EXT: &[&str] = &["xmp", "aae", "thm"];

fn ext_of(name: &str) -> String {
    Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// 짝을 찾는 이름: "IMG_1234.HEIC" · "IMG_1234.MOV" · "IMG_1234.CR2.xmp" → "img_1234"
fn pair_key(name: &str) -> String {
    let lower = name.to_lowercase();
    let stem = |s: &str| s.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_else(|| s.to_string());
    let once = stem(&lower);
    // 사이드카가 "사진.cr2.xmp"처럼 원래 확장자를 품고 있으면 한 번 더
    if formats::kind_of(Path::new(&once)) != Kind::Other { stem(&once) } else { once }
}

/// 묶음 안에서 대표 파일 고르기: 일반 사진 > RAW > 동영상 > 사이드카
fn rank(name: &str) -> u8 {
    let p = Path::new(name);
    match formats::kind_of(p) {
        Kind::Image if formats::is_raw(p) => 1,
        Kind::Image => 0,
        Kind::Video => 2,
        _ => 3,
    }
}

fn is_member(name: &str) -> bool {
    matches!(formats::kind_of(Path::new(name)), Kind::Image | Kind::Video) || SIDECAR_EXT.contains(&ext_of(name).as_str())
}

/// 사진·동영상 경로들을 읽는다. 같은 폴더의 짝 파일은 고르지 않았어도 함께 묶는다.
pub fn scan(paths: &[String], progress: &(dyn Fn(usize, usize) + Sync)) -> Vec<Item> {
    // (폴더, 짝 이름) → 고른 파일들
    let mut groups: HashMap<(PathBuf, String), Vec<String>> = HashMap::new();
    for p in paths {
        let path = Path::new(p);
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { continue };
        let name = name.to_string_lossy();
        if !matches!(formats::kind_of(path), Kind::Image | Kind::Video) {
            continue;
        }
        groups.entry((dir.to_path_buf(), pair_key(&name))).or_default().push(p.clone());
    }
    // 폴더마다 한 번만 읽어서 고르지 않은 짝 파일을 찾는다
    let mut dirs: HashMap<PathBuf, HashMap<String, Vec<String>>> = HashMap::new();
    for (dir, _) in groups.keys() {
        dirs.entry(dir.clone()).or_insert_with(|| {
            let mut by_key: HashMap<String, Vec<String>> = HashMap::new();
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') || !is_member(&name) {
                        continue;
                    }
                    by_key.entry(pair_key(&name)).or_default().push(e.path().to_string_lossy().into_owned());
                }
            }
            by_key
        });
    }
    let mut work: Vec<Vec<String>> = groups
        .into_iter()
        .map(|((dir, key), mut files)| {
            if let Some(more) = dirs.get(&dir).and_then(|d| d.get(&key)) {
                for f in more {
                    if !files.contains(f) {
                        files.push(f.clone());
                    }
                }
            }
            files.sort_by_key(|f| (rank(f), f.clone()));
            files
        })
        .collect();
    work.sort();
    let total = work.len();
    let done = AtomicUsize::new(0);
    work.into_par_iter()
        .filter_map(|mut files| {
            let path = files.remove(0);
            let md = std::fs::metadata(&path).ok()?;
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            let meta = meta::read(Path::new(&path), mtime);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 50 == 0 || n == total {
                progress(n, total);
            }
            Some(Item { path, size: md.len(), mtime, companions: files, meta })
        })
        .collect()
}

// ───────────────────────── 2) 계획 ─────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Options {
    /// 폴더 이름 규칙: {년} {월} {일} {장소} {기간}, "/"로 하위 폴더
    pub pattern: String,
    /// 장소 단위
    pub level: Level,
    /// 위치 없는 사진: 30분 안에 찍은 앞뒤 사진의 위치로 추정
    pub guess_place: bool,
    /// {기간}(이벤트)을 나누는 촬영 간격 (시간)
    pub event_hours: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self { pattern: "{년}/{년}-{월}".into(), level: Level::City, guess_place: true, event_hours: 6.0 }
    }
}

pub const NO_PLACE: &str = "위치 없음";
/// 위치를 앞뒤 사진으로 추정할 때 허용하는 시간 차이 (초)
const GUESS_SECS: i64 = 30 * 60;

#[derive(Clone, Debug, Serialize)]
pub struct PlanItem {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub companions: Vec<String>,
    pub date_source: DateSource,
    /// 현지 촬영 시각 (초)
    pub time: i64,
    pub place: Option<String>,
    pub place_guessed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanFolder {
    /// 정리할 위치 아래의 폴더 ("2024/2024-05")
    pub rel: String,
    pub items: Vec<PlanItem>,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Plan {
    pub folders: Vec<PlanFolder>,
    pub total: usize,
    /// 날짜를 파일 수정 시각으로 정한 것
    pub by_mtime: usize,
    pub by_name: usize,
    pub with_gps: usize,
    pub guessed: usize,
    pub no_place: usize,
    /// 함께 옮기는 짝 파일 수
    pub companions: usize,
    pub ms: f64,
}

fn ymd(t: i64) -> (String, String, String) {
    let (y, m, d) = meta::civil(t);
    (format!("{y:04}"), format!("{m:02}"), format!("{d:02}"))
}

/// 이벤트 기간: "2024-05-12", "2024-05-12~14", "2024-05-30~06-02", "2024-12-30~2025-01-02"
fn period(start: i64, end: i64) -> String {
    let (a, b) = (meta::civil(start), meta::civil(end));
    let first = format!("{:04}-{:02}-{:02}", a.0, a.1, a.2);
    if (a.0, a.1, a.2) == (b.0, b.1, b.2) {
        first
    } else if (a.0, a.1) == (b.0, b.1) {
        format!("{first}~{:02}", b.2)
    } else if a.0 == b.0 {
        format!("{first}~{:02}-{:02}", b.1, b.2)
    } else {
        format!("{first}~{:04}-{:02}-{:02}", b.0, b.1, b.2)
    }
}

/// 폴더 이름에 쓸 수 없는 글자를 바꾸고 앞뒤 공백·점을 정리한다 (윈도우 규칙 포함)
fn clean_segment(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_matches(|c: char| c == '.' || c == ' ' || c == '-' || c == '_' || c == '~');
    if s.is_empty() { "_".into() } else { s.to_string() }
}

/// 규칙 한 칸을 채운다. {장소}만 있는 칸은 장소를 모르면 "위치 없음", 다른 글자와 섞여 있으면 빼고 쓴다.
fn fill(pattern: &str, t: i64, period_text: &str, place: Option<&str>) -> String {
    let (y, m, d) = ymd(t);
    pattern
        .split('/')
        .filter(|seg| !seg.trim().is_empty())
        .map(|seg| {
            let only_place = seg.trim() == "{장소}";
            let place = match place {
                Some(p) => p,
                None if only_place => NO_PLACE,
                None => "",
            };
            clean_segment(
                &seg.replace("{기간}", period_text)
                    .replace("{년}", &y)
                    .replace("{월}", &m)
                    .replace("{일}", &d)
                    .replace("{장소}", place),
            )
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// 규칙대로 폴더를 나눈다 (파일은 건드리지 않음)
pub fn plan(items: &[Item], opt: &Options) -> Plan {
    let t0 = std::time::Instant::now();
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| (items[i].meta.time, items[i].path.clone()));
    // 장소: GPS → 지명. 같은 좌표가 많으므로 조금 반올림해 캐시한다.
    let mut cache: HashMap<(i64, i64), Option<String>> = HashMap::new();
    let mut place: Vec<Option<String>> = order
        .iter()
        .map(|&i| {
            let (la, lo) = items[i].meta.gps?;
            cache
                .entry(((la * 1e4).round() as i64, (lo * 1e4).round() as i64))
                .or_insert_with(|| places::lookup(la, lo).map(|p| p.label(opt.level)))
                .clone()
        })
        .collect();
    let with_gps = place.iter().filter(|p| p.is_some()).count();
    // 위치 없는 사진: 촬영 시각이 가장 가까운 (30분 안) 위치 있는 사진을 따른다
    let mut guessed = vec![false; order.len()];
    if opt.guess_place {
        let known: Vec<usize> = (0..order.len()).filter(|&k| place[k].is_some()).collect();
        if !known.is_empty() {
            for k in 0..order.len() {
                if place[k].is_some() {
                    continue;
                }
                let t = items[order[k]].meta.time;
                let at = known.partition_point(|&j| j < k);
                let near = [at.checked_sub(1), Some(at)]
                    .into_iter()
                    .flatten()
                    .filter_map(|n| known.get(n))
                    .map(|&j| ((items[order[j]].meta.time - t).abs(), j))
                    .min();
                if let Some((dt, j)) = near
                    && dt <= GUESS_SECS
                {
                    place[k] = place[j].clone();
                    guessed[k] = true;
                }
            }
        }
    }
    // {기간}을 쓰면 촬영 간격으로 이벤트를 나눈다
    let gap = (opt.event_hours.max(0.5) * 3600.0) as i64;
    let mut event_of = vec![0usize; order.len()];
    let mut events: Vec<(usize, usize)> = Vec::new(); // (첫, 끝) 순서 위치
    for k in 0..order.len() {
        let new = k == 0 || items[order[k]].meta.time - items[order[k - 1]].meta.time > gap;
        if new {
            events.push((k, k));
        }
        let e = events.len() - 1;
        events[e].1 = k;
        event_of[k] = e;
    }
    let event_mode = opt.pattern.contains("{기간}");
    // 이벤트 장소: 가장 많이 나온 지명
    let event_place: Vec<Option<String>> = events
        .iter()
        .map(|&(a, b)| {
            let mut count: HashMap<&str, usize> = HashMap::new();
            for p in place[a..=b].iter().flatten() {
                *count.entry(p.as_str()).or_default() += 1;
            }
            count.into_iter().max_by_key(|&(p, n)| (n, std::cmp::Reverse(p))).map(|(p, _)| p.to_string())
        })
        .collect();
    let mut folders: Vec<PlanFolder> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut plan = Plan { total: items.len(), with_gps, ..Default::default() };
    for (k, &i) in order.iter().enumerate() {
        let it = &items[i];
        let rel = if event_mode {
            let (a, b) = events[event_of[k]];
            let (start, end) = (items[order[a]].meta.time, items[order[b]].meta.time);
            fill(&opt.pattern, start, &period(start, end), event_place[event_of[k]].as_deref())
        } else {
            let (y, m, d) = ymd(it.meta.time);
            fill(&opt.pattern, it.meta.time, &format!("{y}-{m}-{d}"), place[k].as_deref())
        };
        match it.meta.source {
            DateSource::Modified => plan.by_mtime += 1,
            DateSource::FileName => plan.by_name += 1,
            _ => {}
        }
        if guessed[k] {
            plan.guessed += 1;
        }
        if place[k].is_none() {
            plan.no_place += 1;
        }
        plan.companions += it.companions.len();
        let f = *index.entry(rel.clone()).or_insert_with(|| {
            folders.push(PlanFolder { rel, items: Vec::new(), bytes: 0 });
            folders.len() - 1
        });
        let extra: u64 = it.companions.iter().filter_map(|c| std::fs::metadata(c).ok()).map(|m| m.len()).sum();
        folders[f].bytes += it.size + extra;
        folders[f].items.push(PlanItem {
            path: it.path.clone(),
            size: it.size,
            mtime: it.mtime,
            companions: it.companions.clone(),
            date_source: it.meta.source,
            time: it.meta.time,
            place: place[k].clone(),
            place_guessed: guessed[k],
        });
    }
    folders.sort_by(|a, b| natord::compare(&a.rel, &b.rel));
    plan.folders = folders;
    plan.ms = t0.elapsed().as_secs_f64() * 1000.0;
    plan
}

// ───────────────────────── 3) 실행·되돌리기 ─────────────────────────

#[derive(Clone, Debug, Deserialize)]
pub struct ApplyItem {
    pub path: String,
    #[serde(default)]
    pub companions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApplyFolder {
    pub rel: String,
    pub items: Vec<ApplyItem>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Applied {
    pub moved: Vec<Moved>,
    /// 복사로 새로 만든 파일
    pub copied: Vec<String>,
    /// 새로 만든 폴더 (되돌릴 때 비어 있으면 지운다)
    pub created: Vec<String>,
    /// 이미 제자리에 있어 건너뛴 것
    pub unchanged: usize,
    pub failed: Vec<(String, String)>,
}

/// "사진.jpg" → "사진 (n).jpg"
fn with_suffix(name: &str, n: usize) -> String {
    if n < 2 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// 정리 위치(root) 아래에 폴더를 만들고 옮기거나 복사한다. 짝 파일은 같은 이름 번호로 함께 옮긴다.
pub fn apply(root: &Path, folders: &[ApplyFolder], copy: bool, progress: &dyn Fn(usize, usize)) -> Applied {
    let mut out = Applied::default();
    let total: usize = folders.iter().map(|f| f.items.len()).sum();
    let mut done = 0;
    for f in folders {
        // 폴더 만들기 (없던 칸만 기록)
        let mut dir = root.to_path_buf();
        let mut ok = true;
        for seg in f.rel.split('/').map(clean_segment) {
            dir.push(seg);
            if !dir.exists() {
                match std::fs::create_dir(&dir) {
                    Ok(()) => out.created.push(dir.to_string_lossy().into_owned()),
                    Err(e) => {
                        ok = false;
                        for it in &f.items {
                            out.failed.push((it.path.clone(), format!("폴더를 만들 수 없음: {e}")));
                        }
                        break;
                    }
                }
            }
        }
        if !ok {
            done += f.items.len();
            continue;
        }
        for it in &f.items {
            done += 1;
            let group: Vec<&String> = std::iter::once(&it.path).chain(&it.companions).filter(|p| Path::new(p).exists()).collect();
            if group.is_empty() {
                out.failed.push((it.path.clone(), "파일이 없음".into()));
                continue;
            }
            if !copy && Path::new(&it.path).parent() == Some(dir.as_path()) {
                out.unchanged += 1;
                continue;
            }
            // 짝 파일이 모두 같은 번호를 쓰도록: 전부 비어 있는 가장 작은 번호
            let names: Vec<String> = group.iter().map(|p| file_name(p)).collect();
            let n = (1..).find(|&n| names.iter().all(|nm| !dir.join(with_suffix(nm, n)).exists())).unwrap_or(1);
            for (p, nm) in group.iter().zip(&names) {
                match fsops::transfer_as(Path::new(p), &dir.join(with_suffix(nm, n)), copy) {
                    Ok(m) if copy => out.copied.push(m.to),
                    Ok(m) => out.moved.push(m),
                    Err(e) => out.failed.push(((*p).clone(), e.to_string())),
                }
            }
            if done % 20 == 0 || done == total {
                progress(done, total);
            }
        }
    }
    progress(total, total);
    out
}

/// 정리를 통째로 되돌린다: 옮긴 것은 제자리로, 복사한 것은 휴지통으로, 만든 폴더는 비었으면 지운다.
pub fn undo(a: &Applied) -> anyhow::Result<()> {
    for m in a.moved.iter().rev() {
        fsops::undo_move(m)?;
    }
    for p in &a.copied {
        fsops::trash(Path::new(p))?;
    }
    for d in a.created.iter().rev() {
        let dir = Path::new(d);
        // Finder가 만든 .DS_Store만 남았으면 같이 지운다
        let _ = std::fs::remove_file(dir.join(".DS_Store"));
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, time: i64, gps: Option<(f64, f64)>) -> Item {
        Item {
            path: format!("/x/{name}"),
            size: 1,
            mtime: 0,
            companions: vec![],
            meta: Meta { time, source: DateSource::Exif, gps },
        }
    }

    fn at(y: i64, m: i64, d: i64, h: i64) -> i64 {
        meta::days_from_civil(y, m, d) * 86_400 + h * 3600
    }

    #[test]
    fn keys() {
        assert_eq!(pair_key("IMG_1234.HEIC"), "img_1234");
        assert_eq!(pair_key("IMG_1234.MOV"), "img_1234");
        assert_eq!(pair_key("IMG_1234.CR2.xmp"), "img_1234");
        assert_eq!(pair_key("trip.v2.jpg"), "trip.v2");
        assert!(rank("a.jpg") < rank("a.cr2") && rank("a.cr2") < rank("a.mov") && rank("a.mov") < rank("a.aae"));
    }

    #[test]
    fn date_and_place_folders() {
        let jeju = Some((33.2489, 126.4106));
        let items = vec![
            item("a.jpg", at(2024, 5, 12, 10), jeju),
            item("b.jpg", at(2024, 5, 12, 10) + 600, None), // 10분 뒤, 위치 없음 → 추정
            item("c.jpg", at(2024, 6, 1, 9), None),
        ];
        let p = plan(&items, &Options { pattern: "{년}/{년}-{월}".into(), ..Default::default() });
        let rels: Vec<&str> = p.folders.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["2024/2024-05", "2024/2024-06"]);
        let p = plan(&items, &Options { pattern: "{년}-{월}-{일} {장소}".into(), ..Default::default() });
        let rels: Vec<&str> = p.folders.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["2024-05-12 제주 서귀포시", "2024-06-01"]);
        assert_eq!((p.guessed, p.no_place), (1, 1));
        let p = plan(&items, &Options { pattern: "{장소}/{년}-{월}".into(), guess_place: false, ..Default::default() });
        let rels: Vec<&str> = p.folders.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["위치 없음/2024-05", "위치 없음/2024-06", "제주 서귀포시/2024-05"]);
    }

    #[test]
    fn events() {
        let osaka = Some((34.6873, 135.5262));
        let items = vec![
            item("a.jpg", at(2024, 5, 30, 10), osaka),
            item("b.jpg", at(2024, 5, 31, 1), osaka), // 15시간 뒤 → 6시간 간격이면 새 이벤트
            item("c.jpg", at(2024, 5, 31, 4), None),
            item("d.jpg", at(2024, 6, 1, 0), None), // 20시간 뒤
        ];
        let opt = Options { pattern: "{년}/{기간} {장소}".into(), ..Default::default() };
        let rels: Vec<String> = plan(&items, &opt).folders.into_iter().map(|f| f.rel).collect();
        assert_eq!(rels, ["2024/2024-05-30 일본 오사카", "2024/2024-05-31 일본 오사카", "2024/2024-06-01"]);
        let opt = Options { event_hours: 24.0, ..opt };
        let rels: Vec<String> = plan(&items, &opt).folders.into_iter().map(|f| f.rel).collect();
        assert_eq!(rels, ["2024/2024-05-30~06-01 일본 오사카"]);
        assert_eq!(period(at(2024, 12, 30, 0), at(2025, 1, 2, 0)), "2024-12-30~2025-01-02");
    }

    #[test]
    fn apply_moves_pairs_and_undo() {
        let dir = std::env::temp_dir().join(format!("isee-organize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("2024")).unwrap();
        for n in ["IMG_1.HEIC", "IMG_1.MOV", "IMG_1.AAE", "IMG_2.jpg", "notes.txt"] {
            std::fs::write(dir.join(n), n).unwrap();
        }
        // 정리할 폴더에 같은 이름이 이미 있으면 짝 파일 모두 같은 번호로
        std::fs::create_dir_all(dir.join("2024/05")).unwrap();
        std::fs::write(dir.join("2024/05/IMG_1.MOV"), "old").unwrap();
        let s = |n: &str| dir.join(n).to_string_lossy().into_owned();
        let items = scan(&[s("IMG_1.HEIC"), s("IMG_2.jpg")], &|_, _| {});
        let one = items.iter().find(|i| i.path.ends_with("IMG_1.HEIC")).unwrap();
        assert_eq!(one.companions.len(), 2);
        let folders = vec![ApplyFolder {
            rel: "2024/05".into(),
            items: items.iter().map(|i| ApplyItem { path: i.path.clone(), companions: i.companions.clone() }).collect(),
        }];
        let a = apply(&dir, &folders, false, &|_, _| {});
        assert!(a.failed.is_empty(), "{:?}", a.failed);
        assert_eq!(a.moved.len(), 4);
        assert!(dir.join("2024/05/IMG_1 (2).HEIC").exists());
        assert!(dir.join("2024/05/IMG_1 (2).MOV").exists());
        assert!(dir.join("2024/05/IMG_1 (2).AAE").exists());
        assert!(dir.join("2024/05/IMG_2.jpg").exists());
        assert!(dir.join("notes.txt").exists());
        undo(&a).unwrap();
        for n in ["IMG_1.HEIC", "IMG_1.MOV", "IMG_1.AAE", "IMG_2.jpg"] {
            assert!(dir.join(n).exists(), "{n}");
        }
        // 원래 있던 폴더는 남는다
        assert!(dir.join("2024/05/IMG_1.MOV").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
