//! 인물(얼굴별 분류, 8단계): 얼굴 데이터 저장, 백그라운드 분석, 같은 사람끼리 묶기, 이름.
//! 얼굴 데이터와 이름은 앱 데이터 폴더의 faces.db에만 저장하고, 언제든 전부 지울 수 있다.
//! 사용자가 이름을 붙이거나 고친 얼굴은 다음 분류의 기준이 된다 (쓸수록 정확해진다).

use crate::cache::key_of;
use crate::decode;
use crate::faces::{Face, Models, similarity_of};
use crate::listing::nfc;
use crate::thumbs::Req;
use parking_lot::{Condvar, Mutex};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 분석에 쓰는 사진 크기 (긴 변). 단체 사진의 작은 얼굴도 지문을 뽑을 수 있게 크게 읽는다.
const SIZE: u32 = 3072;
/// 이보다 작은 얼굴(분석 크기 기준 픽셀)은 지문이 불안정해서 묶지 않는다
pub const MIN_FACE_PX: f32 = 36.0;
/// 분석 방식이 바뀌면 올린다 (다시 분석하되 사용자가 붙인 이름은 이어받는다)
const VERSION: u32 = 2;

/// 처음 작은 묶음을 만들 때의 기준 (엄격하게)
const SEED: f32 = 0.50;
/// 묶음끼리 합치는 기준: 두 묶음의 모든 얼굴 쌍 비슷함의 평균 (평균 연결)
pub const SAME: f32 = 0.34;
/// 이름 붙은 사람에게 자동으로 넣는 기준 (조금 더 엄격하게)
pub const NAMED: f32 = 0.45;

#[derive(Clone, Debug, Serialize)]
pub struct FaceRow {
    pub id: i64,
    pub path: String,
    /// 사진 안 위치 (0~1)
    pub rect: [f32; 4],
    /// 분석 크기에서 얼굴 짧은 변 (픽셀)
    pub px: f32,
    #[serde(skip)]
    pub emb: Vec<f32>,
    /// 사용자가 정한 사람
    pub person: Option<i64>,
    /// 사용자가 "이 사람 아님"이라고 한 사람
    pub not_person: Option<i64>,
    /// 지문 품질 (정규화 전 길이, 흐리거나 가려진 얼굴일수록 작다)
    pub q: f32,
}

pub struct FaceDb {
    conn: Mutex<Connection>,
}

/// 분석 기록의 열쇠. path는 NFC로 맞춘 경로 (listing::nfc)
fn photo_key(path: &str, size: u64, mtime: i64) -> i64 {
    key_of(path, size, mtime, 0xFACE_0000 + VERSION)
}

fn emb_blob(e: &[f32]) -> Vec<u8> {
    e.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn blob_emb(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<FaceRow> {
    let rect: String = r.get(2)?;
    let rect: Vec<f32> = serde_json::from_str(&rect).unwrap_or_default();
    Ok(FaceRow {
        id: r.get(0)?,
        path: r.get(1)?,
        rect: [rect.first().copied().unwrap_or(0.0), rect.get(1).copied().unwrap_or(0.0), rect.get(2).copied().unwrap_or(0.0), rect.get(3).copied().unwrap_or(0.0)],
        px: r.get(3)?,
        emb: blob_emb(&r.get::<_, Vec<u8>>(4)?),
        person: r.get(5)?,
        not_person: r.get(6)?,
        q: r.get::<_, Option<f64>>(7)?.unwrap_or(0.0) as f32,
    })
}

const COLS: &str = "id, path, rect, px, emb, person, not_person, quality";

impl FaceDb {
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
             CREATE TABLE IF NOT EXISTS photos (k INTEGER PRIMARY KEY, path TEXT NOT NULL, faces INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS photos_path ON photos(path);
             CREATE TABLE IF NOT EXISTS faces (
                id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL, rect TEXT NOT NULL, px REAL NOT NULL,
                emb BLOB NOT NULL, person INTEGER, not_person INTEGER);
             CREATE INDEX IF NOT EXISTS faces_path ON faces(path);
             CREATE INDEX IF NOT EXISTS faces_person ON faces(person);
             CREATE TABLE IF NOT EXISTS people (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL);
             -- 사용자가 이름 없는 묶음에서 \"빼기\"한 얼굴 쌍: 같은 사람으로 묶지 않는다
             CREATE TABLE IF NOT EXISTS apart (a INTEGER NOT NULL, b INTEGER NOT NULL, PRIMARY KEY (a, b));",
        )?;
        // 1판 DB에는 품질 칸이 없다
        let has_quality = conn
            .prepare("SELECT 1 FROM pragma_table_info('faces') WHERE name = 'quality'")
            .and_then(|mut st| st.exists([]))
            .unwrap_or(false);
        if !has_quality {
            conn.execute_batch("ALTER TABLE faces ADD COLUMN quality REAL;")?;
        }
        // 0.1.5부터 얼굴 썸네일(JPEG)을 함께 저장한다. 그 전에 분석한 얼굴은 처음 볼 때 만든다.
        let has_crop = conn
            .prepare("SELECT 1 FROM pragma_table_info('faces') WHERE name = 'crop'")
            .and_then(|mut st| st.exists([]))
            .unwrap_or(false);
        if !has_crop {
            conn.execute_batch("ALTER TABLE faces ADD COLUMN crop BLOB;")?;
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn analyzed(&self, r: &Req) -> bool {
        let k = photo_key(&nfc(&r.path), r.size, r.mtime);
        let conn = self.conn.lock();
        conn.prepare_cached("SELECT 1 FROM photos WHERE k = ?1")
            .and_then(|mut s| s.query_row(params![k], |_| Ok(())).optional())
            .ok()
            .flatten()
            .is_some()
    }

    /// 사진 한 장의 분석 결과 저장 (같은 경로의 예전 결과는 바꾼다).
    /// 다시 분석해도 사용자가 정한 사람·"이 사람 아님"은 같은 자리의 얼굴로 이어받는다.
    pub fn put(&self, r: &Req, faces: &[Analyzed]) {
        let path = nfc(&r.path);
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        let old: Vec<(i64, [f32; 4], Option<i64>, Option<i64>)> = tx
            .prepare("SELECT id, rect, person, not_person FROM faces WHERE path = ?1")
            .and_then(|mut st| {
                st.query_map(params![path], |row| {
                    let rect: Vec<f32> = serde_json::from_str(&row.get::<_, String>(1)?).unwrap_or_default();
                    let rect = [0, 1, 2, 3].map(|i| rect.get(i).copied().unwrap_or(0.0));
                    Ok((row.get(0)?, rect, row.get(2)?, row.get(3)?))
                })?
                .collect()
            })
            .unwrap_or_default();
        let _ = tx.execute("DELETE FROM faces WHERE path = ?1", params![path]);
        let _ = tx.execute("DELETE FROM photos WHERE path = ?1", params![path]);
        for a in faces {
            let same = |o: &&(i64, [f32; 4], Option<i64>, Option<i64>)| overlap(&o.1, &a.face.rect) > 0.4;
            let carried = old.iter().filter(|o| o.2.is_some() || o.3.is_some()).find(same).map(|o| (o.1, o.2, o.3));
            let prev = old.iter().find(same).map(|o| o.0);
            let _ = tx.execute(
                "INSERT INTO faces (path, rect, px, emb, quality, person, not_person, crop) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    path,
                    serde_json::to_string(&a.face.rect).unwrap_or_default(),
                    a.px,
                    emb_blob(&a.emb),
                    a.quality,
                    carried.and_then(|c| c.1),
                    carried.and_then(|c| c.2),
                    a.crop
                ],
            );
            // "빼기"한 쌍도 새 번호로 옮긴다
            if let Some(prev) = prev {
                let id = tx.last_insert_rowid();
                let _ = tx.execute("UPDATE OR IGNORE apart SET a = ?2 WHERE a = ?1", params![prev, id]);
                let _ = tx.execute("UPDATE OR IGNORE apart SET b = ?2 WHERE b = ?1", params![prev, id]);
            }
        }
        // 이어받지 못한(사라진) 얼굴의 쌍은 지운다
        for o in &old {
            let _ = tx.execute("DELETE FROM apart WHERE a = ?1 OR b = ?1", params![o.0]);
        }
        let k = photo_key(&path, r.size, r.mtime);
        let _ = tx.execute("INSERT OR REPLACE INTO photos (k, path, faces) VALUES (?1, ?2, ?3)", params![k, path, faces.len() as i64]);
        let _ = tx.commit();
    }

    /// 이 사진들에 있는 얼굴
    pub fn faces_of(&self, paths: &[String]) -> Vec<FaceRow> {
        let conn = self.conn.lock();
        let Ok(mut st) = conn.prepare_cached(&format!("SELECT {COLS} FROM faces WHERE path = ?1")) else { return Vec::new() };
        paths
            .iter()
            .flat_map(|p| st.query_map(params![nfc(p)], row).map(|it| it.flatten().collect::<Vec<_>>()).unwrap_or_default())
            .collect()
    }

    /// 저장해 둔 얼굴 썸네일 (JPEG)
    pub fn crop(&self, id: i64) -> Option<Vec<u8>> {
        let conn = self.conn.lock();
        conn.prepare_cached("SELECT crop FROM faces WHERE id = ?1")
            .and_then(|mut st| st.query_row(params![id], |r| r.get::<_, Option<Vec<u8>>>(0)).optional())
            .ok()
            .flatten()
            .flatten()
    }

    pub fn save_crops(&self, crops: &[(i64, Vec<u8>)]) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        for (id, b) in crops {
            let _ = tx.execute("UPDATE faces SET crop = ?2 WHERE id = ?1", params![id, b]);
        }
        let _ = tx.commit();
    }

    pub fn face(&self, id: i64) -> Option<FaceRow> {
        let conn = self.conn.lock();
        conn.query_row(&format!("SELECT {COLS} FROM faces WHERE id = ?1"), params![id], row).optional().ok().flatten()
    }

    /// 이름 붙은 사람들과 (사용자가 정한) 얼굴 지문의 평균
    pub fn people(&self) -> Vec<(i64, String, Vec<f32>, usize)> {
        let conn = self.conn.lock();
        let names: Vec<(i64, String)> = conn
            .prepare("SELECT id, name FROM people ORDER BY name")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default();
        let Ok(mut st) = conn.prepare_cached("SELECT emb FROM faces WHERE person = ?1") else { return Vec::new() };
        names
            .into_iter()
            .map(|(id, name)| {
                let embs: Vec<Vec<f32>> = st
                    .query_map(params![id], |r| Ok(blob_emb(&r.get::<_, Vec<u8>>(0)?)))
                    .map(|it| it.flatten().filter(|e: &Vec<f32>| !e.is_empty()).collect())
                    .unwrap_or_default();
                let n = embs.len();
                (id, name, mean(&embs), n)
            })
            .collect()
    }

    pub fn create_person(&self, name: &str) -> anyhow::Result<i64> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO people (name) VALUES (?1)", params![name.trim()])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn rename_person(&self, id: i64, name: &str) -> anyhow::Result<()> {
        self.conn.lock().execute("UPDATE people SET name = ?2 WHERE id = ?1", params![id, name.trim()])?;
        Ok(())
    }

    /// 사람을 지운다 (얼굴은 남고 이름만 빠진다)
    pub fn delete_person(&self, id: i64) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE faces SET person = NULL WHERE person = ?1", params![id])?;
        conn.execute("UPDATE faces SET not_person = NULL WHERE not_person = ?1", params![id])?;
        conn.execute("DELETE FROM people WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 얼굴들을 이 사람으로 정한다
    pub fn assign(&self, faces: &[i64], person: i64) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for f in faces {
            tx.execute("UPDATE faces SET person = ?2, not_person = NULL WHERE id = ?1", params![f, person])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// "이 사람 아님": 사람에서 빼고 다시 자동으로 들어가지 않게 기억한다
    pub fn reject(&self, faces: &[i64], person: i64) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for f in faces {
            tx.execute("UPDATE faces SET person = NULL, not_person = ?2 WHERE id = ?1", params![f, person])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 이름 없는 묶음에서 "빼기": 고른 얼굴(faces)과 남은 얼굴(others)을 다시 같은 사람으로 묶지 않는다.
    /// 고른 얼굴은 다른 묶음(진짜 그 사람)에는 들어갈 수 있다.
    pub fn keep_apart(&self, faces: &[i64], others: &[i64]) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut st = tx.prepare_cached("INSERT OR IGNORE INTO apart (a, b) VALUES (?1, ?2)")?;
            for &f in faces {
                for &o in others {
                    if f != o {
                        st.execute(params![f.min(o), f.max(o)])?;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// "빼기"한 얼굴 쌍 전부
    pub fn apart_pairs(&self) -> Vec<(i64, i64)> {
        let conn = self.conn.lock();
        conn.prepare("SELECT a, b FROM apart")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default()
    }

    /// 앱이 파일을 옮기거나 이름을 바꾸면 얼굴도 따라간다
    pub fn rename(&self, from: &str, to: &str) {
        let (from, to) = (nfc(from), nfc(to));
        let conn = self.conn.lock();
        let _ = conn.execute("UPDATE faces SET path = ?2 WHERE path = ?1", params![from, to]);
        let _ = conn.execute("UPDATE photos SET path = ?2 WHERE path = ?1", params![from, to]);
    }

    /// 0.1.5까지는 같은 사진이 경로의 꼴(NFC/NFD, listing::nfc 참고)만 다르게 두 벌 저장될 수 있었다.
    /// 경로를 NFC로 바꾸고 겹친 사진은 한 벌로 합친다. 이름·"이 사람 아님"이 많은 쪽을 남기고, 다른 쪽에만 있는
    /// 이름·얼굴 썸네일·"빼기" 쌍은 남기는 쪽의 같은 자리 얼굴로 옮긴다. 이름 붙은 얼굴은 버리지 않는다.
    /// 분석 기록(photos)의 열쇠는 경로로 만든 값이라, 파일 크기·수정 시각을 다시 읽어 새 열쇠로 바꾼다
    /// (파일 정보만 읽으므로 클라우드 파일도 내려받지 않는다). 한 번만 한다 (DB의 user_version에 적어 둠).
    /// 돌려주는 값: 고친 사진 수
    pub fn merge_spellings(&self) -> usize {
        let mut conn = self.conn.lock();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
        if version >= 1 {
            return 0;
        }
        let Ok(tx) = conn.transaction() else { return 0 };
        // 중간에 실패하면 아무것도 바꾸지 않고 다음에 다시 한다
        let Ok(n) = merge_spellings_in(&tx) else { return 0 };
        if tx.execute_batch("PRAGMA user_version = 1").is_err() || tx.commit().is_err() {
            return 0;
        }
        n
    }

    /// 얼굴 데이터와 이름 전부 지우기
    pub fn clear(&self) -> anyhow::Result<()> {
        self.conn.lock().execute_batch("DELETE FROM faces; DELETE FROM photos; DELETE FROM people; DELETE FROM apart; VACUUM;")?;
        Ok(())
    }
}

/// merge_spellings에서 쓰는 얼굴 정보
struct Spelled {
    id: i64,
    rect: [f32; 4],
    person: Option<i64>,
    not_person: Option<i64>,
    crop: bool,
}

impl Spelled {
    fn named(&self) -> bool {
        self.person.is_some() || self.not_person.is_some()
    }
}

fn merge_spellings_in(tx: &rusqlite::Transaction) -> rusqlite::Result<usize> {
    // NFC 경로마다 다른 꼴의 경로들
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut st = tx.prepare("SELECT path FROM faces UNION SELECT path FROM photos")?;
        for p in st.query_map([], |r| r.get::<_, String>(0))? {
            let p = p?;
            let n = nfc(&p).into_owned();
            if n != p {
                groups.entry(n).or_default().push(p);
            }
        }
    }
    let load = |path: &str| -> rusqlite::Result<Vec<Spelled>> {
        let mut st = tx.prepare_cached("SELECT id, rect, person, not_person, crop IS NOT NULL FROM faces WHERE path = ?1")?;
        st.query_map(params![path], |r| {
            let rect: Vec<f32> = serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default();
            Ok(Spelled {
                id: r.get(0)?,
                rect: [0, 1, 2, 3].map(|i| rect.get(i).copied().unwrap_or(0.0)),
                person: r.get(2)?,
                not_person: r.get(3)?,
                crop: r.get(4)?,
            })
        })?
        .collect()
    };
    let n = groups.len();
    for (path, mut spellings) in groups {
        // 이미 NFC 경로로도 저장돼 있으면 그것도 한 꼴로 본다
        if tx.prepare_cached("SELECT 1 FROM faces WHERE path = ?1 UNION ALL SELECT 1 FROM photos WHERE path = ?1")?.exists(params![path])? {
            spellings.push(path.clone());
        }
        let mut sets: Vec<Vec<Spelled>> = spellings.iter().map(|p| load(p)).collect::<rusqlite::Result<_>>()?;
        // 남길 꼴: 이름 붙은 얼굴 → 얼굴 썸네일 → 얼굴 수가 많은 쪽, 같으면 이미 NFC인 쪽
        let keep = (0..sets.len())
            .max_by_key(|&i| {
                let s = &sets[i];
                (s.iter().filter(|f| f.named()).count(), s.iter().filter(|f| f.crop).count(), s.len(), spellings[i] == path)
            })
            .unwrap_or(0);
        let mut kept = std::mem::take(&mut sets[keep]);
        for f in sets.into_iter().flatten() {
            let same = kept
                .iter_mut()
                .map(|g| (overlap(&g.rect, &f.rect), g))
                .filter(|(o, _)| *o > 0.4)
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, g)| g);
            let Some(g) = same else {
                // 남길 쪽에 없는 얼굴: 이름이 붙어 있으면 버리지 않고 옮긴다
                if f.named() {
                    tx.execute("UPDATE faces SET path = ?2 WHERE id = ?1", params![f.id, path])?;
                } else {
                    tx.execute("DELETE FROM faces WHERE id = ?1", params![f.id])?;
                    tx.execute("DELETE FROM apart WHERE a = ?1 OR b = ?1", params![f.id])?;
                }
                continue;
            };
            if !g.named() && f.named() {
                tx.execute("UPDATE faces SET person = ?2, not_person = ?3 WHERE id = ?1", params![g.id, f.person, f.not_person])?;
                (g.person, g.not_person) = (f.person, f.not_person);
            }
            if !g.crop && f.crop {
                tx.execute("UPDATE faces SET crop = (SELECT crop FROM faces WHERE id = ?2) WHERE id = ?1", params![g.id, f.id])?;
                g.crop = true;
            }
            tx.execute("UPDATE OR IGNORE apart SET a = ?2 WHERE a = ?1", params![f.id, g.id])?;
            tx.execute("UPDATE OR IGNORE apart SET b = ?2 WHERE b = ?1", params![f.id, g.id])?;
            tx.execute("DELETE FROM apart WHERE a = ?1 OR b = ?1 OR a = b", params![f.id])?;
            tx.execute("DELETE FROM faces WHERE id = ?1", params![f.id])?;
        }
        tx.execute("UPDATE faces SET path = ?2 WHERE path = ?1", params![spellings[keep], path])?;
        // 분석 기록: 남긴 쪽의 기록이 지금 파일과 맞을 때만 새 열쇠로 옮긴다. 아니면 지워서 다음에 다시 분석한다
        // (다시 분석해도 이름은 같은 자리 얼굴로 이어받는다).
        let old_k: Option<i64> =
            tx.query_row("SELECT k FROM photos WHERE path = ?1", params![spellings[keep]], |r| r.get(0)).optional()?;
        let fresh = std::fs::metadata(&path)
            .ok()
            .map(|m| (m.len(), crate::listing::mtime_ms(&m)))
            .filter(|&(size, mtime)| old_k == Some(photo_key(&spellings[keep], size, mtime)));
        for p in &spellings {
            tx.execute("DELETE FROM photos WHERE path = ?1", params![p])?;
        }
        if let Some((size, mtime)) = fresh {
            tx.execute(
                "INSERT OR REPLACE INTO photos (k, path, faces) VALUES (?1, ?2, (SELECT COUNT(*) FROM faces WHERE path = ?2))",
                params![photo_key(&path, size, mtime), path],
            )?;
        }
    }
    Ok(n)
}

/// 분석한 얼굴 하나
pub struct Analyzed {
    pub face: Face,
    /// 분석 크기에서 얼굴 짧은 변 (픽셀)
    pub px: f32,
    pub emb: Vec<f32>,
    pub quality: f32,
    /// 인물 화면용 얼굴 썸네일 (JPEG). 너무 작은 얼굴은 없음.
    pub crop: Option<Vec<u8>>,
}

/// 두 상자의 겹침 (교집합 / 합집합)
fn overlap(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let iy = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    ix * iy / (a[2] * a[3] + b[2] * b[3] - ix * iy)
}

fn mean(embs: &[Vec<f32>]) -> Vec<f32> {
    let Some(first) = embs.first() else { return Vec::new() };
    let mut m = vec![0f32; first.len()];
    for e in embs {
        for (a, b) in m.iter_mut().zip(e) {
            *a += b;
        }
    }
    let n = m.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
    m.iter_mut().for_each(|v| *v /= n);
    m
}

// ───────────────────────── 묶기 ─────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct Group {
    /// 이름 붙은 사람이면 그 번호
    pub person: Option<i64>,
    pub name: Option<String>,
    pub faces: Vec<FaceRow>,
    /// 대표 얼굴 (가장 크게 나온 것)
    pub cover: i64,
    /// 사진 수
    pub photos: usize,
    /// 자동으로 넣은(사용자가 아직 확인하지 않은) 얼굴 수
    pub auto: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub groups: Vec<Group>,
    /// 혼자만 있는 얼굴 (묶을 상대가 없음)
    pub singles: usize,
    /// 너무 작은 얼굴
    pub small: usize,
    pub analyzed: usize,
    pub total: usize,
    /// 얼굴이 아주 많아 빠른 방식(합치기 단계를 줄임)으로 묶었다
    pub fast: bool,
    pub ms: f64,
}

/// 얼굴이 아주 많을 때의 한도. 합치기 단계는 묶음 수의 제곱만큼 메모리를 쓰므로(3000개면 36MB)
/// 그보다 많으면 큰 묶음끼리만 합친다. 새 묶음도 이 수까지만 만들고 나머지는 혼자인 얼굴로 둔다.
const MAX_MERGE: usize = 3000;
const MAX_SEEDS: usize = 8000;

/// 얼굴들을 사람별로 묶는다.
///   1) 사용자가 정한 얼굴은 그 사람에게
///   2) 이름 붙은 사람의 평균 지문과 충분히 가깝고(NAMED) 두 번째 후보보다 뚜렷하면 그 사람에게 (자동)
///   3) 나머지는 큰 얼굴부터 차례로 가장 가까운 묶음에 넣고(SAME), 없으면 새 묶음. 마지막에 가까운 묶음끼리 합친다.
///   ("빼기"한 얼굴 쌍 apart는 같은 묶음에 넣지 않는다)
pub fn group(
    faces: Vec<FaceRow>,
    people: &[(i64, String, Vec<f32>, usize)],
    apart: &[(i64, i64)],
    analyzed: usize,
    total: usize,
) -> Report {
    let t0 = std::time::Instant::now();
    let mut report = Report { analyzed, total, ..Default::default() };
    let mut named: HashMap<i64, Vec<(FaceRow, bool)>> = HashMap::new();
    let mut rest: Vec<FaceRow> = Vec::new();
    for f in faces {
        if let Some(p) = f.person {
            named.entry(p).or_default().push((f, false));
            continue;
        }
        if f.px < MIN_FACE_PX {
            report.small += 1;
            continue;
        }
        let mut sims: Vec<(f32, i64)> = people
            .iter()
            .filter(|(id, _, c, _)| !c.is_empty() && f.not_person != Some(*id))
            .map(|(id, _, c, _)| (similarity_of(&f.emb, c), *id))
            .collect();
        sims.sort_by(|a, b| b.0.total_cmp(&a.0));
        match (sims.first(), sims.get(1)) {
            (Some(&(s, id)), second) if s >= NAMED && second.is_none_or(|t| s - t.0 >= 0.08) => {
                named.entry(id).or_default().push((f, true));
            }
            _ => rest.push(f),
        }
    }
    let (clusters, fast) = average_linkage(rest, apart);
    report.fast = fast;
    let make = |person: Option<i64>, name: Option<String>, list: Vec<FaceRow>, auto: usize| -> Group {
        let cover = list.iter().max_by(|a, b| a.px.total_cmp(&b.px)).map(|f| f.id).unwrap_or(0);
        let photos = list.iter().map(|f| f.path.as_str()).collect::<std::collections::HashSet<_>>().len();
        Group { person, name, faces: list, cover, photos, auto }
    };
    for (id, name, _, _) in people {
        if let Some(list) = named.remove(id) {
            let auto = list.iter().filter(|(_, a)| *a).count();
            report.groups.push(make(Some(*id), Some(name.clone()), list.into_iter().map(|(f, _)| f).collect(), auto));
        }
    }
    let mut unnamed: Vec<Group> = Vec::new();
    for list in clusters {
        if list.len() < 2 {
            report.singles += list.len();
            continue;
        }
        unnamed.push(make(None, None, list, 0));
    }
    unnamed.sort_by(|a, b| b.photos.cmp(&a.photos));
    report.groups.extend(unnamed);
    report.ms = t0.elapsed().as_secs_f64() * 1000.0;
    report
}

/// 이름 없는 얼굴 묶기
///   1) 큰 얼굴부터 차례로, 아주 비슷하면(SEED) 가장 가까운 작은 묶음에 넣는다 (사슬처럼 번지지 않게 엄격하게)
///   2) 작은 묶음끼리 "모든 얼굴 쌍 비슷함의 평균"이 가장 높은 둘을 차례로 합친다 (SAME 이상일 때까지)
///      평균은 지문 합의 내적으로 바로 구한다: avg(A,B) = (ΣA·ΣB) / (|A||B|)
///   3) "빼기"한 얼굴 쌍은 같은 묶음에 넣지 않는다: 그런 묶음끼리의 비슷함을 -∞로 두면
///      합칠 때의 가중 평균에서도 -∞가 이어져 끝까지 합쳐지지 않는다
fn average_linkage(faces: Vec<FaceRow>, apart: &[(i64, i64)]) -> (Vec<Vec<FaceRow>>, bool) {
    average_linkage_with(faces, apart, MAX_MERGE, MAX_SEEDS)
}

/// 한도(max_merge, max_seeds)를 받는 본체 (시험에서 작은 한도로 확인한다)
fn average_linkage_with(
    mut faces: Vec<FaceRow>,
    apart: &[(i64, i64)],
    max_merge: usize,
    max_seeds: usize,
) -> (Vec<Vec<FaceRow>>, bool) {
    faces.sort_by(|a, b| (b.px * b.q.max(1.0)).total_cmp(&(a.px * a.q.max(1.0))));
    let dim = faces.first().map_or(0, |f| f.emb.len());
    let mut partners: HashMap<i64, Vec<i64>> = HashMap::new();
    for &(a, b) in apart {
        partners.entry(a).or_default().push(b);
        partners.entry(b).or_default().push(a);
    }
    // 쌍에 들어 있는 얼굴이 어느 작은 묶음에 들어갔는지
    let mut seed_of: HashMap<i64, usize> = HashMap::new();
    // 작은 묶음의 중심 지문(정규화한 합)은 한 줄로 이어 두고 훑는다. 합은 얼굴이 들어올 때마다 더해 둔다.
    let mut cents: Vec<f32> = Vec::new();
    let mut seeds: Vec<(Vec<f32>, Vec<FaceRow>)> = Vec::new();
    let mut fast = false;
    let mut lone: Vec<Vec<FaceRow>> = Vec::new();
    for f in faces {
        if dim == 0 || f.emb.len() != dim {
            lone.push(vec![f]);
            continue;
        }
        let id = f.id;
        let banned: Vec<usize> = partners.get(&id).map_or(Vec::new(), |ps| ps.iter().filter_map(|p| seed_of.get(p).copied()).collect());
        let at = match nearest(&cents, dim, &f.emb, &banned) {
            Some((s, i)) if s >= SEED => {
                let (sum, m) = &mut seeds[i];
                for (a, b) in sum.iter_mut().zip(&f.emb) {
                    *a += b;
                }
                m.push(f);
                let len = sum.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
                for (c, v) in cents[i * dim..(i + 1) * dim].iter_mut().zip(sum.iter()) {
                    *c = v / len;
                }
                i
            }
            _ if seeds.len() >= max_seeds => {
                // 한도: 더 만들지 않고 혼자인 얼굴로 둔다 (작은 얼굴부터 오므로 덜 중요한 것)
                fast = true;
                lone.push(vec![f]);
                continue;
            }
            _ => {
                cents.extend_from_slice(&f.emb);
                seeds.push((f.emb.clone(), vec![f]));
                seeds.len() - 1
            }
        };
        if partners.contains_key(&id) {
            seed_of.insert(id, at);
        }
    }
    drop(cents);
    if seeds.len() > max_merge {
        // 큰 묶음만 합치기 단계로 (나머지는 그대로 둔다)
        fast = true;
        let mut order: Vec<usize> = (0..seeds.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(seeds[i].1.len()));
        let keep: std::collections::HashSet<usize> = order.into_iter().take(max_merge).collect();
        let mut kept = Vec::with_capacity(max_merge);
        let mut remap: HashMap<usize, usize> = HashMap::new();
        for (i, sd) in seeds.into_iter().enumerate() {
            if keep.contains(&i) {
                remap.insert(i, kept.len());
                kept.push(sd);
            } else {
                lone.push(sd.1);
            }
        }
        seeds = kept;
        seed_of = seed_of.into_iter().filter_map(|(id, i)| remap.get(&i).map(|&j| (id, j))).collect();
    }
    // 묶음마다 지문 합과 얼굴
    let (mut sums, members): (Vec<Vec<f32>>, Vec<Vec<FaceRow>>) = seeds.into_iter().unzip();
    let mut members: Vec<Option<Vec<FaceRow>>> = members.into_iter().map(Some).collect();
    let n = members.len();
    let size = |m: &Option<Vec<FaceRow>>| m.as_ref().map_or(0, |v| v.len()) as f32;
    // 묶음 쌍 평균 비슷함 표 (줄마다 여러 코어로)
    let mut sim = vec![f32::MIN; n * n];
    sim.par_chunks_mut(n.max(1)).enumerate().for_each(|(i, row)| {
        for (j, v) in row.iter_mut().enumerate() {
            if j != i {
                *v = similarity_of(&sums[i], &sums[j]) / (size(&members[i]) * size(&members[j]));
            }
        }
    });
    for (a, ps) in &partners {
        let Some(&ca) = seed_of.get(a) else { continue };
        for p in ps {
            if let Some(&cb) = seed_of.get(p)
                && ca != cb
            {
                sim[ca * n + cb] = f32::NEG_INFINITY;
                sim[cb * n + ca] = f32::NEG_INFINITY;
            }
        }
    }
    // 줄마다 가장 비슷한 상대를 기억해 두고, 합친 뒤에는 바뀐 줄만 다시 찾는다
    // (예전에는 합칠 때마다 표 전체를 훑어서 묶음이 3000개면 수십 초 걸렸다)
    let row_best = |sim: &[f32], members: &[Option<Vec<FaceRow>>], i: usize| -> (f32, usize) {
        let mut b = (f32::NEG_INFINITY, usize::MAX);
        for (j, &v) in sim[i * n..(i + 1) * n].iter().enumerate() {
            if j != i && v > b.0 && members[j].is_some() {
                b = (v, j);
            }
        }
        b
    };
    let mut best: Vec<(f32, usize)> = (0..n).into_par_iter().map(|i| row_best(&sim, &members, i)).collect();
    loop {
        let mut top = (SAME, usize::MAX);
        for (i, b) in best.iter().enumerate() {
            if b.0 > top.0 && members[i].is_some() {
                top = (b.0, i);
            }
        }
        if top.1 == usize::MAX {
            break;
        }
        let (i, j) = (top.1.min(best[top.1].1), top.1.max(best[top.1].1));
        // j를 i에 합치고, i와 다른 묶음의 평균 비슷함을 크기 가중으로 갱신
        let (ni, nj) = (size(&members[i]), size(&members[j]));
        for k in 0..n {
            if k == i || k == j || members[k].is_none() {
                continue;
            }
            let v = (ni * sim[i * n + k] + nj * sim[j * n + k]) / (ni + nj);
            sim[i * n + k] = v;
            sim[k * n + i] = v;
        }
        let moved = members[j].take().unwrap_or_default();
        let sj = std::mem::take(&mut sums[j]);
        for (a, b) in sums[i].iter_mut().zip(&sj) {
            *a += b;
        }
        if let Some(m) = members[i].as_mut() {
            m.extend(moved);
        }
        best[j] = (f32::NEG_INFINITY, usize::MAX);
        best[i] = row_best(&sim, &members, i);
        for k in 0..n {
            if k == i || members[k].is_none() {
                continue;
            }
            if best[k].1 == i || best[k].1 == j {
                best[k] = row_best(&sim, &members, k);
            } else if sim[k * n + i] > best[k].0 {
                best[k] = (sim[k * n + i], i);
            }
        }
    }
    let mut out: Vec<Vec<FaceRow>> = members.into_iter().flatten().collect();
    out.extend(lone);
    (out, fast)
}

/// 중심 지문들(dim개씩 이어 붙인 것) 가운데 가장 비슷한 것. banned는 빼고, 같으면 뒤의 것.
/// 묶음이 많으면 여러 코어로 나누어 훑는다.
fn nearest(cents: &[f32], dim: usize, e: &[f32], banned: &[usize]) -> Option<(f32, usize)> {
    let score = |(i, c): (usize, &[f32])| (if banned.contains(&i) { f32::NEG_INFINITY } else { similarity_of(e, c) }, i);
    let pick = |a: (f32, usize), b: (f32, usize)| if b.0 > a.0 || (b.0 == a.0 && b.1 > a.1) { b } else { a };
    if cents.len() / dim.max(1) >= 2048 {
        cents.par_chunks_exact(dim).enumerate().map(score).reduce_with(pick)
    } else {
        cents.chunks_exact(dim.max(1)).enumerate().map(score).reduce(pick)
    }
}

/// 주인공 기준 이동: 가장 크게 나온 얼굴의 사람. 비슷한 크기로 여러 사람이 나오면 None("단체 사진").
pub fn main_person(faces: &[&FaceRow], person_of: &dyn Fn(&FaceRow) -> Option<i64>) -> Result<Option<i64>, ()> {
    let mut v: Vec<&&FaceRow> = faces.iter().filter(|f| f.px >= MIN_FACE_PX).collect();
    v.sort_by(|a, b| b.px.total_cmp(&a.px));
    match (v.first(), v.get(1)) {
        (None, _) => Ok(None),
        (Some(a), Some(b)) if b.px >= a.px * 0.75 && person_of(b).is_some() => Err(()),
        (Some(a), _) => Ok(person_of(a)),
    }
}

/// 인물 화면 얼굴 썸네일 크기 (화면에는 108px로 보인다)
const CROP_PX: u32 = 160;

/// 얼굴 주변을 정사각형으로 잘라 CROP_PX JPEG (인물 화면용). rect는 사진 안 위치 (0~1).
fn crop_jpeg(px: &decode::Pixels, rect: &[f32; 4]) -> Option<Vec<u8>> {
    let (w, h) = (px.width as f32, px.height as f32);
    let (cx, cy) = ((rect[0] + rect[2] / 2.0) * w, (rect[1] + rect[3] / 2.0) * h);
    let side = (rect[2] * w).max(rect[3] * h) * 1.6;
    let x0 = (cx - side / 2.0).clamp(0.0, (w - 1.0).max(0.0)) as u32;
    let y0 = (cy - side / 2.0).clamp(0.0, (h - 1.0).max(0.0)) as u32;
    let s = (side as u32).min(px.width.checked_sub(x0)?).min(px.height.checked_sub(y0)?).max(1);
    let mut rgb = Vec::with_capacity((s * s * 3) as usize);
    for y in y0..y0 + s {
        for x in x0..x0 + s {
            let i = ((y * px.width + x) * 4) as usize;
            rgb.extend_from_slice(px.rgba.get(i..i + 3)?);
        }
    }
    let img = image::RgbImage::from_raw(s, s, rgb)?;
    let img = image::imageops::resize(&img, CROP_PX, CROP_PX, image::imageops::FilterType::Triangle);
    decode::generic::encode_jpeg(img.as_raw(), CROP_PX, CROP_PX, 82).ok().map(|e| e.bytes)
}

/// 사진 한 장을 한 번만 읽어 그 사진의 (보여 줄) 얼굴을 모두 자른다.
/// 단체 사진이면 얼굴마다 원본을 다시 읽지 않아도 된다.
fn photo_crops(path: &Path, faces: &[FaceRow]) -> Vec<(i64, Vec<u8>)> {
    let shown: Vec<&FaceRow> = faces.iter().filter(|f| f.px >= MIN_FACE_PX || f.person.is_some()).collect();
    if shown.is_empty() {
        return Vec::new();
    }
    let Ok(px) = decode::screen_rgba(path, SIZE) else { return Vec::new() };
    shown.into_iter().filter_map(|f| Some((f.id, crop_jpeg(&px, &f.rect)?))).collect()
}

// ───────────────────────── 얼굴 썸네일 ─────────────────────────

pub type CropFn = Box<dyn FnOnce(Option<Vec<u8>>) + Send>;

/// 기다리는 요청이 이보다 많으면 오래된 것(화면에서 이미 지나간 얼굴)부터 버린다
const CROP_WAIT_MAX: usize = 200;

/// 인물 화면의 얼굴 썸네일. 분석할 때 잘라 둔 것을 바로 주고, 없으면(0.1.4까지 분석한 얼굴)
/// 사진을 읽어 그 사진의 얼굴을 모두 잘라 저장한다. 사진 읽기는 일꾼 몇 개만 하므로
/// 얼굴이 수천 개여도 메모리가 일정하다 (예전에는 얼굴마다 원본을 동시에 읽어 메모리가 모자라 꺼졌다).
pub struct CropService {
    inner: Arc<CropInner>,
}

struct CropInner {
    db: Arc<FaceDb>,
    queue: Mutex<VecDeque<(i64, CropFn)>>,
    cv: Condvar,
}

impl CropService {
    pub fn new(db: Arc<FaceDb>, workers: usize) -> Self {
        let inner = Arc::new(CropInner { db, queue: Mutex::new(VecDeque::new()), cv: Condvar::new() });
        for _ in 0..workers.max(1) {
            let inner = inner.clone();
            std::thread::spawn(move || crop_worker(inner));
        }
        Self { inner }
    }

    /// 저장된 썸네일은 바로, 없으면 차례를 기다려 만든다
    pub fn request(&self, id: i64, done: CropFn) {
        if let Some(b) = self.inner.db.crop(id) {
            return done(Some(b));
        }
        let dropped = {
            let mut q = self.inner.queue.lock();
            q.push_back((id, done));
            if q.len() > CROP_WAIT_MAX { q.pop_front() } else { None }
        };
        self.inner.cv.notify_one();
        if let Some((_, f)) = dropped {
            f(None);
        }
    }

    /// 인물 화면을 닫으면 기다리던 요청은 버린다
    pub fn cancel(&self) {
        let waiting: Vec<(i64, CropFn)> = self.inner.queue.lock().drain(..).collect();
        for (_, f) in waiting {
            f(None);
        }
    }
}

fn crop_worker(inner: Arc<CropInner>) {
    loop {
        let (id, done) = {
            let mut q = inner.queue.lock();
            loop {
                if let Some(x) = q.pop_front() {
                    break x;
                }
                inner.cv.wait(&mut q);
            }
        };
        // 같은 사진의 다른 얼굴을 자르면서 이미 만들었을 수 있다
        if let Some(b) = inner.db.crop(id) {
            done(Some(b));
            continue;
        }
        let Some(f) = inner.db.face(id) else {
            done(None);
            continue;
        };
        let path = Path::new(&f.path);
        // 클라우드에만 있는 사진은 이때 내려받는다 (사용자가 연 인물 화면에 보이는 얼굴만, 2장씩).
        // 한 번 만든 썸네일은 저장해 두므로 다음부터는 원본을 읽지 않는다.
        let faces = inner.db.faces_of(std::slice::from_ref(&f.path));
        // 깨진 사진 때문에 디코더가 멈춰도(패닉) 프로그램은 계속 돈다
        let crops = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| photo_crops(path, &faces))).unwrap_or_default();
        inner.db.save_crops(&crops);
        done(crops.into_iter().find(|(i, _)| *i == id).map(|(_, b)| b));
    }
}

// ───────────────────────── 백그라운드 분석 ─────────────────────────

pub type ProgressFn = Arc<dyn Fn(usize, usize) + Send + Sync>;

#[derive(Default)]
struct State {
    generation: u64,
    queue: VecDeque<Req>,
    total: usize,
    done: usize,
}

struct Inner {
    db: Arc<FaceDb>,
    models: Mutex<Option<Arc<Models>>>,
    st: Mutex<State>,
    cv: Condvar,
    paused: Arc<AtomicBool>,
    on_progress: Mutex<Option<ProgressFn>>,
}

pub struct FaceService {
    inner: Arc<Inner>,
}

impl FaceService {
    pub fn new(db: Arc<FaceDb>, paused: Arc<AtomicBool>, workers: usize) -> Self {
        let inner = Arc::new(Inner {
            db,
            models: Mutex::new(None),
            st: Mutex::new(State::default()),
            cv: Condvar::new(),
            paused,
            on_progress: Mutex::new(None),
        });
        for i in 0..workers.max(1) {
            let inner = inner.clone();
            std::thread::Builder::new().name(format!("faces-{i}")).spawn(move || worker(inner)).expect("스레드 생성");
        }
        Self { inner }
    }

    pub fn db(&self) -> &Arc<FaceDb> {
        &self.inner.db
    }

    /// 모델 읽기 (설치 뒤 처음 한 번)
    pub fn load_models(&self, dir: &Path) -> anyhow::Result<()> {
        if self.inner.models.lock().is_some() {
            return Ok(());
        }
        let m = Models::load(dir)?;
        *self.inner.models.lock() = Some(Arc::new(m));
        Ok(())
    }

    pub fn models_loaded(&self) -> bool {
        self.inner.models.lock().is_some()
    }

    pub fn set_progress(&self, f: ProgressFn) {
        *self.inner.on_progress.lock() = Some(f);
    }

    /// 이 사진들을 분석한다 (이전 요청은 취소, 분석한 것은 건너뜀). (끝난 수, 전체)
    pub fn analyze(&self, reqs: Vec<Req>) -> (usize, usize) {
        let total = reqs.len();
        let todo: VecDeque<Req> = reqs.into_iter().filter(|r| !self.inner.db.analyzed(r)).collect();
        let done = total - todo.len();
        let mut st = self.inner.st.lock();
        st.generation += 1;
        st.queue = todo;
        st.total = total;
        st.done = done;
        drop(st);
        self.inner.cv.notify_all();
        (done, total)
    }

    pub fn progress(&self) -> (usize, usize) {
        let st = self.inner.st.lock();
        (st.done, st.total)
    }
}

/// 사진 한 장: 크게 읽어서 얼굴 찾기(작은 얼굴은 확대한 조각으로) → 얼굴마다 지문 (원본에 가까운 해상도에서)
pub fn analyze_one(models: &Models, path: &Path) -> anyhow::Result<Vec<Analyzed>> {
    anyhow::ensure!(!crate::listing::is_online_only(path), "온라인 전용 파일");
    let px = decode::screen_rgba(path, SIZE)?;
    let faces = models.detect(&px)?;
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        let side = (f.rect[2] * px.width as f32).min(f.rect[3] * px.height as f32);
        // 너무 작은 얼굴은 지문을 만들지 않는다 (위치만 남겨 "작은 얼굴"로 센다)
        let (emb, quality) = if side >= MIN_FACE_PX { models.fingerprint(&px, &f)? } else { (Vec::new(), 0.0) };
        // 이미 읽은 사진에서 바로 잘라 두면 인물 화면이 원본(클라우드)을 다시 읽지 않는다
        let crop = if side >= MIN_FACE_PX { crop_jpeg(&px, &f.rect) } else { None };
        out.push(Analyzed { face: f, px: side, emb, quality, crop });
    }
    Ok(out)
}

fn worker(inner: Arc<Inner>) {
    loop {
        let (req, generation) = {
            let mut st = inner.st.lock();
            loop {
                if !inner.paused.load(Ordering::Relaxed)
                    && inner.models.lock().is_some()
                    && let Some(r) = st.queue.pop_front()
                {
                    break (r, st.generation);
                }
                inner.cv.wait_for(&mut st, Duration::from_millis(200));
            }
        };
        let Some(models) = inner.models.lock().clone() else { continue };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analyze_one(&models, Path::new(&req.path))))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("디코더 오류")));
        match result {
            Ok(faces) => inner.db.put(&req, &faces),
            // 읽을 수 없는 사진은 얼굴 0개로 기록해 다시 시도하지 않는다 (클라우드 파일은 다음에 다시)
            Err(_) if !crate::listing::is_online_only(Path::new(&req.path)) => inner.db.put(&req, &[]),
            Err(_) => {}
        }
        let progress = {
            let mut st = inner.st.lock();
            if st.generation != generation {
                continue;
            }
            st.done += 1;
            (st.done, st.total)
        };
        if let Some(cb) = inner.on_progress.lock().clone() {
            cb(progress.0, progress.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(id: i64, path: &str, emb: Vec<f32>, px: f32) -> FaceRow {
        FaceRow { id, path: path.into(), rect: [0.0; 4], px, emb: mean(&[emb]), person: None, not_person: None, q: 1.0 }
    }

    #[test]
    fn groups_names_and_rejections() {
        // 2차원 지문으로 흉내: 사람 A는 (1,0) 근처, B는 (0,1) 근처
        let a = |d: f32| vec![1.0, d];
        let b = |d: f32| vec![d, 1.0];
        let faces = vec![
            face(1, "/1.jpg", a(0.05), 120.0),
            face(2, "/2.jpg", a(-0.1), 90.0),
            face(3, "/3.jpg", b(0.1), 100.0),
            face(4, "/4.jpg", b(0.0), 80.0),
            face(5, "/5.jpg", a(0.0), 20.0), // 너무 작음
            face(6, "/6.jpg", vec![-1.0, -0.3], 100.0), // 혼자
        ];
        let r = group(faces.clone(), &[], &[], 6, 6);
        assert_eq!(r.groups.len(), 2);
        assert_eq!((r.small, r.singles), (1, 1));
        // A에 이름: 1번 얼굴만 사용자가 정함 → 2번은 자동으로 들어온다
        let mut fs = faces.clone();
        fs[0].person = Some(7);
        let people = vec![(7, "엄마".to_string(), mean(&[a(0.05)]), 1)];
        let r = group(fs.clone(), &people, &[], 6, 6);
        let mom = r.groups.iter().find(|g| g.person == Some(7)).unwrap();
        assert_eq!((mom.faces.len(), mom.auto), (2, 1));
        // 2번이 "엄마 아님"이면 자동으로 넣지 않는다
        fs[1].not_person = Some(7);
        let r = group(fs, &people, &[], 6, 6);
        assert_eq!(r.groups.iter().find(|g| g.person == Some(7)).unwrap().faces.len(), 1);
    }

    #[test]
    fn apart_faces_are_not_grouped_back() {
        // 거의 같은 얼굴 4개가 한 묶음 → 2·4번을 "빼기"하면 1·3번과 따로 묶인다
        let v = |d: f32| vec![1.0, d];
        let faces = vec![
            face(1, "/1.jpg", v(0.0), 120.0),
            face(2, "/2.jpg", v(0.02), 110.0),
            face(3, "/3.jpg", v(0.01), 100.0),
            face(4, "/4.jpg", v(0.03), 90.0),
        ];
        let r = group(faces.clone(), &[], &[], 4, 4);
        assert_eq!(r.groups.len(), 1);
        let apart = [(1, 2), (2, 3), (1, 4), (3, 4)];
        let r = group(faces, &[], &apart, 4, 4);
        let mut sets: Vec<Vec<i64>> = r.groups.iter().map(|g| {
            let mut ids: Vec<i64> = g.faces.iter().map(|f| f.id).collect();
            ids.sort();
            ids
        }).collect();
        sets.sort();
        assert_eq!(sets, vec![vec![1, 3], vec![2, 4]]);
    }

    #[test]
    fn many_faces_use_the_fast_path_without_huge_memory() {
        // 서로 다른 얼굴 300개, 작은 한도(합치기 100·묶음 200): 한도를 넘으면 빠른 방식으로 끝나고 얼굴은 하나도 잃지 않는다
        let mut seed = 12345u64;
        let mut rnd = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        };
        // 128차원 무작위 지문 (서로 거의 직각 → 모두 다른 사람)
        let n = 300;
        let faces: Vec<FaceRow> = (0..n).map(|i| face(i as i64, "/p.jpg", (0..128).map(|_| rnd()).collect(), 100.0)).collect();
        let (clusters, fast) = average_linkage_with(faces.clone(), &[], 100, 200);
        assert!(fast);
        assert_eq!(clusters.iter().map(|c| c.len()).sum::<usize>(), n);
        // 한도 안이면 보통 방식
        let (_, fast) = average_linkage_with(faces, &[], 1000, 1000);
        assert!(!fast);
    }

    #[test]
    fn db_roundtrip() {
        let db = FaceDb::open(None).unwrap();
        let r = Req { path: "/x/a.jpg".into(), size: 10, mtime: 1, index: 0 };
        assert!(!db.analyzed(&r));
        let f = Face { rect: [0.1, 0.1, 0.2, 0.2], marks: [[0.0; 2]; 5], score: 0.9 };
        db.put(&r, &[Analyzed { face: f.clone(), px: 80.0, emb: vec![0.6, 0.8], quality: 1.0, crop: None }]);
        assert!(db.analyzed(&r));
        let rows = db.faces_of(&["/x/a.jpg".into()]);
        assert_eq!(rows.len(), 1);
        assert!((rows[0].emb[1] - 0.8).abs() < 1e-6);
        let p = db.create_person("민준").unwrap();
        db.assign(&[rows[0].id], p).unwrap();
        assert_eq!(db.people()[0].3, 1);
        // "빼기" 쌍은 다시 분석해도 같은 자리의 얼굴로 이어진다
        db.keep_apart(&[rows[0].id], &[999]).unwrap();
        db.put(&r, &[Analyzed { face: f.clone(), px: 80.0, emb: vec![0.6, 0.8], quality: 1.0, crop: None }]);
        let again = db.faces_of(&["/x/a.jpg".into()]);
        assert_ne!(again[0].id, rows[0].id);
        assert_eq!(again[0].person, Some(p));
        let pairs: Vec<(i64, i64)> = db.apart_pairs().into_iter().map(|(a, b)| (a.min(b), a.max(b))).collect();
        assert_eq!(pairs, vec![(again[0].id.min(999), again[0].id.max(999))]);
        db.rename("/x/a.jpg", "/y/a.jpg");
        assert_eq!(db.faces_of(&["/y/a.jpg".into()]).len(), 1);
        db.clear().unwrap();
        assert!(db.people().is_empty());
    }

    #[test]
    fn face_thumbnails_are_saved_and_made_once_per_photo() {
        let dir = std::env::temp_dir().join(format!("isee-crop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("단체.jpg");
        let (w, h) = (640u32, 480u32);
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w) as u8, (i / w) as u8, 128]).collect();
        std::fs::write(&path, decode::generic::encode_jpeg(&rgb, w, h, 90).unwrap().bytes).unwrap();
        let r = Req { path: path.to_string_lossy().into_owned(), size: 1, mtime: 1, index: 0 };
        let face = |x: f32| Face { rect: [x, 0.3, 0.1, 0.15], marks: [[0.0; 2]; 5], score: 0.9 };
        let db = Arc::new(FaceDb::open(None).unwrap());
        // 분석할 때 잘라 둔 썸네일은 그대로, 없는(예전 분석) 얼굴은 처음 볼 때 사진을 읽어 만든다
        db.put(
            &r,
            &[
                Analyzed { face: face(0.1), px: 80.0, emb: vec![1.0], quality: 1.0, crop: Some(vec![1, 2, 3]) },
                Analyzed { face: face(0.5), px: 80.0, emb: vec![1.0], quality: 1.0, crop: None },
                Analyzed { face: face(0.8), px: 80.0, emb: vec![1.0], quality: 1.0, crop: None },
            ],
        );
        let ids: Vec<i64> = db.faces_of(std::slice::from_ref(&r.path)).iter().map(|f| f.id).collect();
        assert_eq!(db.crop(ids[0]), Some(vec![1, 2, 3]));
        assert_eq!(db.crop(ids[1]), None);
        let crops = CropService::new(db.clone(), 2);
        let (tx, rx) = std::sync::mpsc::channel();
        let t = tx.clone();
        crops.request(ids[1], Box::new(move |b| t.send(b).unwrap()));
        let jpeg = rx.recv_timeout(Duration::from_secs(30)).unwrap().expect("썸네일");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        // 같은 사진의 다른 얼굴도 그때 함께 만들어 저장했다
        assert!(db.crop(ids[2]).is_some());
        // 없는 얼굴은 빈 응답 (멈추지 않음)
        crops.request(-1, Box::new(move |b| tx.send(b).unwrap()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nfd_and_nfc_spellings_merge_into_one_photo() {
        let dir = std::env::temp_dir().join(format!("isee-spell-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("세계박람회.jpg");
        std::fs::write(&file, b"photo").unwrap();
        let meta = std::fs::metadata(&file).unwrap();
        let (size, mtime) = (meta.len(), crate::listing::mtime_ms(&meta));
        let composed = file.to_string_lossy().into_owned();
        let decomposed = crate::listing::nfd_variant(&composed).unwrap();
        let req = |path: &str| Req { path: path.into(), size, mtime, index: 0 };
        let db = FaceDb::open(None).unwrap();
        let p = db.create_person("민준").unwrap();
        // 0.1.4가 저장한 모양: 같은 사진이 두 꼴로 두 벌.
        // NFD 쪽에 이름이 더 많고, NFC 쪽에만 얼굴 썸네일·"빼기" 쌍·이름 붙은 다른 얼굴이 있다.
        {
            let conn = db.conn.lock();
            let add = |path: &str, x: f32, person: Option<i64>, not_person: Option<i64>, crop: Option<Vec<u8>>| {
                conn.execute(
                    "INSERT INTO faces (path, rect, px, emb, person, not_person, crop) VALUES (?1, ?2, 80, ?3, ?4, ?5, ?6)",
                    params![path, serde_json::to_string(&[x, 0.2, 0.1, 0.1]).unwrap(), emb_blob(&[1.0, 0.0]), person, not_person, crop],
                )
                .unwrap();
                conn.last_insert_rowid()
            };
            add(&decomposed, 0.1, Some(p), None, None);
            add(&decomposed, 0.5, None, Some(p), None);
            add(&composed, 0.1, None, None, Some(vec![7]));
            let apart = add(&composed, 0.5, None, None, None);
            add(&composed, 0.8, Some(p), None, None);
            conn.execute("INSERT INTO apart (a, b) VALUES (?1, 999)", params![apart]).unwrap();
            for (path, n) in [(&decomposed, 2), (&composed, 3)] {
                conn.execute("INSERT INTO photos (k, path, faces) VALUES (?1, ?2, ?3)", params![photo_key(path, size, mtime), path, n])
                    .unwrap();
            }
        }
        assert_eq!(db.merge_spellings(), 1);
        // 어느 꼴로 물어도 같은 사진 한 장
        let rows = db.faces_of(std::slice::from_ref(&decomposed));
        let ids = |v: &[FaceRow]| v.iter().map(|f| f.id).collect::<Vec<_>>();
        assert_eq!(ids(&rows), ids(&db.faces_of(std::slice::from_ref(&composed))));
        assert_eq!(rows.len(), 3, "두 벌이 한 벌로, 이름 붙은 얼굴은 남긴다: {rows:?}");
        assert!(rows.iter().all(|f| f.path == composed));
        let at = |x: f32| rows.iter().find(|f| (f.rect[0] - x).abs() < 1e-6).unwrap();
        assert_eq!(at(0.1).person, Some(p));
        assert_eq!(at(0.5).not_person, Some(p));
        assert_eq!(at(0.8).person, Some(p));
        assert_eq!(db.crop(at(0.1).id), Some(vec![7]), "다른 꼴에만 있던 썸네일을 옮김");
        assert_eq!(db.apart_pairs(), vec![(at(0.5).id, 999)], "\"빼기\" 쌍도 옮김");
        assert!(db.analyzed(&req(&decomposed)) && db.analyzed(&req(&composed)), "다시 분석하지 않음");
        let photos: i64 = db.conn.lock().query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0)).unwrap();
        assert_eq!(photos, 1);
        assert_eq!(db.people()[0].3, 2);
        // 한 번만 한다
        assert_eq!(db.merge_spellings(), 0);
        // 이후로는 NFD 경로로 저장해도 같은 한 벌 (이름은 같은 자리 얼굴로 이어받음)
        let f = Face { rect: [0.1, 0.2, 0.1, 0.1], marks: [[0.0; 2]; 5], score: 0.9 };
        db.put(&req(&decomposed), &[Analyzed { face: f, px: 80.0, emb: vec![1.0, 0.0], quality: 1.0, crop: None }]);
        let again = db.faces_of(std::slice::from_ref(&composed));
        assert_eq!(again.len(), 1);
        assert_eq!((again[0].path.as_str(), again[0].person), (composed.as_str(), Some(p)));
        db.rename(&decomposed, &format!("{}/새 이름.jpg", dir.to_string_lossy()));
        assert_eq!(db.faces_of(&[format!("{}/새 이름.jpg", dir.to_string_lossy())]).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
