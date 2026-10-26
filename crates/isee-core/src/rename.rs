//! 이름 일괄 변경: 규칙으로 새 이름을 만들고(미리보기), 서로 겹치지 않게 두 단계로 바꾼다.
//! 규칙의 칸: {이름} 원래 이름, {번호} 순번, {날짜} 촬영일(없으면 수정일), {시간} 촬영 시각, {폴더} 폴더 이름

use crate::fsops::{Moved, unique_in};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub pattern: String,
    pub start: u32,
    pub digits: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Planned {
    pub from: String,
    pub name: String,
}

/// 촬영 일시 "YYYY-MM-DD", "HHMMSS". EXIF가 없으면 수정 시각.
fn when(path: &Path) -> (String, String) {
    let exif = File::open(path)
        .ok()
        .and_then(|f| exif::Reader::new().read_from_container(&mut BufReader::new(f)).ok())
        .and_then(|e| {
            let f = e.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
                .or_else(|| e.get_field(exif::Tag::DateTime, exif::In::PRIMARY))?;
            match &f.value {
                exif::Value::Ascii(v) => v.first().map(|b| String::from_utf8_lossy(b).to_string()),
                _ => None,
            }
        });
    if let Some(s) = exif {
        // "2024:05:12 14:03:22"
        let d: String = s.chars().take(10).map(|c| if c == ':' { '-' } else { c }).collect();
        let t: String = s.chars().skip(11).filter(|c| c.is_ascii_digit()).collect();
        if d.len() == 10 {
            return (d, t);
        }
    }
    let secs = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil(secs)
}

/// Unix 초 → (날짜, 시간) (UTC. 수정 시각은 참고용이라 시간대 오차는 허용)
fn civil(secs: i64) -> (String, String) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant의 civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        format!("{y:04}-{m:02}-{d:02}"),
        format!("{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60),
    )
}

fn clean(name: &str) -> String {
    name.chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// 규칙대로 새 이름 만들기 (확장자는 그대로 둔다). 순서는 들어온 순서.
pub fn plan(paths: &[String], rule: &Rule) -> Vec<Planned> {
    let uses_date = ["{날짜}", "{시간}", "{date}", "{time}"].iter().any(|t| rule.pattern.contains(t));
    paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let path = Path::new(p);
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let folder = path
                .parent()
                .and_then(|d| d.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let n = format!("{:0width$}", rule.start as usize + i, width = rule.digits.clamp(1, 8) as usize);
            let (date, time) = if uses_date { when(path) } else { Default::default() };
            let name = rule
                .pattern
                .replace("{이름}", &stem)
                .replace("{name}", &stem)
                .replace("{번호}", &n)
                .replace("{n}", &n)
                .replace("{날짜}", &date)
                .replace("{date}", &date)
                .replace("{시간}", &time)
                .replace("{time}", &time)
                .replace("{폴더}", &folder);
            let name = clean(&name);
            Planned { from: p.clone(), name: if name.is_empty() { stem } else { name } + &ext }
        })
        .collect()
}

/// 두 단계로 바꾼다: 먼저 모두 임시 이름으로, 그다음 새 이름으로.
/// 그래서 "1.jpg ↔ 2.jpg"처럼 서로 이름을 맞바꾸는 경우도 안전하다.
/// 새 이름이 바꾸지 않는 다른 파일과 겹치면 " (2)"를 붙인다.
pub fn apply(items: &[Planned]) -> anyhow::Result<Vec<Moved>> {
    let mut targets: HashSet<PathBuf> = HashSet::new();
    for it in items {
        let to = Path::new(&it.from).parent().context("상위 폴더 없음")?.join(&it.name);
        if !targets.insert(to.clone()) {
            bail!("새 이름이 겹칩니다: {}", it.name);
        }
    }
    let mut temps = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let from = Path::new(&it.from);
        let tmp = from.with_file_name(format!(".isee-rename-{}-{i}", std::process::id()));
        std::fs::rename(from, &tmp).with_context(|| format!("{} 이름을 바꿀 수 없음", it.from))?;
        temps.push(tmp);
    }
    let mut done = Vec::with_capacity(items.len());
    for (it, tmp) in items.iter().zip(&temps) {
        let dir = Path::new(&it.from).parent().unwrap();
        let mut to = dir.join(&it.name);
        if to.exists() {
            to = unique_in(dir, &it.name);
        }
        std::fs::rename(tmp, &to)?;
        done.push(Moved { from: it.from.clone(), to: to.to_string_lossy().into_owned() });
    }
    Ok(done)
}

/// 일괄 변경 되돌리기 (역시 두 단계)
pub fn undo(done: &[Moved]) -> anyhow::Result<()> {
    let back: Vec<Planned> = done
        .iter()
        .map(|m| Planned {
            from: m.to.clone(),
            name: Path::new(&m.from).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        })
        .collect();
    apply(&back).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_names_and_undo() {
        let d = std::env::temp_dir().join(format!("isee-rn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("a.jpg");
        let b = d.join("b.jpg");
        std::fs::write(&a, b"A").unwrap();
        std::fs::write(&b, b"B").unwrap();
        let paths = vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
        // 이름 맞바꾸기
        let items = vec![
            Planned { from: paths[0].clone(), name: "b.jpg".into() },
            Planned { from: paths[1].clone(), name: "a.jpg".into() },
        ];
        let done = apply(&items).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"B");
        undo(&done).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"A");
        // 규칙: 여행_{번호}
        let p = plan(&paths, &Rule { pattern: "여행_{번호}".into(), start: 1, digits: 3 });
        assert_eq!(p[0].name, "여행_001.jpg");
        assert_eq!(p[1].name, "여행_002.jpg");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn civil_date() {
        assert_eq!(civil(0), ("1970-01-01".into(), "000000".into()));
        assert_eq!(civil(1_715_522_602).0, "2024-05-12");
    }
}
