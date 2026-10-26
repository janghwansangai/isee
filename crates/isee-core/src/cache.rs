//! 썸네일 캐시: 메모리 LRU + 디스크(SQLite).
//! 키는 (경로, 파일 크기, 수정 시각, 썸네일 크기)의 해시라서 파일이 바뀌면 자동으로 새로 만든다.

use crossbeam_channel::{Receiver, Sender, unbounded};
use lru::LruCache;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct Thumb {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub mime: &'static str,
}

/// FNV-1a 64비트. 러스트 버전이 바뀌어도 값이 같아야 디스크 캐시를 계속 쓸 수 있다.
pub fn key_of(path: &str, size: u64, mtime: i64, px: u32) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(path.as_bytes());
    feed(&size.to_le_bytes());
    feed(&mtime.to_le_bytes());
    feed(&px.to_le_bytes());
    h as i64
}

fn mime_code(m: &str) -> i64 {
    if m == "image/png" { 1 } else { 0 }
}

fn mime_of(code: i64) -> &'static str {
    if code == 1 { "image/png" } else { "image/jpeg" }
}

pub struct ThumbCache {
    mem: Mutex<LruCache<i64, Arc<Thumb>>>,
    reader: Option<Mutex<Connection>>,
    writer: Option<Sender<(i64, Arc<Thumb>)>>,
}

impl ThumbCache {
    /// db_path가 None이면 메모리 캐시만 쓴다 (측정용).
    pub fn open(db_path: Option<&Path>, mem_entries: usize) -> anyhow::Result<Self> {
        let mem = Mutex::new(LruCache::new(NonZeroUsize::new(mem_entries.max(1)).unwrap()));
        let Some(db_path) = db_path else {
            return Ok(Self { mem, reader: None, writer: None });
        };
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let w = Connection::open(db_path)?;
        w.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS thumbs (
                 k INTEGER PRIMARY KEY,
                 w INTEGER NOT NULL,
                 h INTEGER NOT NULL,
                 m INTEGER NOT NULL,
                 t INTEGER NOT NULL,
                 data BLOB NOT NULL
             );",
        )?;
        let r = Connection::open(db_path)?;
        r.execute_batch("PRAGMA mmap_size=268435456;")?;
        let (tx, rx) = unbounded();
        std::thread::Builder::new()
            .name("thumb-db-writer".into())
            .spawn(move || writer_loop(w, rx))?;
        Ok(Self {
            mem,
            reader: Some(Mutex::new(r)),
            writer: Some(tx),
        })
    }

    pub fn get_mem(&self, k: i64) -> Option<Arc<Thumb>> {
        self.mem.lock().get(&k).cloned()
    }

    pub fn get_disk(&self, k: i64) -> Option<Arc<Thumb>> {
        let conn = self.reader.as_ref()?.lock();
        let row = conn
            .prepare_cached("SELECT w, h, m, data FROM thumbs WHERE k = ?1")
            .ok()?
            .query_row(params![k], |r| {
                Ok(Thumb {
                    width: r.get::<_, i64>(0)? as u32,
                    height: r.get::<_, i64>(1)? as u32,
                    mime: mime_of(r.get(2)?),
                    bytes: r.get(3)?,
                })
            })
            .optional()
            .ok()??;
        drop(conn);
        let t = Arc::new(row);
        self.mem.lock().put(k, t.clone());
        Some(t)
    }

    pub fn on_disk(&self, k: i64) -> bool {
        let Some(conn) = self.reader.as_ref() else {
            return false;
        };
        let conn = conn.lock();
        conn.prepare_cached("SELECT 1 FROM thumbs WHERE k = ?1")
            .and_then(|mut s| s.exists(params![k]))
            .unwrap_or(false)
    }

    pub fn put(&self, k: i64, t: Arc<Thumb>) {
        self.mem.lock().put(k, t.clone());
        if let Some(w) = &self.writer {
            let _ = w.send((k, t));
        }
    }
}

/// 쓰기는 한 스레드가 모아서 한 트랜잭션으로 처리한다 (디스크 왕복 최소화).
fn writer_loop(mut conn: Connection, rx: Receiver<(i64, Arc<Thumb>)>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        let deadline = Instant::now() + Duration::from_millis(30);
        while batch.len() < 256 {
            match rx.recv_deadline(deadline) {
                Ok(x) => batch.push(x),
                Err(_) => break,
            }
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let Ok(tx) = conn.transaction() else { continue };
        {
            let Ok(mut stmt) = tx.prepare_cached(
                "INSERT OR REPLACE INTO thumbs (k, w, h, m, t, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            ) else {
                continue;
            };
            for (k, t) in &batch {
                let _ = stmt.execute(params![
                    k,
                    t.width as i64,
                    t.height as i64,
                    mime_code(t.mime),
                    now,
                    t.bytes
                ]);
            }
        }
        let _ = tx.commit();
    }
}
