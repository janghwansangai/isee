//! 뷰어용 이미지 준비. 화면 크기로 줄여 디코딩한 픽셀을 압축 없이 그대로 넘긴다
//! (다시 JPEG로 압축하면 HEIC는 120ms가 더 걸렸다, docs/BENCH.md).
//! 다음·이전 사진 미리 불러오기도 같은 경로를 쓰므로, 미리 받은 사진은 메모리에서 즉시 나간다.

use crate::decode::{self, Pixels};
use crate::formats::web_mime;
use crossbeam_channel::{Sender, unbounded};
use lru::LruCache;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug)]
pub enum Payload {
    /// 원본 파일 그대로 (움직이는 GIF처럼 웹뷰가 직접 재생해야 하는 경우)
    Original { bytes: Vec<u8>, mime: &'static str },
    Pixels(Pixels),
}

#[derive(Debug)]
pub struct ViewImage {
    pub payload: Payload,
    /// 회전 적용 후 원본 해상도 (확대 배율 계산용)
    pub full_width: u32,
    pub full_height: u32,
    /// 준비에 걸린 시간 (ms). 화면의 속도 표시용.
    pub prep_ms: f64,
}

impl ViewImage {
    pub fn route(&self) -> &'static str {
        match self.payload {
            Payload::Original { .. } => "original",
            Payload::Pixels(_) => "pixels",
        }
    }
}

pub type ViewCallback = Box<dyn FnOnce(Option<Arc<ViewImage>>) + Send>;
/// (경로, 화면 상자 가로, 세로) — 상자가 0×0이면 원본 해상도
type Key = (String, u32, u32);

struct Inner {
    done: Mutex<LruCache<Key, Arc<ViewImage>>>,
    inflight: Mutex<HashMap<Key, Vec<ViewCallback>>>,
}

pub struct ViewerService {
    inner: Arc<Inner>,
    tx: Sender<Key>,
}

impl ViewerService {
    /// keep: 메모리에 남겨 둘 준비된 사진 수 (화면 크기 RGBA 한 장이 약 20MB).
    pub fn new(workers: usize, keep: usize) -> Self {
        let inner = Arc::new(Inner {
            done: Mutex::new(LruCache::new(NonZeroUsize::new(keep.max(1)).unwrap())),
            inflight: Mutex::new(HashMap::new()),
        });
        let (tx, rx) = unbounded::<Key>();
        for i in 0..workers.max(1) {
            let inner = inner.clone();
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("viewer-{i}"))
                .spawn(move || {
                    while let Ok(key) = rx.recv() {
                        let result = prepare(Path::new(&key.0), key.1, key.2).ok().map(Arc::new);
                        if let Some(r) = &result {
                            inner.done.lock().put(key.clone(), r.clone());
                        }
                        let waiters = inner.inflight.lock().remove(&key).unwrap_or_default();
                        for cb in waiters {
                            cb(result.clone());
                        }
                    }
                })
                .expect("스레드 생성");
        }
        Self { inner, tx }
    }

    /// 사진을 화면 상자(box_w × box_h 픽셀)에 맞출 때 필요한 만큼만 디코딩한다. 0×0이면 원본 해상도.
    pub fn request(&self, path: String, box_w: u32, box_h: u32, cb: ViewCallback) {
        let key = (path, box_w, box_h);
        if let Some(r) = self.inner.done.lock().get(&key).cloned() {
            cb(Some(r));
            return;
        }
        let mut inflight = self.inner.inflight.lock();
        if let Some(w) = inflight.get_mut(&key) {
            w.push(cb);
            return;
        }
        inflight.insert(key.clone(), vec![cb]);
        drop(inflight);
        let _ = self.tx.send(key);
    }

    /// 파일이 지워지거나 바뀌었을 때 남은 결과를 버린다.
    pub fn forget(&self, path: &str) {
        let mut done = self.inner.done.lock();
        let keys: Vec<Key> = done.iter().filter(|(k, _)| k.0 == path).map(|(k, _)| k.clone()).collect();
        for k in keys {
            done.pop(&k);
        }
    }
}

/// 상자에 맞출 때 필요한 긴 변 픽셀 수 (원본보다 크게 키우지는 않음). 0이면 원본.
pub fn fit_long_side(w: u32, h: u32, box_w: u32, box_h: u32) -> u32 {
    if box_w == 0 || box_h == 0 || w == 0 || h == 0 {
        return 0;
    }
    let s = (box_w as f64 / w as f64).min(box_h as f64 / h as f64).min(1.0);
    (w.max(h) as f64 * s).ceil() as u32
}

pub fn prepare(path: &Path, box_w: u32, box_h: u32) -> anyhow::Result<ViewImage> {
    let t0 = Instant::now();
    let (full_width, full_height) = decode::dimensions(path).unwrap_or((0, 0));
    let max_px = match fit_long_side(full_width, full_height, box_w, box_h) {
        0 if box_w > 0 => box_w.max(box_h), // 크기를 모르면 상자 긴 변까지
        n => n,
    };
    let payload = if web_mime(path) == Some("image/gif") {
        Payload::Original {
            bytes: std::fs::read(path)?,
            mime: "image/gif",
        }
    } else {
        Payload::Pixels(decode::screen_rgba(path, max_px)?)
    };
    Ok(ViewImage {
        payload,
        full_width,
        full_height,
        prep_ms: t0.elapsed().as_secs_f64() * 1000.0,
    })
}
