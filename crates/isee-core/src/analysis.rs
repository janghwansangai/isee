//! 비슷한 사진 정리를 위한 사진별 특징값: 비슷함 지문(pHash), 선명도, 노출, 노이즈, 촬영 시각.
//! 한 번 계산하면 디스크에 저장해 두고, 슬라이더를 움직일 때는 저장된 값으로만 다시 묶는다.
//! 분석은 백그라운드에서 하고, 뷰어·비교 화면이 열려 있는 동안에는 멈춘다 (보기 속도 우선).

use crate::cache::key_of;
use crate::decode;
use crate::listing::{nfc, nfd_variant};
use crate::thumbs::Req;
use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 특징값 계산에 쓰는 이미지 크기 (긴 변). JPEG는 1/4 축소 디코딩이 되어 빠르다.
const SIZE: u32 = 1024;
/// 특징값 형식이 바뀌면 올려서 예전 캐시를 버린다
const VERSION: u32 = 5;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Features {
    /// 비슷함 지문 (DCT 기반 64비트). 비트가 적게 다를수록 비슷하다.
    pub phash: u64,
    /// 선명도: 가장 선명한 영역 몇 곳의 라플라시안 분산 (피사체만 초점이 맞은 사진도 공정하게)
    pub sharp: f32,
    /// 밝기 평균 (0~1)
    pub mean: f32,
    /// 하얗게 날아간 비율, 까맣게 묻힌 비율
    pub clip_hi: f32,
    pub clip_lo: f32,
    /// 노이즈 추정값 (Immerkær)
    pub noise: f32,
    pub width: u32,
    pub height: u32,
    /// 촬영 시각 (초). EXIF가 없으면 수정 시각.
    pub taken: f64,
}

fn gray_of(p: &decode::Pixels) -> Vec<f32> {
    p.rgba
        .chunks_exact(4)
        .map(|c| 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32)
        .collect()
}

/// 32×32로 줄인 뒤 DCT 저주파 8×8 계수가 중앙값보다 큰지로 64비트를 만든다.
pub fn phash(gray: &[f32], w: usize, h: usize) -> u64 {
    const N: usize = 32;
    // 칸 평균으로 32×32 축소
    let mut small = [0f32; N * N];
    for y in 0..N {
        let (y0, y1) = (y * h / N, ((y + 1) * h / N).max(y * h / N + 1).min(h));
        for x in 0..N {
            let (x0, x1) = (x * w / N, ((x + 1) * w / N).max(x * w / N + 1).min(w));
            let mut s = 0f32;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    s += gray[yy * w + xx];
                }
            }
            small[y * N + x] = s / ((y1 - y0) * (x1 - x0)) as f32;
        }
    }
    let cos: Vec<[f32; N]> = (0..8)
        .map(|u| {
            let mut row = [0f32; N];
            for (x, c) in row.iter_mut().enumerate() {
                *c = (((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI) / (2 * N) as f32).cos();
            }
            row
        })
        .collect();
    let mut coef = [0f32; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut s = 0f32;
            for y in 0..N {
                let cy = cos[v][y];
                for x in 0..N {
                    s += small[y * N + x] * cos[u][x] * cy;
                }
            }
            coef[v * 8 + u] = s;
        }
    }
    let mut ac: Vec<f32> = coef[1..].to_vec();
    ac.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = ac[ac.len() / 2];
    coef.iter()
        .enumerate()
        .fold(0u64, |bits, (i, &c)| if c > median { bits | (1 << i) } else { bits })
}

/// 5탭 이항 필터(σ≈1)로 가볍게 흐린다: 픽셀 단위 노이즈가 선명도로 잘못 잡히지 않게
fn smooth(gray: &[f32], w: usize, h: usize) -> Vec<f32> {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        let row = &gray[y * w..(y + 1) * w];
        for x in 0..w {
            let mut s = 0f32;
            for (k, &kv) in K.iter().enumerate() {
                s += row[(x + k).saturating_sub(2).min(w - 1)] * kv;
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for (k, &kv) in K.iter().enumerate() {
            let src = &tmp[(y + k).saturating_sub(2).min(h - 1) * w..][..w];
            for (o, &v) in out[y * w..(y + 1) * w].iter_mut().zip(src) {
                *o += v * kv;
            }
        }
    }
    out
}

/// 선명도: 4×4 구역 중 경계가 가장 많은 3곳의 라플라시안 분산을, 그 구역들의 밝기 분산(대비)으로 나눈 값.
/// 대비로 나누므로 같은 장면을 밝게·어둡게 찍어도 선명도는 그대로이고,
/// 하늘이 하얗게 날아간 사진도 경계가 있는 곳만 보므로 부풀려지지 않는다.
fn sharpness(gray: &[f32], w: usize, h: usize) -> f32 {
    if w < 8 || h < 8 {
        return 0.0;
    }
    let sm = smooth(gray, w, h);
    const T: usize = 4;
    // 구역별 (라플라시안 합, 제곱합, 밝기 합, 밝기 제곱합, 개수)
    let mut acc = [[0f64; 5]; T * T];
    for y in 1..h - 1 {
        let ty = (y * T / h).min(T - 1);
        for x in 1..w - 1 {
            let i = y * w + x;
            let lap = (sm[i - w] + sm[i + w] + sm[i - 1] + sm[i + 1] - 4.0 * sm[i]) as f64;
            let g = gray[i] as f64;
            let a = &mut acc[ty * T + (x * T / w).min(T - 1)];
            a[0] += lap;
            a[1] += lap * lap;
            a[2] += g;
            a[3] += g * g;
            a[4] += 1.0;
        }
    }
    let var = |s: f64, sq: f64, n: f64| (sq / n - (s / n).powi(2)).max(0.0);
    let mut tiles: Vec<(f64, f64)> = acc
        .iter()
        .filter(|a| a[4] > 0.0)
        .map(|a| (var(a[0], a[1], a[4]), var(a[2], a[3], a[4])))
        .collect();
    tiles.sort_by(|a, b| b.0.total_cmp(&a.0));
    let top = &tiles[..tiles.len().min(3)];
    let lap: f64 = top.iter().map(|t| t.0).sum();
    let contrast: f64 = top.iter().map(|t| t.1).sum();
    (lap / (contrast + 12.0) * 1000.0) as f32
}

/// Immerkær(1996) 빠른 노이즈 추정
fn noise(gray: &[f32], w: usize, h: usize) -> f32 {
    if w < 3 || h < 3 {
        return 0.0;
    }
    let mut s = 0f64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let g = |dx: isize, dy: isize| gray[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
            let v = g(-1, -1) - 2.0 * g(0, -1) + g(1, -1) - 2.0 * g(-1, 0) + 4.0 * g(0, 0) - 2.0 * g(1, 0)
                + g(-1, 1)
                - 2.0 * g(0, 1)
                + g(1, 1);
            s += v.abs() as f64;
        }
    }
    ((std::f64::consts::FRAC_PI_2).sqrt() * s / (6.0 * ((w - 2) * (h - 2)) as f64)) as f32
}

/// EXIF 촬영 시각 (초, 날짜 기준 상대값으로만 쓴다). 없으면 None.
fn exif_taken(path: &Path) -> Option<f64> {
    let mut r = BufReader::new(File::open(path).ok()?);
    let exif = exif::Reader::new().read_from_container(&mut r).ok()?;
    let ascii = |tag| match &exif.get_field(tag, exif::In::PRIMARY)?.value {
        exif::Value::Ascii(v) => v.first().map(|b| String::from_utf8_lossy(b).trim().to_string()),
        _ => None,
    };
    let s = ascii(exif::Tag::DateTimeOriginal).or_else(|| ascii(exif::Tag::DateTime))?;
    // "2024:05:12 14:03:22"
    let n: Vec<i64> = s
        .split([':', ' '])
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    if n.len() < 6 {
        return None;
    }
    let (y, m, d) = (n[0], n[1], n[2]);
    // Howard Hinnant의 days_from_civil
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let sub = ascii(exif::Tag::SubSecTimeOriginal)
        .and_then(|s| format!("0.{s}").parse::<f64>().ok())
        .unwrap_or(0.0);
    Some((days * 86_400 + n[3] * 3600 + n[4] * 60 + n[5]) as f64 + sub)
}

/// 사진 한 장의 특징값 (12MP JPEG 기준 약 20ms)
pub fn compute(path: &Path, mtime_ms: i64) -> anyhow::Result<Features> {
    // 아직 내려받지 않은 클라우드 파일은 분석하지 않는다 (읽으면 다운로드가 시작됨)
    anyhow::ensure!(!crate::listing::is_online_only(path), "온라인 전용 파일");
    let px = decode::screen_rgba(path, SIZE)?;
    let (w, h) = (px.width as usize, px.height as usize);
    let gray = gray_of(&px);
    let n = gray.len().max(1) as f32;
    let mean = gray.iter().sum::<f32>() / n / 255.0;
    let clip_hi = gray.iter().filter(|&&v| v >= 250.0).count() as f32 / n;
    let clip_lo = gray.iter().filter(|&&v| v <= 5.0).count() as f32 / n;
    let (fw, fh) = decode::dimensions(path).unwrap_or((px.width, px.height));
    Ok(Features {
        phash: phash(&gray, w, h),
        sharp: sharpness(&gray, w, h),
        mean,
        clip_hi,
        clip_lo,
        noise: noise(&gray, w, h),
        width: fw,
        height: fh,
        taken: exif_taken(path).unwrap_or(mtime_ms as f64 / 1000.0),
    })
}

/// 파일 내용 전체의 해시 (완전히 같은 파일 찾기용)
pub fn content_hash(path: &Path) -> Option<u64> {
    if crate::listing::is_online_only(path) {
        return None;
    }
    let mut f = File::open(path).ok()?;
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.digest())
}

// ───────────────────────── 저장소 ─────────────────────────

pub struct FeatureStore {
    conn: Mutex<Connection>,
}

/// 특징값의 열쇠. path는 NFC로 맞춘 경로 (listing::nfc)
fn feature_key(path: &str, r: &Req) -> i64 {
    key_of(path, r.size, r.mtime, 0xA1A0_0000 + VERSION)
}

impl FeatureStore {
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
             CREATE TABLE IF NOT EXISTS features (k INTEGER PRIMARY KEY, data TEXT NOT NULL);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn get(&self, r: &Req) -> Option<Features> {
        let path = nfc(&r.path);
        let k = feature_key(&path, r);
        let conn = self.conn.lock();
        let read = |k: i64| -> Option<String> {
            conn.prepare_cached("SELECT data FROM features WHERE k = ?1")
                .ok()?
                .query_row(params![k], |row| row.get(0))
                .optional()
                .ok()?
        };
        let data = match read(k) {
            Some(d) => d,
            None => {
                // 꼴을 맞추기 전(0.1.5까지) 풀어 쓴(NFD) 경로로 저장한 값이면 새 열쇠로 옮겨 둔다 (다시 분석하지 않게)
                let d = read(feature_key(&nfd_variant(&path)?, r))?;
                let _ = conn.execute("INSERT OR REPLACE INTO features (k, data) VALUES (?1, ?2)", params![k, d]);
                d
            }
        };
        serde_json::from_str(&data).ok()
    }

    pub fn put(&self, r: &Req, f: &Features) {
        let Ok(data) = serde_json::to_string(f) else { return };
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO features (k, data) VALUES (?1, ?2)",
            params![feature_key(&nfc(&r.path), r), data],
        );
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
    /// 클라우드에만 있어(내려받지 않음) 분석하지 않고 넘긴 수
    skipped: usize,
}

struct Inner {
    store: FeatureStore,
    st: Mutex<State>,
    cv: Condvar,
    /// 뷰어·비교 화면이 열려 있으면 true → 분석을 멈춘다
    paused: Arc<AtomicBool>,
    on_progress: Mutex<Option<ProgressFn>>,
}

pub struct AnalysisService {
    inner: Arc<Inner>,
}

impl AnalysisService {
    pub fn new(store: FeatureStore, paused: Arc<AtomicBool>, workers: usize) -> Self {
        let inner = Arc::new(Inner {
            store,
            st: Mutex::new(State::default()),
            cv: Condvar::new(),
            paused,
            on_progress: Mutex::new(None),
        });
        for i in 0..workers.max(1) {
            let inner = inner.clone();
            std::thread::Builder::new()
                .name(format!("analysis-{i}"))
                .spawn(move || worker(inner))
                .expect("스레드 생성");
        }
        Self { inner }
    }

    pub fn set_progress(&self, f: ProgressFn) {
        *self.inner.on_progress.lock() = Some(f);
    }

    /// 이 사진들을 분석한다 (이전 요청은 취소). 이미 저장된 것과, 클라우드에만 있어 읽을 수 없는 것
    /// (읽으면 내려받기가 시작됨)은 끝난 것으로 센다.
    pub fn analyze(&self, reqs: Vec<Req>) -> (usize, usize) {
        let mut todo = VecDeque::new();
        let total = reqs.len();
        let mut skipped = 0;
        for r in reqs {
            if self.inner.store.get(&r).is_some() {
                continue;
            }
            if crate::listing::is_online_only(Path::new(&r.path)) {
                skipped += 1;
            } else {
                todo.push_back(r);
            }
        }
        let done = total - todo.len();
        let mut st = self.inner.st.lock();
        st.generation += 1;
        st.queue = todo;
        st.total = total;
        st.done = done;
        st.skipped = skipped;
        drop(st);
        self.inner.cv.notify_all();
        (done, total)
    }

    pub fn progress(&self) -> (usize, usize) {
        let st = self.inner.st.lock();
        (st.done, st.total)
    }

    /// 시작하지 않고 셈만: (새로 분석할 수, 클라우드에만 있어 넘길 수)
    pub fn pending(&self, reqs: &[Req]) -> (usize, usize) {
        let (mut todo, mut cloud) = (0, 0);
        for r in reqs {
            if self.inner.store.get(r).is_some() {
                continue;
            }
            if crate::listing::is_online_only(Path::new(&r.path)) {
                cloud += 1;
            } else {
                todo += 1;
            }
        }
        (todo, cloud)
    }

    /// 지금 분석 중인 폴더에서 클라우드에만 있어 넘긴 수
    pub fn skipped(&self) -> usize {
        self.inner.st.lock().skipped
    }

    /// 저장된 특징값 (아직 분석 안 된 것은 None)
    pub fn features(&self, reqs: &[Req]) -> Vec<Option<Features>> {
        reqs.iter().map(|r| self.inner.store.get(r)).collect()
    }
}

fn worker(inner: Arc<Inner>) {
    loop {
        let (req, generation) = {
            let mut st = inner.st.lock();
            loop {
                if !inner.paused.load(Ordering::Relaxed)
                    && let Some(r) = st.queue.pop_front()
                {
                    break (r, st.generation);
                }
                inner.cv.wait_for(&mut st, Duration::from_millis(150));
            }
        };
        let result = compute(Path::new(&req.path), req.mtime);
        if let Ok(f) = &result {
            inner.store.put(&req, f);
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

    /// 원·사각형·그라데이션이 섞인 회색 장면 (seed마다 다른 장면)
    fn scene(w: usize, h: usize, seed: u32) -> Vec<f32> {
        let mut g = vec![0f32; w * h];
        let (cx, cy) = ((seed * 37 % 100) as f32 / 100.0, (seed * 61 % 100) as f32 / 100.0);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let mut val = 60.0 + 120.0 * u * (seed % 3) as f32 / 2.0 + 60.0 * v;
                if ((u - cx).powi(2) + (v - cy).powi(2)).sqrt() < 0.2 {
                    val = 230.0;
                }
                if (x / 16 + y / 16 + seed as usize) % 5 == 0 {
                    val -= 40.0;
                }
                g[y * w + x] = val.clamp(0.0, 255.0);
            }
        }
        g
    }

    fn blur(g: &[f32], w: usize, h: usize, r: isize) -> Vec<f32> {
        let mut out = vec![0f32; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                let (mut s, mut n) = (0f32, 0f32);
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (xx, yy) = (x + dx, y + dy);
                        if xx >= 0 && yy >= 0 && xx < w as isize && yy < h as isize {
                            s += g[yy as usize * w + xx as usize];
                            n += 1.0;
                        }
                    }
                }
                out[y as usize * w + x as usize] = s / n;
            }
        }
        out
    }

    #[test]
    fn phash_similar_vs_different() {
        let (w, h) = (256, 192);
        let a = scene(w, h, 1);
        let blurred = blur(&a, w, h, 2);
        let brighter: Vec<f32> = a.iter().map(|v| (v * 1.15 + 10.0).min(255.0)).collect();
        let other = scene(w, h, 2);
        let d = |x: u64, y: u64| (x ^ y).count_ones();
        let (ha, hb, hc, ho) = (phash(&a, w, h), phash(&blurred, w, h), phash(&brighter, w, h), phash(&other, w, h));
        assert!(d(ha, hb) <= 6, "흐린 것 {}", d(ha, hb));
        assert!(d(ha, hc) <= 6, "밝은 것 {}", d(ha, hc));
        assert!(d(ha, ho) >= 16, "다른 장면 {}", d(ha, ho));
    }

    #[test]
    fn sharpness_drops_with_blur() {
        let (w, h) = (256, 192);
        let a = scene(w, h, 3);
        let b = blur(&a, w, h, 3);
        assert!(sharpness(&a, w, h) > sharpness(&b, w, h) * 3.0);
        // 어둡게 찍어도 선명도는 거의 같다
        let dark: Vec<f32> = a.iter().map(|v| v * 0.4).collect();
        let (sa, sd) = (sharpness(&a, w, h), sharpness(&dark, w, h));
        assert!((sa - sd).abs() / sa < 0.1, "{sa} {sd}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn features_saved_under_the_old_nfd_path_are_found() {
        let store = FeatureStore::open(None).unwrap();
        let composed = "/사진/세계박람회/a.jpg";
        let old = Req { path: nfd_variant(composed).unwrap(), size: 10, mtime: 1, index: 0 };
        let f = Features { phash: 42, ..Default::default() };
        // 0.1.4가 폴더를 읽어서 온 NFD 경로로 저장한 값
        store
            .conn
            .lock()
            .execute("INSERT INTO features (k, data) VALUES (?1, ?2)", params![feature_key(&old.path, &old), serde_json::to_string(&f).unwrap()])
            .unwrap();
        let now = Req { path: composed.into(), ..old.clone() };
        assert_eq!(store.get(&now), Some(f.clone()));
        // 새 열쇠로 옮겨 두었다
        let k = feature_key(composed, &now);
        assert!(store.conn.lock().prepare("SELECT 1 FROM features WHERE k = ?1").unwrap().exists(params![k]).unwrap());
        assert_eq!(store.get(&old), Some(f));
    }
}
