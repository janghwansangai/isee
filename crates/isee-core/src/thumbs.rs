//! 썸네일 생성 순서 관리.
//! 1) 화면에 보이는 칸 → 2) 요청은 왔지만 화면 밖 → 3) 폴더 전체 미리 만들기.
//! 뷰어가 열려 있는 동안에는 3번(백그라운드)을 멈춰서 사진 넘기기를 방해하지 않는다.

use crate::cache::{Thumb, ThumbCache, key_of};
use crossbeam_channel::{Sender, unbounded};
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub type Callback = Box<dyn FnOnce(Option<Arc<Thumb>>) + Send>;

#[derive(Clone, Debug)]
pub struct Req {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    /// 현재 폴더 목록에서의 위치 (화면과의 거리 계산용)
    pub index: i64,
}

struct Task {
    path: PathBuf,
    index: i64,
    waiters: Vec<Callback>,
}

#[derive(Default)]
struct State {
    tasks: HashMap<i64, Task>,
    failed: HashSet<i64>,
    generation: u64,
    viewport: (i64, i64),
    viewer_active: bool,
}

#[derive(Default)]
pub struct Stats {
    pub mem_hits: AtomicU64,
    pub disk_hits: AtomicU64,
    pub generated: AtomicU64,
    pub gen_micros: AtomicU64,
    pub failed: AtomicU64,
}

struct Inner {
    cache: ThumbCache,
    px: u32,
    st: Mutex<State>,
    cv: Condvar,
    stats: Stats,
}

pub struct ThumbService {
    inner: Arc<Inner>,
    lookup_tx: Sender<(i64, Req, Callback)>,
}

impl ThumbService {
    pub fn new(cache: ThumbCache, px: u32, workers: usize) -> Self {
        let inner = Arc::new(Inner {
            cache,
            px,
            st: Mutex::new(State::default()),
            cv: Condvar::new(),
            stats: Stats::default(),
        });
        for i in 0..workers.max(1) {
            let inner = inner.clone();
            std::thread::Builder::new()
                .name(format!("thumb-{i}"))
                .spawn(move || worker_loop(inner))
                .expect("스레드 생성");
        }
        // 디스크 캐시 확인은 생성 작업과 따로 돌려서, 생성이 밀려 있어도 캐시된 썸네일은 바로 나가게 한다.
        let (lookup_tx, lookup_rx) = unbounded::<(i64, Req, Callback)>();
        for i in 0..2 {
            let inner = inner.clone();
            let rx = lookup_rx.clone();
            std::thread::Builder::new()
                .name(format!("thumb-lookup-{i}"))
                .spawn(move || {
                    while let Ok((k, req, cb)) = rx.recv() {
                        if let Some(t) = inner.cache.get_disk(k) {
                            inner.stats.disk_hits.fetch_add(1, Ordering::Relaxed);
                            cb(Some(t));
                        } else {
                            enqueue(&inner, k, req, Some(cb));
                        }
                    }
                })
                .expect("스레드 생성");
        }
        Self { inner, lookup_tx }
    }

    pub fn stats(&self) -> &Stats {
        &self.inner.stats
    }

    pub fn key(&self, path: &str, size: u64, mtime: i64) -> i64 {
        key_of(path, size, mtime, self.inner.px)
    }

    /// 썸네일 하나 요청. 준비되면 cb가 호출된다 (메모리에 있으면 즉시).
    pub fn request(&self, req: Req, cb: Callback) {
        let k = self.key(&req.path, req.size, req.mtime);
        if let Some(t) = self.inner.cache.get_mem(k) {
            self.inner.stats.mem_hits.fetch_add(1, Ordering::Relaxed);
            cb(Some(t));
            return;
        }
        if self.inner.st.lock().failed.contains(&k) {
            cb(None);
            return;
        }
        let _ = self.lookup_tx.send((k, req, cb));
    }

    /// 다른 폴더로 이동: 이전 폴더의 대기 작업을 버리고, 새 폴더 전체를 백그라운드로 미리 만든다.
    pub fn set_folder(&self, items: Vec<Req>) {
        let generation = {
            let mut st = self.inner.st.lock();
            st.generation += 1;
            for (_, t) in st.tasks.drain() {
                for cb in t.waiters {
                    cb(None);
                }
            }
            st.viewport = (0, 0);
            st.generation
        };
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("thumb-prefill".into())
            .spawn(move || {
                for req in items {
                    if inner.st.lock().generation != generation {
                        return; // 그사이 또 다른 폴더로 옮겨 감
                    }
                    let k = key_of(&req.path, req.size, req.mtime, inner.px);
                    if inner.cache.get_mem(k).is_some() || inner.cache.on_disk(k) {
                        continue;
                    }
                    enqueue(&inner, k, req, None);
                }
            })
            .expect("스레드 생성");
    }

    pub fn set_viewport(&self, start: i64, end: i64) {
        self.inner.st.lock().viewport = (start, end);
        self.inner.cv.notify_all();
    }

    pub fn set_viewer_active(&self, active: bool) {
        self.inner.st.lock().viewer_active = active;
        self.inner.cv.notify_all();
    }

    /// 남은 작업 수 (측정·진행 표시용)
    pub fn pending(&self) -> usize {
        self.inner.st.lock().tasks.len()
    }
}

fn enqueue(inner: &Inner, k: i64, req: Req, cb: Option<Callback>) {
    let mut st = inner.st.lock();
    let task = st.tasks.entry(k).or_insert_with(|| Task {
        path: PathBuf::from(&req.path),
        index: req.index,
        waiters: Vec::new(),
    });
    task.index = req.index;
    if let Some(cb) = cb {
        task.waiters.push(cb);
    }
    drop(st);
    inner.cv.notify_one();
}

/// 다음에 만들 썸네일 고르기: 기다리는 요청 + 화면 안 → 기다리는 요청 → 백그라운드 순.
fn pick(st: &State) -> Option<i64> {
    let (vs, ve) = st.viewport;
    st.tasks
        .iter()
        .filter(|(_, t)| !st.viewer_active || !t.waiters.is_empty())
        .min_by_key(|(_, t)| {
            let dist = if t.index < vs {
                vs - t.index
            } else if t.index > ve {
                t.index - ve
            } else {
                0
            };
            let tier = match (!t.waiters.is_empty(), dist == 0) {
                (true, true) => 0,
                (true, false) => 1,
                _ => 2,
            };
            (tier, dist, t.index)
        })
        .map(|(k, _)| *k)
}

fn worker_loop(inner: Arc<Inner>) {
    loop {
        let (k, task, generation) = {
            let mut st = inner.st.lock();
            loop {
                if let Some(k) = pick(&st) {
                    let t = st.tasks.remove(&k).unwrap();
                    break (k, t, st.generation);
                }
                inner.cv.wait_for(&mut st, Duration::from_millis(200));
            }
        };
        let t0 = Instant::now();
        let result = crate::decode::thumbnail(&task.path, inner.px);
        let thumb = match result {
            Ok(e) => {
                inner.stats.generated.fetch_add(1, Ordering::Relaxed);
                inner
                    .stats
                    .gen_micros
                    .fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
                let t = Arc::new(Thumb {
                    bytes: e.bytes,
                    width: e.width,
                    height: e.height,
                    mime: e.mime,
                });
                inner.cache.put(k, t.clone());
                Some(t)
            }
            Err(_) => {
                inner.stats.failed.fetch_add(1, Ordering::Relaxed);
                let mut st = inner.st.lock();
                st.failed.insert(k);
                None
            }
        };
        // 만드는 사이 같은 썸네일을 또 요청한 경우 (새 작업으로 들어와 있음)도 함께 응답한다.
        let late = {
            let mut st = inner.st.lock();
            if st.generation == generation && thumb.is_some() {
                st.tasks.remove(&k).map(|t| t.waiters).unwrap_or_default()
            } else {
                Vec::new()
            }
        };
        for cb in task.waiters.into_iter().chain(late) {
            cb(thumb.clone());
        }
    }
}
