//! 클라우드에만 있는 사진 내려받기 (사용자가 고른 때만).
//! 구글 드라이브·iCloud 등에서 아직 내려받지 않은 사진은 읽으면 내려받기가 시작되므로 평소에는 분석하지 않는다.
//! 사용자가 "내려받아 분석"을 고르면 백그라운드에서 2장씩 내려받고, 내려받은 사진을 기록해 둔다.
//! 분석이 끝나면 "다운로드 제거"로 이 컴퓨터에 받은 내용만 한 번에 비운다.
//! 클라우드의 사진은 그대로이고, 분석 결과(파일 크기·수정 시각으로 찾음)도 그대로 남는다.

use crate::listing::{self, nfc};
use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// iSee가 다운로드를 제거할 수 있는가 (맥: File Provider·iCloud. 윈도우는 탐색기에서 "공간 확보")
pub const CAN_EVICT: bool = cfg!(target_os = "macos");

/// 클라우드 전용 파일을 내려받는다: 내용을 끝까지 읽으면 클라우드 앱이 받아 온다.
/// 이미 이 컴퓨터에 있으면 바로 끝난다.
pub fn download(path: &Path) -> anyhow::Result<()> {
    if !listing::online_only(&std::fs::metadata(path)?) {
        return Ok(());
    }
    let mut f = std::fs::File::open(path)?;
    std::io::copy(&mut f, &mut std::io::sink())?;
    anyhow::ensure!(!listing::is_online_only(path), "내려받지 못했습니다");
    Ok(())
}

/// 이 컴퓨터에 받은 내용만 비우고 다시 클라우드에만 둔다 (파일은 지우지 않는다)
pub fn evict(path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSFileManager, NSString, NSURL};
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSFileManager::defaultManager()
            .evictUbiquitousItemAtURL_error(&url)
            .map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        anyhow::bail!("이 컴퓨터에서는 iSee가 다운로드를 제거할 수 없습니다")
    }
}

/// 경로가 있는 디스크의 남은 공간 (바이트). 알 수 없으면 None
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

// ───────────────────────── 내려받은 기록 ─────────────────────────

/// iSee가 내려받은 사진 기록 (사용자가 따로 받아 둔 사진은 건드리지 않으려고)
pub struct Fetched {
    conn: Mutex<Connection>,
}

impl Fetched {
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
             CREATE TABLE IF NOT EXISTS fetched (
                 path TEXT PRIMARY KEY,
                 size INTEGER NOT NULL,
                 at INTEGER NOT NULL
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn add(&self, path: &str, size: u64) {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let _ = self.conn.lock().execute(
            "INSERT OR REPLACE INTO fetched (path, size, at) VALUES (?1, ?2, ?3)",
            params![nfc(path).as_ref(), size as i64, at],
        );
    }

    fn remove(&self, paths: &[String]) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        for p in paths {
            let _ = tx.execute("DELETE FROM fetched WHERE path = ?1", params![p]);
        }
        let _ = tx.commit();
    }

    /// 내려받아 지금도 이 컴퓨터에 있는 사진 (경로, 크기).
    /// 지웠거나 Finder에서 다운로드를 제거해 이미 클라우드에만 있는 것은 기록에서 뺀다.
    pub fn local(&self) -> Vec<(String, u64)> {
        let all: Vec<(String, u64)> = {
            let conn = self.conn.lock();
            let Ok(mut st) = conn.prepare("SELECT path, size FROM fetched ORDER BY at") else { return Vec::new() };
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
        };
        let (keep, gone): (Vec<_>, Vec<_>) = all.into_iter().partition(|(p, _)| {
            std::fs::metadata(p).is_ok_and(|m| !listing::online_only(&m))
        });
        if !gone.is_empty() {
            self.remove(&gone.into_iter().map(|(p, _)| p).collect::<Vec<_>>());
        }
        keep
    }
}

// ───────────────────────── 백그라운드 내려받기 ─────────────────────────

/// 내려받기 진행 (화면·백그라운드 작업 창에 보여 준다)
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    /// 그중 내려받지 못한 수 (인터넷이 끊김 등)
    pub failed: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// 남은 시간 어림 (초). 몇 장 받아 본 뒤부터
    pub eta_secs: Option<u64>,
    pub paused: bool,
}

impl Progress {
    pub fn busy(&self) -> bool {
        self.done < self.total
    }
}

/// "다운로드 제거" 결과
#[derive(Clone, Debug, Default, Serialize)]
pub struct Evicted {
    pub count: usize,
    pub bytes: u64,
    /// 제거하지 못한 수 (Finder에서 "오프라인으로 사용 가능"을 켠 폴더 등)
    pub failed: usize,
    pub error: Option<String>,
}

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;
/// 내려받기가 끝났을 때 (끝까지 받았거나 중단해서 남은 것이 없을 때). 새 요청으로 바뀌면 부르지 않는다.
pub type DoneFn = Box<dyn FnOnce(Progress) + Send>;

struct Job {
    path: String,
    size: u64,
}

#[derive(Default)]
struct State {
    generation: u64,
    queue: VecDeque<Job>,
    /// 지금 내려받는 수
    active: usize,
    p: Progress,
    /// 속도를 재기 시작한 때와 그때까지 받은 바이트 (멈췄다 다시 하면 새로 잰다)
    since: Option<(Instant, u64)>,
    then: Option<DoneFn>,
}

impl State {
    fn progress(&self) -> Progress {
        let mut p = self.p;
        p.eta_secs = None;
        if let Some((t, b)) = self.since
            && !p.paused
            && p.busy()
        {
            let secs = t.elapsed().as_secs_f64();
            let got = p.bytes_done.saturating_sub(b);
            // 3초 넘게, 2장 넘게 받아 본 뒤부터 (처음 한두 장은 연결 준비로 느리다)
            if secs >= 3.0 && p.done >= 2 && got > 0 {
                let rate = got as f64 / secs;
                p.eta_secs = Some((p.bytes_total.saturating_sub(p.bytes_done) as f64 / rate).ceil() as u64);
            }
        }
        p
    }
}

struct Inner {
    log: Fetched,
    st: Mutex<State>,
    cv: Condvar,
    on_progress: Mutex<Option<ProgressFn>>,
}

pub struct Fetcher {
    inner: Arc<Inner>,
}

impl Fetcher {
    /// workers: 동시에 내려받을 수 (클라우드 앱과 인터넷에 무리가 가지 않게 2)
    pub fn new(log: Fetched, workers: usize) -> Self {
        let inner = Arc::new(Inner { log, st: Mutex::new(State::default()), cv: Condvar::new(), on_progress: Mutex::new(None) });
        for i in 0..workers.max(1) {
            let inner = inner.clone();
            std::thread::Builder::new()
                .name(format!("cloud-{i}"))
                .spawn(move || worker(inner))
                .expect("스레드 생성");
        }
        Self { inner }
    }

    pub fn set_progress(&self, f: ProgressFn) {
        *self.inner.on_progress.lock() = Some(f);
    }

    /// 이 파일들(경로, 크기)을 내려받는다 (하던 것은 그만둔다). 다 받으면 then을 부른다.
    pub fn start(&self, files: Vec<(String, u64)>, then: DoneFn) -> Progress {
        let p = {
            let mut st = self.inner.st.lock();
            st.generation += 1;
            st.p = Progress {
                total: files.len(),
                bytes_total: files.iter().map(|f| f.1).sum(),
                paused: st.p.paused,
                ..Default::default()
            };
            st.queue = files.into_iter().map(|(path, size)| Job { path, size }).collect();
            st.since = Some((Instant::now(), 0));
            st.then = Some(then);
            st.progress()
        };
        self.inner.cv.notify_all();
        if p.total == 0 {
            finish(&self.inner);
        }
        self.notify(p);
        p
    }

    /// 남은 것은 받지 않는다. 지금 받는 것이 끝나면 then을 부른다 (받은 만큼 분석하게).
    pub fn stop(&self) {
        let p = {
            let mut st = self.inner.st.lock();
            let left: Vec<Job> = st.queue.drain(..).collect();
            st.p.total -= left.len();
            st.p.bytes_total -= left.iter().map(|j| j.size).sum::<u64>();
            st.progress()
        };
        if !p.busy() {
            finish(&self.inner);
        }
        self.notify(p);
    }

    pub fn set_paused(&self, paused: bool) {
        let p = {
            let mut st = self.inner.st.lock();
            if st.p.paused == paused {
                return;
            }
            st.p.paused = paused;
            if !paused {
                st.since = Some((Instant::now(), st.p.bytes_done));
            }
            st.progress()
        };
        self.inner.cv.notify_all();
        self.notify(p);
    }

    pub fn progress(&self) -> Progress {
        self.inner.st.lock().progress()
    }

    /// iSee가 내려받아 지금도 이 컴퓨터에 있는 사진 (수, 바이트)
    pub fn downloaded(&self) -> (usize, u64) {
        let local = self.inner.log.local();
        (local.len(), local.iter().map(|f| f.1).sum())
    }

    /// 내려받은 사진의 다운로드를 모두 제거한다 (클라우드의 사진과 분석 결과는 그대로).
    /// 내려받는 중에는 하지 않는다.
    pub fn evict_all(&self) -> anyhow::Result<Evicted> {
        anyhow::ensure!(!self.progress().busy(), "내려받는 중에는 제거할 수 없습니다. 끝나거나 멈춘 뒤에 해 주세요.");
        let mut out = Evicted::default();
        let mut removed = Vec::new();
        for (path, size) in self.inner.log.local() {
            match evict(Path::new(&path)) {
                Ok(()) => {
                    out.count += 1;
                    out.bytes += size;
                    removed.push(path);
                }
                Err(e) => {
                    out.failed += 1;
                    out.error.get_or_insert_with(|| e.to_string());
                }
            }
        }
        self.inner.log.remove(&removed);
        Ok(out)
    }

    fn notify(&self, p: Progress) {
        if let Some(cb) = self.inner.on_progress.lock().clone() {
            cb(p);
        }
    }
}

/// 남은 것이 없으면 then을 한 번 부른다
fn finish(inner: &Inner) {
    let (then, p) = {
        let mut st = inner.st.lock();
        if !st.queue.is_empty() || st.active > 0 {
            return;
        }
        (st.then.take(), st.progress())
    };
    if let Some(f) = then {
        f(p);
    }
}

fn worker(inner: Arc<Inner>) {
    loop {
        let (job, generation) = {
            let mut st = inner.st.lock();
            loop {
                if !st.p.paused
                    && let Some(j) = st.queue.pop_front()
                {
                    st.active += 1;
                    break (j, st.generation);
                }
                inner.cv.wait_for(&mut st, Duration::from_millis(500));
            }
        };
        let was_cloud = listing::is_online_only(Path::new(&job.path));
        let ok = download(Path::new(&job.path)).is_ok();
        // 새 요청으로 바뀌었어도 받은 것은 기록한다 (나중에 제거할 수 있게)
        if ok && was_cloud {
            inner.log.add(&job.path, job.size);
        }
        let p = {
            let mut st = inner.st.lock();
            st.active -= 1;
            if st.generation != generation {
                continue;
            }
            st.p.done += 1;
            st.p.bytes_done += job.size;
            if !ok {
                st.p.failed += 1;
            }
            st.progress()
        };
        if let Some(cb) = inner.on_progress.lock().clone() {
            cb(p);
        }
        if !p.busy() {
            finish(&inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn local_files_count_as_downloaded_and_then_runs_once() {
        let dir = std::env::temp_dir().join(format!("isee-cloud-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<(String, u64)> = (0..5)
            .map(|i| {
                let p = dir.join(format!("{i}.jpg"));
                std::fs::write(&p, vec![0u8; 100]).unwrap();
                (p.to_string_lossy().into_owned(), 100)
            })
            .collect();
        let f = Fetcher::new(Fetched::open(None).unwrap(), 2);
        let (tx, rx) = mpsc::channel();
        f.start(files, Box::new(move |p| tx.send(p).unwrap()));
        let p = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((p.done, p.total, p.failed, p.bytes_done), (5, 5, 0, 500));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "then은 한 번만");
        // 원래 이 컴퓨터에 있던 파일은 "내려받은 것"으로 기록하지 않는다 (제거 대상 아님)
        assert_eq!(f.downloaded(), (0, 0));
        assert_eq!(f.evict_all().unwrap().count, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_files_fail_and_empty_start_finishes() {
        let f = Fetcher::new(Fetched::open(None).unwrap(), 2);
        let (tx, rx) = mpsc::channel();
        f.start(vec![("/없는/사진.jpg".into(), 10)], Box::new(move |p| tx.send(p).unwrap()));
        let p = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((p.done, p.failed), (1, 1));
        let (tx, rx) = mpsc::channel();
        f.start(Vec::new(), Box::new(move |p| tx.send(p).unwrap()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap().total, 0);
    }

    #[test]
    fn records_drop_files_that_are_gone() {
        let log = Fetched::open(None).unwrap();
        log.add("/없는/사진.jpg", 10);
        assert!(log.local().is_empty());
        assert_eq!(log.conn.lock().query_row("SELECT COUNT(*) FROM fetched", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }
}
