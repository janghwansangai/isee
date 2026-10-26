//! 사진 표시: 남김(Pick)·버림(Reject)·별점(0~5).
//! 파일 자체는 건드리지 않고 앱 데이터베이스에 둔다. 파일을 고치면 수정 시각이 바뀌어
//! 썸네일 캐시를 다시 만들어야 하므로 느려진다.

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
        let conn = match db_path {
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
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 폴더 바로 안의 파일들 표시 (하위 폴더 안쪽은 제외). 경로 앞부분 범위 검색이라 수천 장도 몇 ms.
    pub fn in_dir(&self, dir: &Path) -> HashMap<String, Mark> {
        let mut prefix = dir.to_string_lossy().into_owned();
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
                s.query_row(params![path], |r| {
                    Ok(Mark { flag: r.get::<_, i64>(0)? as i8, rating: r.get::<_, i64>(1)? as u8 })
                })
                .optional()
            })
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn put(conn: &Connection, path: &str, m: Mark) {
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
}
