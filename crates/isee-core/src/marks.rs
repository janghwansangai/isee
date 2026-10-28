//! 사진 표시: 남김(Pick)·버림(Reject)·별점(0~5).
//! 파일 자체는 건드리지 않고 앱 데이터베이스에 둔다. 파일을 고치면 수정 시각이 바뀌어
//! 썸네일 캐시를 다시 만들어야 하므로 느려진다.

use crate::listing::nfc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{MAIN_SEPARATOR, Path};

/// flag: 1 남김, -1 버림, 0 없음. rating: 0~5
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub flag: i8,
    pub rating: u8,
}

impl Mark {
    pub fn is_empty(&self) -> bool {
        self.flag == 0 && self.rating == 0
    }
}

pub struct Marks {
    conn: Mutex<Connection>,
}

impl Marks {
    /// db_path가 None이면 메모리에만 둔다 (테스트용)
    pub fn open(db_path: Option<&Path>) -> anyhow::Result<Self> {
        let mut conn = match db_path {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                Connection::open(p)?
            }
            None => Connection::open_in_memory()?,
        };
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS marks (
                 path TEXT PRIMARY KEY,
                 flag INTEGER NOT NULL DEFAULT 0,
                 rating INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            merge_spellings(&mut conn)?;
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 폴더 바로 안의 파일들 표시 (하위 폴더 안쪽은 제외). 경로 앞부분 범위 검색이라 수천 장도 몇 ms.
    pub fn in_dir(&self, dir: &Path) -> HashMap<String, Mark> {
        let mut prefix = nfc(&dir.to_string_lossy()).into_owned();
        if !prefix.ends_with(MAIN_SEPARATOR) {
            prefix.push(MAIN_SEPARATOR);
        }
        let hi = format!("{prefix}\u{10FFFF}");
        let conn = self.conn.lock();
        let Ok(mut stmt) = conn.prepare_cached("SELECT path, flag, rating FROM marks WHERE path >= ?1 AND path < ?2")
        else {
            return HashMap::new();
        };
        let rows = stmt.query_map(params![prefix, hi], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Mark { flag: r.get::<_, i64>(1)? as i8, rating: r.get::<_, i64>(2)? as u8 },
            ))
        });
        let Ok(rows) = rows else { return HashMap::new() };
        rows.filter_map(|r| r.ok())
            .filter(|(p, _)| !p[prefix.len()..].contains(MAIN_SEPARATOR))
            .collect()
    }

    pub fn get(&self, path: &str) -> Mark {
        let conn = self.conn.lock();
        conn.prepare_cached("SELECT flag, rating FROM marks WHERE path = ?1")
            .and_then(|mut s| {
                s.query_row(params![nfc(path)], |r| {
                    Ok(Mark { flag: r.get::<_, i64>(0)? as i8, rating: r.get::<_, i64>(1)? as u8 })
                })
                .optional()
            })
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn put(conn: &Connection, path: &str, m: Mark) {
        let path = nfc(path);
        if m.is_empty() {
            let _ = conn.execute("DELETE FROM marks WHERE path = ?1", params![path]);
        } else {
            let _ = conn.execute(
                "INSERT INTO marks (path, flag, rating) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET flag = excluded.flag, rating = excluded.rating",
                params![path, m.flag as i64, m.rating as i64],
            );
        }
    }

    /// 여러 장에 남김/버림 또는 별점을 준다 (None은 그대로 둠). 되돌리기용으로 이전 값을 돌려준다.
    pub fn set(&self, paths: &[String], flag: Option<i8>, rating: Option<u8>) -> Vec<(String, Mark)> {
        let old: Vec<(String, Mark)> = paths.iter().map(|p| (p.clone(), self.get(p))).collect();
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return old };
        for (p, m) in &old {
            let next = Mark {
                flag: flag.map(|f| f.clamp(-1, 1)).unwrap_or(m.flag),
                rating: rating.map(|r| r.min(5)).unwrap_or(m.rating),
            };
            Self::put(&tx, p, next);
        }
        let _ = tx.commit();
        old
    }

    /// 이전 값으로 되돌린다
    pub fn restore(&self, old: &[(String, Mark)]) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        for (p, m) in old {
            Self::put(&tx, p, *m);
        }
        let _ = tx.commit();
    }

    /// 파일을 옮기거나 이름을 바꾸면 표시도 따라간다
    pub fn rename(&self, from: &str, to: &str) {
        let m = self.get(from);
        if m.is_empty() {
            return;
        }
        let conn = self.conn.lock();
        Self::put(&conn, from, Mark::default());
        Self::put(&conn, to, m);
    }
}

/// 0.1.5까지는 같은 사진의 표시가 경로의 꼴(NFC/NFD, listing::nfc 참고)만 다르게 두 줄일 수 있었다.
/// 경로를 NFC로 바꾸고, 두 줄이면 값이 있는 쪽을 남긴다 (둘 다 있으면 이미 NFC인 쪽). 한 번만 한다.
fn merge_spellings(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let old: Vec<(String, Mark)> = tx
        .prepare("SELECT path, flag, rating FROM marks")?
        .query_map([], |r| Ok((r.get(0)?, Mark { flag: r.get::<_, i64>(1)? as i8, rating: r.get::<_, i64>(2)? as u8 })))?
        .collect::<rusqlite::Result<_>>()?;
    let mut by_path: HashMap<String, Mark> = old.iter().filter(|(p, _)| nfc(p) == p.as_str()).cloned().collect();
    let mut changed = Vec::new();
    for (p, m) in &old {
        let n = nfc(p).into_owned();
        if n == *p {
            continue;
        }
        let cur = by_path.entry(n.clone()).or_default();
        if cur.flag == 0 {
            cur.flag = m.flag;
        }
        if cur.rating == 0 {
            cur.rating = m.rating;
        }
        tx.execute("DELETE FROM marks WHERE path = ?1", params![p])?;
        changed.push(n);
    }
    for n in changed {
        Marks::put(&tx, &n, by_path[&n]);
    }
    tx.execute_batch("PRAGMA user_version = 1")?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_dir_and_undo() {
        let m = Marks::open(None).unwrap();
        let sep = MAIN_SEPARATOR;
        let a = format!("{sep}p{sep}a.jpg");
        let b = format!("{sep}p{sep}b.jpg");
        let sub = format!("{sep}p{sep}sub{sep}c.jpg");
        let other = format!("{sep}pp{sep}d.jpg");
        let old = m.set(&[a.clone(), b.clone()], Some(1), None);
        m.set(&[sub.clone(), other.clone()], Some(-1), Some(3));
        m.set(std::slice::from_ref(&b), None, Some(4));
        assert_eq!(m.get(&b), Mark { flag: 1, rating: 4 });
        let dir = m.in_dir(Path::new(&format!("{sep}p")));
        assert_eq!(dir.len(), 2, "하위 폴더와 비슷한 이름 폴더는 빠져야 함: {dir:?}");
        m.restore(&old);
        assert_eq!(m.get(&a), Mark::default());
        // 이름 바꾸면 따라감
        m.rename(&sub, &a);
        assert_eq!(m.get(&a), Mark { flag: -1, rating: 3 });
        assert!(m.get(&sub).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nfd_and_nfc_spellings_are_one_mark() {
        use crate::listing::nfd_variant;
        let composed = "/사진/학생활동/a.jpg";
        let decomposed = nfd_variant(composed).unwrap();
        let other = nfd_variant("/사진/학생활동/b.jpg").unwrap();
        let file = std::env::temp_dir().join(format!("isee-marks-{}.db", std::process::id()));
        let clean = || {
            for ext in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{ext}", file.to_string_lossy()));
            }
        };
        clean();
        {
            // 0.1.4가 저장한 모양: 같은 사진이 두 꼴로 두 줄
            let conn = Connection::open(&file).unwrap();
            conn.execute_batch("CREATE TABLE marks (path TEXT PRIMARY KEY, flag INTEGER NOT NULL DEFAULT 0, rating INTEGER NOT NULL DEFAULT 0);")
                .unwrap();
            conn.execute("INSERT INTO marks VALUES (?1, 1, 0)", params![decomposed]).unwrap();
            conn.execute("INSERT INTO marks VALUES (?1, 0, 4)", params![composed]).unwrap();
            conn.execute("INSERT INTO marks VALUES (?1, -1, 2)", params![other]).unwrap();
        }
        let m = Marks::open(Some(&file)).unwrap();
        assert_eq!(m.get(composed), Mark { flag: 1, rating: 4 });
        assert_eq!(m.get(&decomposed), Mark { flag: 1, rating: 4 });
        let dir = m.in_dir(Path::new(&nfd_variant("/사진/학생활동").unwrap()));
        assert_eq!(dir.len(), 2, "{dir:?}");
        assert_eq!(dir.get("/사진/학생활동/b.jpg"), Some(&Mark { flag: -1, rating: 2 }));
        // 이후로는 어느 꼴로 표시해도 한 줄
        m.set(std::slice::from_ref(&decomposed), None, Some(5));
        assert_eq!(m.get(composed), Mark { flag: 1, rating: 5 });
        let rows: i64 = m.conn.lock().query_row("SELECT COUNT(*) FROM marks", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 2);
        drop(m);
        clean();
    }
}
