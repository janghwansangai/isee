//! 인물(얼굴별 분류, 8단계): 얼굴 데이터 저장, 백그라운드 분석, 같은 사람끼리 묶기, 이름.
//! 얼굴 데이터와 이름은 앱 데이터 폴더의 faces.db에만 저장하고, 언제든 전부 지울 수 있다.
//! 사용자가 이름을 붙이거나 고친 얼굴은 다음 분류의 기준이 된다 (쓸수록 정확해진다).

use crate::cache::key_of;
use crate::decode;
use crate::faces::{Face, Models, similarity_of};
use crate::thumbs::Req;
use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 분석에 쓰는 사진 크기 (긴 변)
const SIZE: u32 = 1024;
/// 이보다 작은 얼굴(분석 크기 기준 픽셀)은 지문이 불안정해서 묶지 않는다
pub const MIN_FACE_PX: f32 = 40.0;
const VERSION: u32 = 1;

/// 같은 사람으로 묶는 기준 (SFace 코사인). 실측: 0.40에서 다른 사람을 같다고 본 경우 0%
pub const SAME: f32 = 0.40;
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
}

pub struct FaceDb {
    conn: Mutex<Connection>,
}

fn photo_key(r: &Req) -> i64 {
    key_of(&r.path, r.size, r.mtime, 0xFACE_0000 + VERSION)
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
    })
}

const COLS: &str = "id, path, rect, px, emb, person, not_person";

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
             CREATE TABLE IF NOT EXISTS people (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn analyzed(&self, r: &Req) -> bool {
        let conn = self.conn.lock();
        conn.prepare_cached("SELECT 1 FROM photos WHERE k = ?1")
            .and_then(|mut s| s.query_row(params![photo_key(r)], |_| Ok(())).optional())
            .ok()
            .flatten()
            .is_some()
    }

    /// 사진 한 장의 분석 결과 저장 (같은 경로의 예전 결과는 바꾼다)
    pub fn put(&self, r: &Req, faces: &[(Face, f32, Vec<f32>)]) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        let _ = tx.execute("DELETE FROM faces WHERE path = ?1", params![r.path]);
        let _ = tx.execute("DELETE FROM photos WHERE path = ?1", params![r.path]);
        for (f, px, emb) in faces {
            let _ = tx.execute(
                "INSERT INTO faces (path, rect, px, emb) VALUES (?1, ?2, ?3, ?4)",
                params![r.path, serde_json::to_string(&f.rect).unwrap_or_default(), px, emb_blob(emb)],
            );
        }
        let _ = tx.execute("INSERT OR REPLACE INTO photos (k, path, faces) VALUES (?1, ?2, ?3)", params![photo_key(r), r.path, faces.len() as i64]);
        let _ = tx.commit();
    }

    /// 이 사진들에 있는 얼굴
    pub fn faces_of(&self, paths: &[String]) -> Vec<FaceRow> {
        let conn = self.conn.lock();
        let Ok(mut st) = conn.prepare_cached(&format!("SELECT {COLS} FROM faces WHERE path = ?1")) else { return Vec::new() };
        paths
            .iter()
            .flat_map(|p| st.query_map(params![p], row).map(|it| it.flatten().collect::<Vec<_>>()).unwrap_or_default())
            .collect()
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
                    .map(|it| it.flatten().collect())
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

    /// 앱이 파일을 옮기거나 이름을 바꾸면 얼굴도 따라간다
    pub fn rename(&self, from: &str, to: &str) {
        let conn = self.conn.lock();
        let _ = conn.execute("UPDATE faces SET path = ?2 WHERE path = ?1", params![from, to]);
        let _ = conn.execute("UPDATE photos SET path = ?2 WHERE path = ?1", params![from, to]);
    }

    /// 얼굴 데이터와 이름 전부 지우기
    pub fn clear(&self) -> anyhow::Result<()> {
        self.conn.lock().execute_batch("DELETE FROM faces; DELETE FROM photos; DELETE FROM people; VACUUM;")?;
        Ok(())
    }
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
    pub ms: f64,
}

/// 얼굴들을 사람별로 묶는다.
///   1) 사용자가 정한 얼굴은 그 사람에게
///   2) 이름 붙은 사람의 평균 지문과 충분히 가깝고(NAMED) 두 번째 후보보다 뚜렷하면 그 사람에게 (자동)
///   3) 나머지는 큰 얼굴부터 차례로 가장 가까운 묶음에 넣고(SAME), 없으면 새 묶음. 마지막에 가까운 묶음끼리 합친다.
pub fn group(faces: Vec<FaceRow>, people: &[(i64, String, Vec<f32>, usize)], analyzed: usize, total: usize) -> Report {
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
    // 이름 없는 얼굴 묶기: 큰 얼굴(믿을 만한 것)부터
    rest.sort_by(|a, b| b.px.total_cmp(&a.px));
    let mut clusters: Vec<(Vec<f32>, Vec<FaceRow>)> = Vec::new();
    for f in rest {
        let best = clusters
            .iter()
            .enumerate()
            .map(|(i, (c, _))| (similarity_of(&f.emb, c), i))
            .max_by(|a, b| a.0.total_cmp(&b.0));
        match best {
            Some((s, i)) if s >= SAME => {
                let (c, members) = &mut clusters[i];
                members.push(f);
                *c = mean(&members.iter().map(|m| m.emb.clone()).collect::<Vec<_>>());
            }
            _ => clusters.push((f.emb.clone(), vec![f])),
        }
    }
    // 가까운 묶음끼리 합치기
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..clusters.len() {
            for j in i + 1..clusters.len() {
                if similarity_of(&clusters[i].0, &clusters[j].0) >= SAME + 0.05 {
                    let (_, b) = clusters.remove(j);
                    clusters[i].1.extend(b);
                    clusters[i].0 = mean(&clusters[i].1.iter().map(|m| m.emb.clone()).collect::<Vec<_>>());
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
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
    for (_, list) in clusters {
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

/// 얼굴 주변을 정사각형으로 잘라 size px JPEG (인물 화면용)
pub fn face_crop(f: &FaceRow, size: u32) -> Option<Vec<u8>> {
    let px = decode::screen_rgba(Path::new(&f.path), SIZE).ok()?;
    let (w, h) = (px.width as f32, px.height as f32);
    let (cx, cy) = ((f.rect[0] + f.rect[2] / 2.0) * w, (f.rect[1] + f.rect[3] / 2.0) * h);
    let side = (f.rect[2] * w).max(f.rect[3] * h) * 1.6;
    let x0 = (cx - side / 2.0).clamp(0.0, (w - 1.0).max(0.0)) as u32;
    let y0 = (cy - side / 2.0).clamp(0.0, (h - 1.0).max(0.0)) as u32;
    let s = (side as u32).min(px.width - x0).min(px.height - y0).max(1);
    let mut rgb = Vec::with_capacity((s * s * 3) as usize);
    for y in y0..y0 + s {
        for x in x0..x0 + s {
            let i = ((y * px.width + x) * 4) as usize;
            rgb.extend_from_slice(&px.rgba[i..i + 3]);
        }
    }
    let img = image::RgbImage::from_raw(s, s, rgb)?;
    let img = image::imageops::resize(&img, size, size, image::imageops::FilterType::Triangle);
    decode::generic::encode_jpeg(img.as_raw(), size, size, 85).ok().map(|e| e.bytes)
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

/// 사진 한 장: 얼굴 찾기 → 얼굴마다 지문
pub fn analyze_one(models: &Models, path: &Path) -> anyhow::Result<Vec<(Face, f32, Vec<f32>)>> {
    anyhow::ensure!(!crate::listing::is_online_only(path), "온라인 전용 파일");
    let px = decode::screen_rgba(path, SIZE)?;
    let faces = models.detect(&px)?;
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        let side = (f.rect[2] * px.width as f32).min(f.rect[3] * px.height as f32);
        let emb = models.fingerprint(&px, &f)?;
        out.push((f, side, emb));
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
        match analyze_one(&models, Path::new(&req.path)) {
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
        FaceRow { id, path: path.into(), rect: [0.0; 4], px, emb: mean(&[emb]), person: None, not_person: None }
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
        let r = group(faces.clone(), &[], 6, 6);
        assert_eq!(r.groups.len(), 2);
        assert_eq!((r.small, r.singles), (1, 1));
        // A에 이름: 1번 얼굴만 사용자가 정함 → 2번은 자동으로 들어온다
        let mut fs = faces.clone();
        fs[0].person = Some(7);
        let people = vec![(7, "엄마".to_string(), mean(&[a(0.05)]), 1)];
        let r = group(fs.clone(), &people, 6, 6);
        let mom = r.groups.iter().find(|g| g.person == Some(7)).unwrap();
        assert_eq!((mom.faces.len(), mom.auto), (2, 1));
        // 2번이 "엄마 아님"이면 자동으로 넣지 않는다
        fs[1].not_person = Some(7);
        let r = group(fs, &people, 6, 6);
        assert_eq!(r.groups.iter().find(|g| g.person == Some(7)).unwrap().faces.len(), 1);
    }

    #[test]
    fn db_roundtrip() {
        let db = FaceDb::open(None).unwrap();
        let r = Req { path: "/x/a.jpg".into(), size: 10, mtime: 1, index: 0 };
        assert!(!db.analyzed(&r));
        let f = Face { rect: [0.1, 0.1, 0.2, 0.2], marks: [[0.0; 2]; 5], score: 0.9 };
        db.put(&r, &[(f.clone(), 80.0, vec![0.6, 0.8])]);
        assert!(db.analyzed(&r));
        let rows = db.faces_of(&["/x/a.jpg".into()]);
        assert_eq!(rows.len(), 1);
        assert!((rows[0].emb[1] - 0.8).abs() < 1e-6);
        let p = db.create_person("민준").unwrap();
        db.assign(&[rows[0].id], p).unwrap();
        assert_eq!(db.people()[0].3, 1);
        db.rename("/x/a.jpg", "/y/a.jpg");
        assert_eq!(db.faces_of(&["/y/a.jpg".into()]).len(), 1);
        db.clear().unwrap();
        assert!(db.people().is_empty());
    }
}
