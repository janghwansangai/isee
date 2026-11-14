//! 얼굴 인식 연구용 측정 (docs/FACE-RESEARCH.md). 벤치에만 있고 앱의 동작과는 무관하다.
//!   isee-bench face-lab speed <이름=파일.onnx:종류>...
//!       모델 읽기 시간, 파일 크기, 얼굴 하나의 지문 시간 (한 코어)
//!   isee-bench face-lab embed <모델 폴더> <faces.db 복사본> <lab.db> <이름=파일.onnx:종류>...
//!       faces.db의 얼굴을 사진에서 다시 찾아 후보 모델로 지문을 만들어 lab.db에 넣는다 (이 컴퓨터에 있는 사진만)
//!   isee-bench face-lab eval <faces.db 복사본> <lab.db>
//!       사용자가 붙인 이름(같은 사람)과 "같은 사진의 두 얼굴은 다른 사람"으로 모델·묶는 방법을 비교한다
//!   isee-bench face-lab lfw <모델 폴더> <lfw 폴더> <pairs.txt> <lab.db> [--folds N] [--sizes 0,36,24] <이름=파일:종류>...
//!       공개 사진(LFW)으로 같은 사람 맞히기 정확도. 크기 0은 원래 크기, 그 밖은 얼굴을 그 픽셀로 줄여서
//!   isee-bench face-lab lfw-eval <pairs.txt> <lab.db>
//! 종류(입력 형식): sface = RGB 0~255 · arc = RGB (x-127.5)/127.5 · ada = BGR (x-127.5)/127.5

use anyhow::Context as _;
use isee_core::decode::{self, Pixels};
use isee_core::faces::{Face, Models, similarity_of};
use isee_core::listing;
use isee_core::people::{FaceDb, FaceRow, MIN_FACE_PX, Options, group};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tract_onnx::prelude::*;

/// 앱이 분석할 때 읽는 사진 크기 (people.rs의 SIZE와 같게)
const SIZE: u32 = 3072;
const REC: usize = 112;
/// ArcFace 계열이 학습한 얼굴 위치 (faces.rs의 TEMPLATE과 같음)
const TEMPLATE: [[f32; 2]; 5] = [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

pub fn main(args: &[String]) -> anyhow::Result<()> {
    let p = |i: usize| -> anyhow::Result<&Path> { Ok(Path::new(args.get(i).context("인자가 모자람 (파일 맨 위 설명 참고)")?)) };
    match args.first().map(String::as_str) {
        Some("speed") => speed(&args[1..]),
        Some("embed") => embed_db(p(1)?, p(2)?, p(3)?, &args[4..]),
        Some("app") => app_embed(p(1)?, p(2)?, p(3)?, args.get(4).map_or("core", String::as_str)),
        Some("eval") => eval(p(1)?, p(2)?),
        Some("lfw") => lfw(p(1)?, p(2)?, p(3)?, p(4)?, &args[5..]),
        Some("lfw-eval") => lfw_eval(p(1)?, p(2)?),
        _ => anyhow::bail!("face-lab speed | embed | eval | lfw | lfw-eval"),
    }
}

// ───────────────────────── 지문 모델 ─────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Prep {
    Sface,
    Arc,
    Ada,
}

struct Embedder {
    name: String,
    plan: Arc<TypedRunnableModel>,
    prep: Prep,
    load_ms: f64,
    bytes: u64,
}

impl Embedder {
    /// "이름=파일.onnx:종류"
    fn load(spec: &str) -> anyhow::Result<Self> {
        let (name, rest) = spec.split_once('=').context("이름=파일:종류")?;
        let (file, kind) = rest.rsplit_once(':').context("이름=파일:종류")?;
        let prep = match kind {
            "sface" => Prep::Sface,
            "arc" => Prep::Arc,
            "ada" => Prep::Ada,
            _ => anyhow::bail!("종류는 sface | arc | ada"),
        };
        let t = Instant::now();
        let plan = tract_onnx::onnx()
            .model_for_path(file)
            .with_context(|| format!("모델을 읽을 수 없음: {file}"))?
            .with_input_fact(0, f32::fact([1, 3, REC, REC]).into())?
            .into_optimized()?
            .into_runnable()?;
        Ok(Self { name: name.into(), plan, prep, load_ms: t.elapsed().as_secs_f64() * 1000.0, bytes: std::fs::metadata(file)?.len() })
    }

    fn run(&self, aligned: &[[f32; 3]], flip: bool) -> anyhow::Result<Vec<f32>> {
        let mut input = Tensor::zero::<f32>(&[1, 3, REC, REC])?;
        {
            let mut v = input.to_plain_array_view_mut::<f32>()?;
            for y in 0..REC {
                for x in 0..REC {
                    let sx = if flip { REC - 1 - x } else { x };
                    let p = aligned[y * REC + sx];
                    for c in 0..3 {
                        v[[0, c, y, x]] = match self.prep {
                            Prep::Sface => p[c],
                            Prep::Arc => (p[c] - 127.5) / 127.5,
                            Prep::Ada => (p[2 - c] - 127.5) / 127.5,
                        };
                    }
                }
            }
        }
        let out = self.plan.run(tvec!(input.into()))?;
        Ok(out[0].to_plain_array_view::<f32>()?.iter().copied().collect())
    }

    /// 앱과 같은 방식: 정렬한 얼굴과 좌우로 뒤집은 얼굴의 지문을 더해 길이 1로. 두 번째 값은 정규화 전 길이의 평균.
    fn embed(&self, aligned: &[[f32; 3]], flip: bool) -> anyhow::Result<(Vec<f32>, f32)> {
        let mut sum = self.run(aligned, false)?;
        let mut norms = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
        let mut runs = 1.0;
        if flip {
            let f = self.run(aligned, true)?;
            norms += f.iter().map(|v| v * v).sum::<f32>().sqrt();
            sum.iter_mut().zip(&f).for_each(|(a, b)| *a += b);
            runs = 2.0;
        }
        Ok((unit(sum), norms / runs))
    }
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

/// 눈·코·입 5점으로 얼굴을 112×112에 맞춰 자른다 (faces.rs의 fingerprint와 같은 계산)
fn align(px: &Pixels, marks: &[[f32; 2]; 5]) -> Vec<[f32; 3]> {
    let n = 5.0;
    let mean = |p: &[[f32; 2]; 5]| [p.iter().map(|v| v[0]).sum::<f32>() / n, p.iter().map(|v| v[1]).sum::<f32>() / n];
    let (ms, md) = (mean(marks), mean(&TEMPLATE));
    let (mut sxx, mut sxy, mut var) = (0f32, 0f32, 0f32);
    for i in 0..5 {
        let (sx, sy) = (marks[i][0] - ms[0], marks[i][1] - ms[1]);
        let (dx, dy) = (TEMPLATE[i][0] - md[0], TEMPLATE[i][1] - md[1]);
        sxx += sx * dx + sy * dy;
        sxy += sx * dy - sy * dx;
        var += sx * sx + sy * sy;
    }
    let (a, b) = (sxx / var.max(1e-6), sxy / var.max(1e-6));
    let m = [a, -b, md[0] - (a * ms[0] - b * ms[1]), b, a, md[1] - (b * ms[0] + a * ms[1])];
    let det = m[0] * m[4] - m[1] * m[3];
    let (ia, ib, ic, id) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
    let inv = [ia, ib, -(ia * m[2] + ib * m[5]), ic, id, -(ic * m[2] + id * m[5])];
    let mut out = vec![[0f32; 3]; REC * REC];
    for y in 0..REC {
        for x in 0..REC {
            let (fx, fy) = (x as f32, y as f32);
            out[y * REC + x] = sample(px, inv[0] * fx + inv[1] * fy + inv[2], inv[3] * fx + inv[4] * fy + inv[5]);
        }
    }
    out
}

/// 쌍선형 샘플링 (밖이면 0) → [R, G, B]
fn sample(px: &Pixels, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (px.width as i64, px.height as i64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let get = |xx: i64, yy: i64| -> [f32; 3] {
        if xx < 0 || yy < 0 || xx >= w || yy >= h {
            return [0.0; 3];
        }
        let i = ((yy * w + xx) * 4) as usize;
        [px.rgba[i] as f32, px.rgba[i + 1] as f32, px.rgba[i + 2] as f32]
    };
    let (a, b, c, d) = (get(x0, y0), get(x0 + 1, y0), get(x0, y0 + 1), get(x0 + 1, y0 + 1));
    let mut out = [0f32; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bot = c[k] + (d[k] - c[k]) * fx;
        out[k] = top + (bot - top) * fy;
    }
    out
}

fn overlap(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let iy = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    ix * iy / (a[2] * a[3] + b[2] * b[3] - ix * iy)
}

fn blob(e: &[f32]) -> Vec<u8> {
    e.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn unblob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn open_lab(path: &Path) -> anyhow::Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS emb (model TEXT NOT NULL, face INTEGER NOT NULL, emb BLOB NOT NULL, norm REAL, PRIMARY KEY (model, face));
         CREATE TABLE IF NOT EXISTS lfw (model TEXT NOT NULL, image TEXT NOT NULL, size INTEGER NOT NULL, emb BLOB NOT NULL, PRIMARY KEY (model, image, size));
         CREATE TABLE IF NOT EXISTS lfw_meta (image TEXT PRIMARY KEY, px REAL, found INTEGER);",
    )?;
    Ok(conn)
}

// ───────────────────────── 속도 ─────────────────────────

fn speed(specs: &[String]) -> anyhow::Result<()> {
    println!("| 모델 | 파일 크기 | 읽기 | 지문 1번 (한 코어) | 얼굴 하나 (좌우 뒤집기 포함 2번) | 지문 길이 |");
    println!("|---|---|---|---|---|---|");
    for s in specs {
        let e = Embedder::load(s)?;
        // 흉내 낸 얼굴 (부드러운 무늬). 속도는 내용과 거의 무관하다.
        let aligned: Vec<[f32; 3]> = (0..REC * REC).map(|i| [(i % 251) as f32, (i % 127) as f32 * 2.0, (i % 83) as f32 * 3.0]).collect();
        let mut ms = Vec::new();
        let mut dim = 0;
        for _ in 0..12 {
            let t = Instant::now();
            dim = e.run(&aligned, false)?.len();
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        ms.sort_by(f64::total_cmp);
        let med = ms[ms.len() / 2];
        println!("| {} | {:.1}MB | {:.0}ms | {med:.1}ms | {:.1}ms | {dim} |", e.name, e.bytes as f64 / 1e6, e.load_ms, med * 2.0);
    }
    Ok(())
}

// ───────────────────────── 사용자 사진의 얼굴을 후보 모델로 다시 지문 ─────────────────────────

/// 얼굴 하나를 크게 잘라 다시 찾아서 눈·코·입 위치를 정밀하게 잡는다.
/// 단체 사진의 작은 얼굴은 처음 찾을 때 17px쯤으로 줄어든 그림에서 위치를 잡아 오차가 크다.
/// rect: 얼굴 상자 [x, y, 가로, 세로] (img의 픽셀). 못 찾으면 None.
fn refine_marks(det: &Models, img: &Pixels, rect: [f32; 4], zoom: f32) -> Option<[[f32; 2]; 5]> {
    let side = rect[2].max(rect[3]) * zoom;
    let (cx, cy) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    let (x0, y0) = ((cx - side / 2.0).round() as i64, (cy - side / 2.0).round() as i64);
    let n = (side.round() as i64).max(8);
    let (w, h) = (img.width as i64, img.height as i64);
    let mut rgba = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        let sy = y0 + y;
        if sy < 0 || sy >= h {
            continue;
        }
        for x in 0..n {
            let sx = x0 + x;
            if sx < 0 || sx >= w {
                continue;
            }
            let (s, d) = (((sy * w + sx) * 4) as usize, ((y * n + x) * 4) as usize);
            rgba[d..d + 4].copy_from_slice(&img.rgba[s..s + 4]);
        }
    }
    let crop = Pixels { rgba, width: n as u32, height: n as u32 };
    let found = det.detect(&crop).ok()?;
    // 자른 그림 가운데에 있고 크기가 비슷한 얼굴
    let want = rect[2].max(rect[3]) / side;
    let f = found
        .iter()
        .filter(|f| {
            let s = f.rect[2].max(f.rect[3]);
            let off = ((f.rect[0] + f.rect[2] / 2.0 - 0.5).powi(2) + (f.rect[1] + f.rect[3] / 2.0 - 0.5).powi(2)).sqrt();
            s > want * 0.6 && s < want * 1.6 && off < want * 0.5
        })
        .max_by(|a, b| a.score.total_cmp(&b.score))?;
    Some(f.marks.map(|[x, y]| [x + x0 as f32, y + y0 as f32]))
}

fn embed_db(models_dir: &Path, db: &Path, lab: &Path, rest: &[String]) -> anyhow::Result<()> {
    // --full: 지문은 원본 크기 사진에서 뜬다 (찾기는 그대로 3072) · --refine: 얼굴마다 눈·코·입 위치를 다시 잡는다
    // --tag 이름: 결과 이름 뒤에 @이름을 붙인다
    // --zoom 3,5: 다시 잡을 때 얼굴 크기의 몇 배를 자를지 (여러 개면 위치를 평균) · --tta: 조금씩 다르게 자른 얼굴 여러 장의 지문을 평균
    let (mut full, mut refine, mut tag) = (false, false, String::new());
    let mut zooms: Vec<f32> = vec![3.0];
    let mut tta = false;
    let mut specs: Vec<String> = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--full" => full = true,
            "--refine" => refine = true,
            "--tta" => tta = true,
            "--zoom" => zooms = it.next().context("--zoom 3,5")?.split(',').filter_map(|v| v.parse().ok()).collect(),
            "--tag" => tag = format!("@{}", it.next().context("--tag 이름")?),
            _ => specs.push(a.clone()),
        }
    }
    let det = Models::load(models_dir)?;
    let mut embs: Vec<Embedder> = specs.iter().map(|s| Embedder::load(s)).collect::<anyhow::Result<_>>()?;
    embs.iter_mut().for_each(|e| e.name += &tag);
    let refined = std::sync::atomic::AtomicUsize::new(0);
    let faces: Vec<FaceRow> = FaceDb::open(Some(db))?.all_faces().into_iter().filter(|f| f.px >= MIN_FACE_PX && !f.emb.is_empty()).collect();
    let lab = Mutex::new(open_lab(lab)?);
    let done: Vec<HashSet<i64>> = {
        let conn = lab.lock().unwrap();
        embs.iter()
            .map(|e| -> anyhow::Result<HashSet<i64>> {
                let mut st = conn.prepare("SELECT face FROM emb WHERE model = ?1")?;
                Ok(st.query_map([&e.name], |r| r.get(0))?.flatten().collect())
            })
            .collect::<anyhow::Result<_>>()?
    };
    let mut by_photo: HashMap<&str, Vec<&FaceRow>> = HashMap::new();
    for f in &faces {
        by_photo.entry(f.path.as_str()).or_default().push(f);
    }
    let photos: Vec<(&str, Vec<&FaceRow>)> = by_photo.into_iter().collect();
    let t0 = Instant::now();
    let count = std::sync::atomic::AtomicUsize::new(0);
    // (사진 없음, 읽기 실패, 찾은 얼굴, 못 찾은 얼굴)
    let stats: Vec<(usize, usize, usize, usize)> = photos
        .par_iter()
        .map(|(path, list)| {
            let n = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n % 50 == 0 {
                eprintln!("  사진 {n}/{} · {:.0}초", photos.len(), t0.elapsed().as_secs_f64());
            }
            if list.iter().all(|f| done.iter().all(|d| d.contains(&f.id))) {
                return (0, 0, 0, 0);
            }
            let p = Path::new(path);
            // 클라우드에만 있는 사진은 읽으면 내려받기가 시작되므로 건너뛴다
            if !p.exists() || listing::is_online_only(p) {
                return (1, 0, 0, 0);
            }
            let Ok(px) = decode::screen_rgba(p, SIZE) else { return (0, 1, 0, 0) };
            let Ok(found) = det.detect(&px) else { return (0, 1, 0, 0) };
            // 지문을 뜰 그림: 원본 크기(--full)이거나 찾을 때 쓴 그림
            let big = if full { decode::screen_rgba(p, 16384).ok() } else { None };
            let img = big.as_ref().unwrap_or(&px);
            let up = img.width as f32 / px.width as f32;
            let (mut hit, mut miss) = (0, 0);
            let mut rows: Vec<(usize, i64, Vec<f32>, f32)> = Vec::new();
            for f in list {
                let best = found.iter().map(|d| (overlap(&d.rect, &f.rect), d)).max_by(|a, b| a.0.total_cmp(&b.0));
                let Some((o, d)) = best.filter(|(o, _)| *o > 0.4) else {
                    miss += 1;
                    continue;
                };
                let _ = o;
                hit += 1;
                let mut marks = d.marks.map(|[x, y]| [x * up, y * up]);
                if refine {
                    let (w, h) = (img.width as f32, img.height as f32);
                    let got: Vec<[[f32; 2]; 5]> =
                        zooms.iter().filter_map(|&z| refine_marks(&det, img, [d.rect[0] * w, d.rect[1] * h, d.rect[2] * w, d.rect[3] * h], z)).collect();
                    if !got.is_empty() {
                        for (k, m) in marks.iter_mut().enumerate() {
                            *m = [got.iter().map(|g| g[k][0]).sum::<f32>() / got.len() as f32, got.iter().map(|g| g[k][1]).sum::<f32>() / got.len() as f32];
                        }
                        refined.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                // 조금씩 다르게 자른 얼굴들 (크기 ±4%, 기울기 ±3°): 위치 오차에 덜 민감해지는지 본다
                let mut crops = vec![align(img, &marks)];
                if tta {
                    let c = [marks.iter().map(|m| m[0]).sum::<f32>() / 5.0, marks.iter().map(|m| m[1]).sum::<f32>() / 5.0];
                    for (sc, deg) in [(0.96f32, 0.0f32), (1.04, 0.0), (1.0, 3.0), (1.0, -3.0)] {
                        let (co, si) = (deg.to_radians().cos() * sc, deg.to_radians().sin() * sc);
                        let m2 = marks.map(|[x, y]| [c[0] + (x - c[0]) * co - (y - c[1]) * si, c[1] + (x - c[0]) * si + (y - c[1]) * co]);
                        crops.push(align(img, &m2));
                    }
                }
                for (k, e) in embs.iter().enumerate() {
                    if done[k].contains(&f.id) {
                        continue;
                    }
                    // 자른 얼굴마다 (좌우 뒤집기 포함) 지문을 더한다. 정규화 전 값을 더해 선명한 것이 더 크게 반영된다.
                    let mut sum: Vec<f32> = Vec::new();
                    let mut norm = 0.0;
                    for a in &crops {
                        for flip in [false, true] {
                            let Ok(v) = e.run(a, flip) else { continue };
                            norm += v.iter().map(|x| x * x).sum::<f32>().sqrt() / (crops.len() * 2) as f32;
                            if sum.is_empty() {
                                sum = v;
                            } else {
                                sum.iter_mut().zip(&v).for_each(|(a, b)| *a += b);
                            }
                        }
                    }
                    if !sum.is_empty() {
                        rows.push((k, f.id, unit(sum), norm));
                    }
                }
            }
            let conn = lab.lock().unwrap();
            for (k, id, v, norm) in rows {
                let _ = conn.execute("INSERT OR REPLACE INTO emb (model, face, emb, norm) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![embs[k].name, id, blob(&v), norm]);
            }
            (0, 0, hit, miss)
        })
        .collect();
    let s = stats.iter().fold((0, 0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2, a.3 + b.3));
    println!(
        "사진 {}장 (이 컴퓨터에 없어 건너뜀 {}, 읽기 실패 {}) · 얼굴 {}개 가운데 같은 자리에서 다시 찾음 {} · 못 찾음 {} · 위치 다시 잡음 {} · {:.0}초",
        photos.len(),
        s.0,
        s.1,
        faces.len(),
        s.2,
        s.3,
        refined.load(std::sync::atomic::Ordering::Relaxed),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

/// 앱의 분석(people::analyze_one)을 그대로 돌려 그 지문을 lab.db에 넣는다 (이름: name). 앞 40장은 한 장씩 시간을 잰다.
fn app_embed(models_dir: &Path, db: &Path, lab: &Path, name: &str) -> anyhow::Result<()> {
    let det = Models::load(models_dir)?;
    let faces: Vec<FaceRow> = FaceDb::open(Some(db))?.all_faces().into_iter().filter(|f| f.px >= MIN_FACE_PX && !f.emb.is_empty()).collect();
    let conn = Mutex::new(open_lab(lab)?);
    let mut by_photo: HashMap<&str, Vec<&FaceRow>> = HashMap::new();
    for f in &faces {
        by_photo.entry(f.path.as_str()).or_default().push(f);
    }
    let mut photos: Vec<(&str, Vec<&FaceRow>)> = by_photo.into_iter().filter(|(p, _)| Path::new(p).exists() && !listing::is_online_only(Path::new(p))).collect();
    photos.sort_by_key(|p| p.0);
    let (mut ms, mut nf) = (0.0, 0usize);
    for (p, _) in photos.iter().step_by(photos.len() / 40 + 1) {
        let t = Instant::now();
        nf += isee_core::people::analyze_one(&det, Path::new(p)).map_or(0, |v| v.iter().filter(|a| !a.emb.is_empty()).count());
        ms += t.elapsed().as_secs_f64() * 1000.0;
    }
    let np = photos.iter().step_by(photos.len() / 40 + 1).count();
    println!("한 코어로 {np}장: 장당 {:.0}ms · 지문 뜬 얼굴 장당 {:.1}개", ms / np as f64, nf as f64 / np as f64);
    let hit: usize = photos
        .par_iter()
        .map(|(p, list)| {
            let Ok(found) = isee_core::people::analyze_one(&det, Path::new(p)) else { return 0 };
            let mut n = 0;
            let c = conn.lock().unwrap();
            for f in list {
                if let Some(a) = found.iter().filter(|a| !a.emb.is_empty() && overlap(&a.face.rect, &f.rect) > 0.4).max_by(|a, b| overlap(&a.face.rect, &f.rect).total_cmp(&overlap(&b.face.rect, &f.rect))) {
                    let _ = c.execute("INSERT OR REPLACE INTO emb (model, face, emb, norm) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![name, f.id, blob(&a.emb), a.quality]);
                    n += 1;
                }
            }
            n
        })
        .sum();
    println!("얼굴 {}개 가운데 같은 자리에서 다시 찾음 {hit}", faces.len());
    Ok(())
}

// ───────────────────────── 비교 ─────────────────────────

struct F {
    id: i64,
    photo: u32,
    /// 사진이 든 폴더 (같은 행사로 본다)
    folder: u32,
    px: f32,
    q: f32,
    turn: Option<f32>,
    person: Option<i64>,
    emb: Vec<f32>,
}

/// 오름차순으로 정렬된 값에서 p(0~1) 자리의 값
fn quant(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return f32::NAN;
    }
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

/// 다른 사람 쌍 가운데 far 비율만 통과시키는 기준값
fn thr_at_far(neg_sorted: &[f32], far: f64) -> f32 {
    let k = ((neg_sorted.len() as f64) * far).floor() as usize;
    // 위에서 k개만 기준 이상이 되게: k번째로 큰 값보다 조금 큰 값
    if k >= neg_sorted.len() {
        return f32::MIN;
    }
    let v = neg_sorted[neg_sorted.len() - 1 - k];
    f32::from_bits(v.to_bits() + if v >= 0.0 { 1 } else { 0 }).max(v)
}

fn rate_ge(sorted: &[f32], t: f32) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = sorted.partition_point(|&v| v < t);
    (sorted.len() - i) as f64 / sorted.len() as f64
}

/// 같은 사람을 놓치는 비율과 다른 사람을 같다고 보는 비율이 같아지는 점
fn eer(pos: &[f32], neg: &[f32]) -> f64 {
    let (mut lo, mut hi) = (-1.0f32, 1.0f32);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        let (frr, far) = (1.0 - rate_ge(pos, mid), rate_ge(neg, mid));
        if far > frr { lo = mid } else { hi = mid }
    }
    let t = (lo + hi) / 2.0;
    ((1.0 - rate_ge(pos, t)) + rate_ge(neg, t)) / 2.0
}

fn eval(db: &Path, lab: &Path) -> anyhow::Result<()> {
    let all: Vec<FaceRow> = FaceDb::open(Some(db))?.all_faces().into_iter().filter(|f| f.px >= MIN_FACE_PX && !f.emb.is_empty()).collect();
    let conn = open_lab(lab)?;
    let mut models: Vec<String> = conn.prepare("SELECT DISTINCT model FROM emb ORDER BY model")?.query_map([], |r| r.get(0))?.flatten().collect();
    models.insert(0, "앱(SFace, faces.db에 저장된 지문)".into());
    let mut photo_ix: HashMap<&str, u32> = HashMap::new();
    let mut folder_ix: HashMap<String, u32> = HashMap::new();
    let named = all.iter().filter(|f| f.person.is_some()).count();
    let people: HashSet<i64> = all.iter().filter_map(|f| f.person).collect();
    println!("얼굴 {}개 (36px 이상) · 이름 붙은 얼굴 {named}개 · 사람 {}명", all.len(), people.len());

    // 모든 모델에 지문이 있는 얼굴만 비교한다 (같은 얼굴로 견주기)
    let mut per: Vec<HashMap<i64, Vec<f32>>> = Vec::new();
    for m in &models[1..] {
        let mut st = conn.prepare("SELECT face, emb FROM emb WHERE model = ?1")?;
        per.push(st.query_map([m], |r| Ok((r.get::<_, i64>(0)?, unblob(&r.get::<_, Vec<u8>>(1)?))))?.flatten().collect());
    }
    let common: HashSet<i64> = all.iter().map(|f| f.id).filter(|id| per.iter().all(|m| m.contains_key(id))).collect();
    println!("모든 모델로 지문을 만든 얼굴 {}개 (이 얼굴들로만 비교)\n", common.len());

    for (mi, name) in models.iter().enumerate() {
        let faces: Vec<F> = all
            .iter()
            .filter(|f| common.contains(&f.id))
            .map(|f| {
                let n = photo_ix.len() as u32;
                let photo = *photo_ix.entry(f.path.as_str()).or_insert(n);
                let dir = Path::new(&f.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                let n = folder_ix.len() as u32;
                let folder = *folder_ix.entry(dir).or_insert(n);
                let emb = if mi == 0 { f.emb.clone() } else { per[mi - 1][&f.id].clone() };
                F { id: f.id, photo, folder, px: f.px, q: f.q, turn: f.turn, person: f.person, emb }
            })
            .collect();
        println!("════════ {name} (지문 {}칸) ════════", faces.first().map_or(0, |f| f.emb.len()));
        if mi == 1 {
            // 다시 만든 SFace 지문이 앱이 저장한 지문과 같은지 (측정 도구 확인)
            let app: HashMap<i64, &Vec<f32>> = all.iter().map(|f| (f.id, &f.emb)).collect();
            let mut c: Vec<f32> = faces.iter().filter(|_| name.starts_with("sface")).map(|f| similarity_of(&f.emb, app[&f.id])).collect();
            c.sort_by(f32::total_cmp);
            if !c.is_empty() {
                println!("  (확인) 앱이 저장한 지문과의 비슷함: 중간 {:.4} · 하위 1% {:.4}", quant(&c, 0.5), quant(&c, 0.01));
            }
        }
        let report = pairs_report(&faces);
        cluster_report(&faces, &report, mi == 0, &all);
        assign_report(&faces);
        println!();
    }
    Ok(())
}

struct PairStats {
    /// 같은 사진의 두 얼굴 (다른 사람)을 0.1%, 1%만 통과시키는 기준값
    t01: f32,
    t1: f32,
    neg_photo: Vec<f32>,
}

fn pairs_report(faces: &[F]) -> PairStats {
    // 같은 사진의 두 얼굴 = 다른 사람 (이름과 무관, 학급 친구끼리라 가장 어려운 비교)
    let mut by_photo: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        by_photo.entry(f.photo).or_default().push(i);
    }
    let mut neg_photo: Vec<(f32, f32)> = Vec::new();
    for list in by_photo.values() {
        for a in 0..list.len() {
            for b in a + 1..list.len() {
                let (x, y) = (&faces[list[a]], &faces[list[b]]);
                neg_photo.push((similarity_of(&x.emb, &y.emb), x.px.min(y.px)));
            }
        }
    }
    // 이름 붙은 얼굴: 같은 사람 쌍(다른 사진), 다른 사람 쌍
    let lab: Vec<&F> = faces.iter().filter(|f| f.person.is_some()).collect();
    let mut pos: Vec<(f32, f32, bool, f32)> = Vec::new();
    let mut neg_lab: Vec<f32> = Vec::new();
    for a in 0..lab.len() {
        for b in a + 1..lab.len() {
            let (x, y) = (lab[a], lab[b]);
            if x.photo == y.photo {
                continue;
            }
            let s = similarity_of(&x.emb, &y.emb);
            if x.person == y.person {
                pos.push((s, x.px.min(y.px), x.folder == y.folder, x.turn.unwrap_or(0.0).max(y.turn.unwrap_or(0.0))));
            } else {
                neg_lab.push(s);
            }
        }
    }
    let sorted = |v: Vec<f32>| {
        let mut v = v;
        v.sort_by(f32::total_cmp);
        v
    };
    let np = sorted(neg_photo.iter().map(|p| p.0).collect());
    let ps = sorted(pos.iter().map(|p| p.0).collect());
    let nl = sorted(neg_lab);
    let line = |name: &str, v: &[f32]| {
        println!(
            "  {name}: {}쌍 · 1% {:.2} · 10% {:.2} · 중간 {:.2} · 90% {:.2} · 99% {:.2} · 99.9% {:.2} · 최대 {:.2}",
            v.len(),
            quant(v, 0.01),
            quant(v, 0.1),
            quant(v, 0.5),
            quant(v, 0.9),
            quant(v, 0.99),
            quant(v, 0.999),
            quant(v, 1.0)
        )
    };
    line("같은 사람 (이름 붙은 얼굴, 다른 사진)", &ps);
    line("다른 사람 (같은 사진의 두 얼굴)      ", &np);
    line("다른 사람 (이름이 다른 얼굴)         ", &nl);
    let (t1, t01, t001) = (thr_at_far(&np, 0.01), thr_at_far(&np, 0.001), thr_at_far(&np, 0.0001));
    println!(
        "  같은 사진 쌍을 1% / 0.1% / 0.01%만 같다고 보는 기준값 {t1:.3} / {t01:.3} / {t001:.3} → 그 기준에서 같은 사람을 맞힘 **{:.1}% / {:.1}% / {:.1}%** · EER {:.2}%",
        100.0 * rate_ge(&ps, t1),
        100.0 * rate_ge(&ps, t01),
        100.0 * rate_ge(&ps, t001),
        100.0 * eer(&ps, &np)
    );
    let (u1, u01) = (thr_at_far(&nl, 0.01), thr_at_far(&nl, 0.001));
    println!(
        "  이름이 다른 쌍을 1% / 0.1%만 같다고 보는 기준값 {u1:.3} / {u01:.3} → 같은 사람을 맞힘 {:.1}% / {:.1}% · EER {:.2}%",
        100.0 * rate_ge(&ps, u1),
        100.0 * rate_ge(&ps, u01),
        100.0 * eer(&ps, &nl)
    );
    // 얼굴 크기별 (쌍에서 작은 쪽 얼굴 기준), 기준값은 전체 0.1%
    let buckets: [(f32, f32, &str); 4] = [(0.0, 50.0, "36~50px"), (50.0, 80.0, "50~80px"), (80.0, 120.0, "80~120px"), (120.0, 1e9, "120px 이상")];
    let mut s = String::from("  얼굴 크기별 (기준값 0.1%짜리):");
    for (lo, hi, name) in buckets {
        let p: Vec<f32> = sorted(pos.iter().filter(|p| p.1 >= lo && p.1 < hi).map(|p| p.0).collect());
        let n: Vec<f32> = sorted(neg_photo.iter().filter(|p| p.1 >= lo && p.1 < hi).map(|p| p.0).collect());
        s += &format!(" {name} 맞힘 {:.0}%({}쌍)·잘못 {:.2}%({}쌍) |", 100.0 * rate_ge(&p, t01), p.len(), 100.0 * rate_ge(&n, t01), n.len());
    }
    println!("{s}");
    // 같은 폴더(같은 행사) 안의 쌍과 다른 폴더 사이의 쌍
    let same_f: Vec<f32> = sorted(pos.iter().filter(|p| p.2).map(|p| p.0).collect());
    let diff_f: Vec<f32> = sorted(pos.iter().filter(|p| !p.2).map(|p| p.0).collect());
    println!(
        "  같은 사람 쌍: 같은 폴더 안 {}쌍 중간 {:.2}·맞힘 {:.1}% | 다른 폴더 사이 {}쌍 중간 {:.2}·맞힘 {:.1}% (기준값 0.1%짜리)",
        same_f.len(),
        quant(&same_f, 0.5),
        100.0 * rate_ge(&same_f, t01),
        diff_f.len(),
        quant(&diff_f, 0.5),
        100.0 * rate_ge(&diff_f, t01)
    );
    // 옆으로 돌린 정도별 (쌍에서 더 많이 돌린 쪽)
    let mut s = String::from("  돌린 정도별 맞힘:");
    for (lo, hi, name) in [(0.0, 0.35, "정면(~0.35)"), (0.35, 0.6, "조금 돌림(~0.6)"), (0.6, 9.0, "옆모습(0.6~)")] {
        let p: Vec<f32> = sorted(pos.iter().filter(|p| p.3 >= lo && p.3 < hi).map(|p| p.0).collect());
        s += &format!(" {name} {:.0}%({}쌍) |", 100.0 * rate_ge(&p, t01), p.len());
    }
    println!("{s}");
    // 가장 비슷한 다른 이름 붙은 얼굴(다른 사진)이 같은 사람인가
    let (mut right, mut tried) = (0, 0);
    for (a, x) in lab.iter().enumerate() {
        if !lab.iter().any(|y| y.person == x.person && y.photo != x.photo) {
            continue;
        }
        let best = lab
            .iter()
            .enumerate()
            .filter(|(b, y)| *b != a && y.photo != x.photo)
            .map(|(_, y)| (similarity_of(&x.emb, &y.emb), y.person))
            .max_by(|p, q| p.0.total_cmp(&q.0));
        tried += 1;
        right += best.is_some_and(|b| b.1 == x.person) as usize;
    }
    println!("  가장 비슷한 이름 붙은 얼굴이 같은 사람: {right}/{tried} ({:.1}%)", 100.0 * right as f64 / tried.max(1) as f64);
    PairStats { t01, t1, neg_photo: np }
}

// ───────────────────────── 묶기 (people.rs의 방법을 옮겨 와 조건을 바꿔 본다) ─────────────────────────

/// people.rs의 average_linkage와 같은 방법. ban_photo이면 같은 사진의 두 얼굴은 같은 묶음에 넣지 않는다.
/// 돌려주는 값: 얼굴마다 묶음 번호
fn cluster(faces: &[F], seed: f32, same: f32, ban_photo: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..faces.len()).collect();
    order.sort_by(|&a, &b| (faces[b].px * faces[b].q.max(1.0)).total_cmp(&(faces[a].px * faces[a].q.max(1.0))));
    let dim = faces.first().map_or(0, |f| f.emb.len());
    let mut cents: Vec<f32> = Vec::new();
    let mut sums: Vec<Vec<f32>> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut seeds_of_photo: HashMap<u32, Vec<usize>> = HashMap::new();
    for i in order {
        let f = &faces[i];
        let banned: &[usize] = if ban_photo { seeds_of_photo.get(&f.photo).map_or(&[], |v| v.as_slice()) } else { &[] };
        let mut best = (f32::NEG_INFINITY, usize::MAX);
        for (k, c) in cents.chunks_exact(dim.max(1)).enumerate() {
            if banned.contains(&k) {
                continue;
            }
            let s = similarity_of(&f.emb, c);
            if s >= best.0 {
                best = (s, k);
            }
        }
        let at = if best.0 >= seed {
            let k = best.1;
            sums[k].iter_mut().zip(&f.emb).for_each(|(a, b)| *a += b);
            let len = sums[k].iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
            cents[k * dim..(k + 1) * dim].iter_mut().zip(&sums[k]).for_each(|(c, v)| *c = v / len);
            members[k].push(i);
            k
        } else {
            cents.extend_from_slice(&f.emb);
            sums.push(f.emb.clone());
            members.push(vec![i]);
            members.len() - 1
        };
        seeds_of_photo.entry(f.photo).or_default().push(at);
    }
    let n = members.len();
    let mut alive = vec![true; n];
    let size = |m: &Vec<usize>| m.len() as f32;
    let mut sim = vec![f32::MIN; n * n];
    sim.par_chunks_mut(n.max(1)).enumerate().for_each(|(i, row)| {
        for (j, v) in row.iter_mut().enumerate() {
            if j != i {
                *v = similarity_of(&sums[i], &sums[j]) / (size(&members[i]) * size(&members[j]));
            }
        }
    });
    if ban_photo {
        for list in seeds_of_photo.values() {
            for &a in list {
                for &b in list {
                    if a != b {
                        sim[a * n + b] = f32::NEG_INFINITY;
                    }
                }
            }
        }
    }
    let row_best = |sim: &[f32], alive: &[bool], i: usize| -> (f32, usize) {
        let mut b = (f32::NEG_INFINITY, usize::MAX);
        for (j, &v) in sim[i * n..(i + 1) * n].iter().enumerate() {
            if j != i && v > b.0 && alive[j] {
                b = (v, j);
            }
        }
        b
    };
    let mut best: Vec<(f32, usize)> = (0..n).into_par_iter().map(|i| row_best(&sim, &alive, i)).collect();
    loop {
        let mut top = (same, usize::MAX);
        for (i, b) in best.iter().enumerate() {
            if b.0 > top.0 && alive[i] {
                top = (b.0, i);
            }
        }
        if top.1 == usize::MAX {
            break;
        }
        let (i, j) = (top.1.min(best[top.1].1), top.1.max(best[top.1].1));
        let (ni, nj) = (size(&members[i]), size(&members[j]));
        for k in 0..n {
            if k == i || k == j || !alive[k] {
                continue;
            }
            let v = (ni * sim[i * n + k] + nj * sim[j * n + k]) / (ni + nj);
            sim[i * n + k] = v;
            sim[k * n + i] = v;
        }
        let moved = std::mem::take(&mut members[j]);
        members[i].extend(moved);
        alive[j] = false;
        best[j] = (f32::NEG_INFINITY, usize::MAX);
        best[i] = row_best(&sim, &alive, i);
        for k in 0..n {
            if k == i || !alive[k] {
                continue;
            }
            if best[k].1 == i || best[k].1 == j {
                best[k] = row_best(&sim, &alive, k);
            } else if sim[k * n + i] > best[k].0 {
                best[k] = (sim[k * n + i], i);
            }
        }
    }
    let mut out = vec![usize::MAX; faces.len()];
    for (g, m) in members.iter().enumerate() {
        for &i in m {
            out[i] = g;
        }
    }
    out
}

#[derive(Default, Clone, Copy)]
struct Score {
    groups: usize,
    unsorted: usize,
    /// 이름이 같은 얼굴 쌍, 그 가운데 한 묶음에 들어간 쌍
    same: usize,
    same_tog: usize,
    /// 한 묶음에 들어간 이름 붙은 얼굴 쌍, 그 가운데 이름이 다른 쌍
    tog: usize,
    wrong: usize,
    /// 같은 사진의 두 얼굴이 한 묶음에 들어간 쌍 (모든 얼굴, 틀린 것이 확실함)
    photo_viol: usize,
}

/// gid: 얼굴마다 묶음 번호 (혼자인 묶음은 미분류로 본다)
fn score(faces: &[F], gid: &[usize]) -> Score {
    let mut sizes: HashMap<usize, usize> = HashMap::new();
    for &g in gid {
        *sizes.entry(g).or_default() += 1;
    }
    let g = |i: usize| Some(gid[i]).filter(|g| *g != usize::MAX && sizes[g] >= 2);
    let mut s = Score { groups: sizes.values().filter(|&&n| n >= 2).count(), ..Default::default() };
    s.unsorted = (0..faces.len()).filter(|&i| g(i).is_none()).count();
    let lab: Vec<usize> = (0..faces.len()).filter(|&i| faces[i].person.is_some()).collect();
    for a in 0..lab.len() {
        for b in a + 1..lab.len() {
            let (i, j) = (lab[a], lab[b]);
            let together = g(i).is_some() && g(i) == g(j);
            let same = faces[i].person == faces[j].person;
            s.same += same as usize;
            s.same_tog += (same && together) as usize;
            s.tog += together as usize;
            s.wrong += (!same && together) as usize;
        }
    }
    let mut by: HashMap<(u32, usize), usize> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        if let Some(gi) = g(i) {
            *by.entry((f.photo, gi)).or_default() += 1;
        }
    }
    s.photo_viol = by.values().map(|n| n * (n - 1) / 2).sum();
    s
}

fn score_line(name: &str, s: &Score) -> String {
    format!(
        "  {name}: 묶음 {} · 미분류 {} · 같은 사람 쌍 함께 {:.1}% ({}/{}) · 함께 묶인 쌍 중 다른 사람 {:.2}% ({}/{}) · 같은 사진 두 얼굴이 한 묶음 {}쌍",
        s.groups,
        s.unsorted,
        100.0 * s.same_tog as f64 / s.same.max(1) as f64,
        s.same_tog,
        s.same,
        100.0 * s.wrong as f64 / s.tog.max(1) as f64,
        s.wrong,
        s.tog,
        s.photo_viol
    )
}

fn cluster_report(faces: &[F], st: &PairStats, is_app: bool, all: &[FaceRow]) {
    println!("  ── 묶기 (이름을 지우고 전부 다시 묶은 뒤 이름과 비교) ──");
    if is_app {
        // 앱의 실제 묶기 (보통 단계) — 옮겨 온 방법이 같은 결과를 내는지 확인용
        let ids: HashSet<i64> = faces.iter().map(|f| f.id).collect();
        let rows: Vec<FaceRow> = all.iter().filter(|f| ids.contains(&f.id)).map(|f| FaceRow { person: None, not_person: None, ..f.clone() }).collect();
        let r = group(rows, &[], &[], 0, 0, &Options::default());
        let ix: HashMap<i64, usize> = faces.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
        let mut gid = vec![usize::MAX; faces.len()];
        for (g, grp) in r.groups.iter().enumerate() {
            for f in &grp.faces {
                gid[ix[&f.id]] = g;
            }
        }
        println!("{}", score_line("앱의 지금 묶기 (보통 0.50/0.34)        ", &score(faces, &gid)));
    }
    // 기준값을 바꿔 가며 (씨앗 기준 = 합치기 기준 + 0.16, 앱의 보통 단계와 같은 간격을 모델의 눈금에 맞춰 줄임)
    // 모델마다 비슷함의 눈금이 다르므로 "같은 사진 쌍을 얼마나 통과시키는 기준값인가"로 맞춘다.
    let fars = [0.03, 0.02, 0.01, 0.005, 0.002, 0.001];
    let runs: Vec<(f64, f32, f32, bool)> = fars
        .iter()
        .flat_map(|&far| {
            let same = thr_at_far(&st.neg_photo, far);
            // 씨앗 기준: 같은 사진 쌍 통과 비율이 그 1/50인 값
            let seed = thr_at_far(&st.neg_photo, far / 50.0).max(same);
            [(far, seed, same, false), (far, seed, same, true)]
        })
        .collect();
    let scores: Vec<Score> = runs.par_iter().map(|&(_, seed, same, ban)| score(faces, &cluster(faces, seed, same, ban))).collect();
    for (r, s) in runs.iter().zip(&scores) {
        let name = format!("합치기 기준 {:.3}(같은 사진 쌍 {:.1}% 통과)·씨앗 {:.3}{}", r.2, r.0 * 100.0, r.1, if r.3 { " + 같은 사진 금지" } else { "                 " });
        println!("{}", score_line(&name, s));
    }
    let _ = st.t1;
    let _ = st.t01;
    if is_app {
        // 앱의 세 단계 기준값 그대로, 같은 사진 금지를 넣으면
        for (name, seed, same) in [("낮음 0.46/0.30", 0.46, 0.30), ("보통 0.50/0.34", 0.50, 0.34), ("높음 0.55/0.40", 0.55, 0.40)] {
            for ban in [false, true] {
                let s = score(faces, &cluster(faces, seed, same, ban));
                println!("{}", score_line(&format!("{name}{}", if ban { " + 같은 사진 금지" } else { "                 " }), &s));
            }
        }
    }
}

// ───────────────────────── 이름 붙은 사람에게 자동으로 넣기 ─────────────────────────

/// 사람마다 이름 붙은 얼굴 k개만 "아는 얼굴"로 두고, 나머지 이름 붙은 얼굴을 다시 넣어 본다.
/// 얼굴이 10개가 안 되는 사람은 "모르는 사람"으로 두어, 아무에게도 들어가지 않아야 맞다.
fn assign_report(faces: &[F]) {
    println!("  ── 이름 붙은 사람에게 자동으로 넣기 (사람마다 k개만 알려 주고 나머지를 맞히기, 5번 평균) ──");
    let mut by: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        if let Some(p) = f.person {
            by.entry(p).or_default().push(i);
        }
    }
    let mut known: Vec<(i64, Vec<usize>)> = by.iter().filter(|(_, v)| v.len() >= 10).map(|(p, v)| (*p, v.clone())).collect();
    known.sort();
    let strangers: Vec<usize> = by.iter().filter(|(_, v)| v.len() < 10).flat_map(|(_, v)| v.clone()).collect();
    println!("    아는 사람 {}명 · 모르는 사람의 얼굴 {}개", known.len(), strangers.len());
    for k in [1usize, 3, 5, 10] {
        // 방법마다 (점수, 맞음?, 넣을 사람이 있었나)
        let methods = ["평균 지문(지금 방식)", "가까운 3개 평균", "평균 + 같은 사진 한 사람 한 번", "가까운 3개 + 같은 사진 한 번"];
        let mut picks: Vec<Vec<(f32, bool)>> = vec![Vec::new(); methods.len()];
        let mut probes_known = 0usize;
        // 지금 방식의 기준 그대로 (0.45, 차이 0.08) — SFace 눈금에서만 뜻이 있다
        let (mut cur_right, mut cur_wrong) = (0usize, 0usize);
        for rep in 0..5u64 {
            let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ (rep * 7919 + k as u64);
            let mut rnd = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed
            };
            let mut enrolled: Vec<(i64, Vec<usize>)> = Vec::new();
            let mut probes: Vec<(usize, Option<i64>)> = strangers.iter().map(|&i| (i, None)).collect();
            for (p, list) in &known {
                let mut l = list.clone();
                for i in (1..l.len()).rev() {
                    l.swap(i, (rnd() % (i as u64 + 1)) as usize);
                }
                probes.extend(l[k..].iter().map(|&i| (i, Some(*p))));
                enrolled.push((*p, l[..k].to_vec()));
            }
            probes_known += probes.iter().filter(|p| p.1.is_some()).count();
            let cents: Vec<Vec<f32>> = enrolled
                .iter()
                .map(|(_, l)| {
                    let mut m = vec![0f32; faces[l[0]].emb.len()];
                    for &i in l {
                        m.iter_mut().zip(&faces[i].emb).for_each(|(a, b)| *a += b);
                    }
                    unit(m)
                })
                .collect();
            // 사진마다 이미 이름이 붙어 있는 사람 (같은 사진에 또 넣지 않는다)
            let mut taken: HashMap<u32, Vec<usize>> = HashMap::new();
            for (pi, (_, l)) in enrolled.iter().enumerate() {
                for &i in l {
                    taken.entry(faces[i].photo).or_default().push(pi);
                }
            }
            for (mi, _) in methods.iter().enumerate() {
                let (knn, excl) = (mi == 1 || mi == 3, mi >= 2);
                // 얼굴마다 사람별 점수
                let scored: Vec<Vec<f32>> = probes
                    .iter()
                    .map(|&(i, _)| {
                        enrolled
                            .iter()
                            .enumerate()
                            .map(|(pi, (_, l))| {
                                if excl && taken.get(&faces[i].photo).is_some_and(|t| t.contains(&pi)) {
                                    return f32::NEG_INFINITY;
                                }
                                if knn {
                                    let mut s: Vec<f32> = l.iter().map(|&j| similarity_of(&faces[i].emb, &faces[j].emb)).collect();
                                    s.sort_by(|a, b| b.total_cmp(a));
                                    let n = s.len().min(3);
                                    s[..n].iter().sum::<f32>() / n as f32
                                } else {
                                    similarity_of(&faces[i].emb, &cents[pi])
                                }
                            })
                            .collect()
                    })
                    .collect();
                let mut choice: Vec<(f32, usize, f32)> = scored
                    .iter()
                    .map(|s| {
                        let mut o: Vec<usize> = (0..s.len()).collect();
                        o.sort_by(|&a, &b| s[b].total_cmp(&s[a]));
                        (s[o[0]], o[0], o.get(1).map_or(f32::NEG_INFINITY, |&x| s[x]))
                    })
                    .collect();
                if excl {
                    // 같은 사진에서 같은 사람을 고른 얼굴이 여럿이면 점수가 가장 높은 하나만
                    let mut best: HashMap<(u32, usize), (f32, usize)> = HashMap::new();
                    for (n, c) in choice.iter().enumerate() {
                        let key = (faces[probes[n].0].photo, c.1);
                        if best.get(&key).is_none_or(|b| c.0 > b.0) {
                            best.insert(key, (c.0, n));
                        }
                    }
                    for (n, c) in choice.iter_mut().enumerate() {
                        if best[&(faces[probes[n].0].photo, c.1)].1 != n {
                            c.0 = f32::NEG_INFINITY;
                        }
                    }
                }
                for (n, c) in choice.iter().enumerate() {
                    let right = probes[n].1 == Some(enrolled[c.1].0);
                    picks[mi].push((c.0, right));
                    if mi == 0 && c.0 >= 0.45 && c.0 - c.2 >= 0.08 {
                        cur_right += right as usize;
                        cur_wrong += !right as usize;
                    }
                }
            }
        }
        let mut line = format!("    k={k:<2}");
        for (mi, m) in methods.iter().enumerate() {
            // 점수가 높은 것부터 넣어 가며, 틀린 비율이 1%·3%를 넘지 않는 가장 큰 "맞게 넣은 비율"
            let mut v = picks[mi].clone();
            v.sort_by(|a, b| b.0.total_cmp(&a.0));
            let (mut right, mut wrong) = (0usize, 0usize);
            let (mut at1, mut at3) = (0usize, 0usize);
            for (s, ok) in &v {
                if *s == f32::NEG_INFINITY {
                    break;
                }
                if *ok { right += 1 } else { wrong += 1 }
                let rate = wrong as f64 / (right + wrong) as f64;
                if rate <= 0.01 {
                    at1 = right;
                }
                if rate <= 0.03 {
                    at3 = right;
                }
            }
            line += &format!(" | {m}: {:.1}% / {:.1}%", 100.0 * at1 as f64 / probes_known.max(1) as f64, 100.0 * at3 as f64 / probes_known.max(1) as f64);
        }
        println!("{line}   (틀린 비율 1% / 3% 안에서 맞게 넣은 비율)");
        println!(
            "         지금 기준 그대로(0.45, 차이 0.08; SFace 눈금): 맞게 {:.1}% · 넣은 것 중 틀림 {:.1}%",
            100.0 * cur_right as f64 / probes_known.max(1) as f64,
            100.0 * cur_wrong as f64 / (cur_right + cur_wrong).max(1) as f64
        );
    }
}

// ───────────────────────── 공개 사진 (LFW) ─────────────────────────

fn lfw_image(dir: &Path, name: &str, n: &str) -> (String, PathBuf) {
    let file = format!("{name}_{:04}.jpg", n.parse::<u32>().unwrap_or(0));
    (file.clone(), dir.join(name).join(file))
}

/// pairs.txt → 묶음(fold)마다 (사진1, 사진2, 같은 사람?)
fn lfw_pairs(pairs: &Path) -> anyhow::Result<Vec<Vec<(String, String, bool)>>> {
    let text = std::fs::read_to_string(pairs)?;
    let mut lines = text.lines();
    let head: Vec<usize> = lines.next().context("빈 파일")?.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    let (folds, per) = (head[0], head[1]);
    let mut out = Vec::new();
    for _ in 0..folds {
        let mut fold = Vec::new();
        for _ in 0..per * 2 {
            let c: Vec<&str> = lines.next().context("줄이 모자람")?.split('\t').collect();
            let d = Path::new("");
            match c.len() {
                3 => fold.push((lfw_image(d, c[0], c[1]).0, lfw_image(d, c[0], c[2]).0, true)),
                4 => fold.push((lfw_image(d, c[0], c[1]).0, lfw_image(d, c[2], c[3]).0, false)),
                _ => anyhow::bail!("pairs.txt 형식"),
            }
        }
        out.push(fold);
    }
    Ok(out)
}

fn lfw(models_dir: &Path, dir: &Path, pairs: &Path, lab: &Path, rest: &[String]) -> anyhow::Result<()> {
    let mut folds = 10usize;
    let mut sizes: Vec<u32> = vec![0, 36, 24];
    let mut specs: Vec<String> = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--folds" => folds = it.next().context("--folds N")?.parse()?,
            "--sizes" => sizes = it.next().context("--sizes 0,36,24")?.split(',').filter_map(|v| v.parse().ok()).collect(),
            _ => specs.push(a.clone()),
        }
    }
    let det = Models::load(models_dir)?;
    let embs: Vec<Embedder> = specs.iter().map(|s| Embedder::load(s)).collect::<anyhow::Result<_>>()?;
    let all = lfw_pairs(pairs)?;
    let mut images: Vec<String> = all.iter().take(folds).flatten().flat_map(|p| [p.0.clone(), p.1.clone()]).collect();
    images.sort();
    images.dedup();
    let lab = Mutex::new(open_lab(lab)?);
    let done: HashSet<(String, String, u32)> = {
        let conn = lab.lock().unwrap();
        let mut st = conn.prepare("SELECT model, image, size FROM lfw")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.flatten().collect()
    };
    let t0 = Instant::now();
    let count = std::sync::atomic::AtomicUsize::new(0);
    let missed: usize = images
        .par_iter()
        .map(|image| {
            let n = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n % 500 == 0 {
                eprintln!("  사진 {n}/{} · {:.0}초", images.len(), t0.elapsed().as_secs_f64());
            }
            if embs.iter().all(|e| sizes.iter().all(|s| done.contains(&(e.name.clone(), image.clone(), *s)))) {
                return 0;
            }
            let person = image.rsplit_once('_').map_or("", |p| p.0);
            let Ok(px) = decode::screen_rgba(&dir.join(person).join(image), 250) else { return 1 };
            let (w, h) = (px.width as f32, px.height as f32);
            // 사진 가운데에 가장 가까운 얼굴 (LFW는 주인공이 가운데에 있다)
            let found = det.detect(&px).unwrap_or_default();
            let center = |f: &Face| ((f.rect[0] + f.rect[2] / 2.0 - 0.5).powi(2) + (f.rect[1] + f.rect[3] / 2.0 - 0.5).powi(2)).sqrt();
            let face = found.iter().min_by(|a, b| center(a).total_cmp(&center(b))).filter(|f| center(f) < 0.25);
            // 못 찾으면 가운데에 표준 자리로 둔다
            let (marks, side, ok) = match face {
                Some(f) => (f.marks, (f.rect[2] * w).min(f.rect[3] * h), 1),
                None => (TEMPLATE.map(|[x, y]| [w / 2.0 + (x - 56.0) * 1.0, h / 2.0 + (y - 56.0) * 1.0]), 100.0, 0),
            };
            let mut rows: Vec<(usize, u32, Vec<f32>)> = Vec::new();
            for &size in &sizes {
                // 얼굴이 size 픽셀이 되도록 사진을 줄인다 (작게 찍힌 얼굴 흉내). 0은 그대로.
                let f = if size == 0 { 1.0 } else { (side / size as f32).max(1.0) };
                let small;
                let (img, m) = if f > 1.0 {
                    let (nw, nh) = (((w / f).round() as u32).max(8), ((h / f).round() as u32).max(8));
                    let Some(src) = image::RgbaImage::from_raw(px.width, px.height, px.rgba.clone()) else { return 1 };
                    let r = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Triangle);
                    small = Pixels { rgba: r.into_raw(), width: nw, height: nh };
                    (&small, marks.map(|[x, y]| [x * nw as f32 / w, y * nh as f32 / h]))
                } else {
                    (&px, marks)
                };
                let aligned = align(img, &m);
                for (k, e) in embs.iter().enumerate() {
                    if done.contains(&(e.name.clone(), image.clone(), size)) {
                        continue;
                    }
                    if let Ok((v, _)) = e.embed(&aligned, true) {
                        rows.push((k, size, v));
                    }
                }
            }
            let conn = lab.lock().unwrap();
            let _ = conn.execute("INSERT OR REPLACE INTO lfw_meta (image, px, found) VALUES (?1, ?2, ?3)", rusqlite::params![image, side, ok]);
            for (k, size, v) in rows {
                let _ = conn.execute("INSERT OR REPLACE INTO lfw (model, image, size, emb) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![embs[k].name, image, size, blob(&v)]);
            }
            0
        })
        .sum();
    println!("사진 {}장 · 읽기 실패 {missed} · {:.0}초", images.len(), t0.elapsed().as_secs_f64());
    Ok(())
}

fn lfw_eval(pairs: &Path, lab: &Path) -> anyhow::Result<()> {
    let all = lfw_pairs(pairs)?;
    let conn = open_lab(lab)?;
    let (n, found, px): (i64, i64, f64) = conn.query_row("SELECT COUNT(*), COALESCE(SUM(found), 0), COALESCE(AVG(px), 0) FROM lfw_meta", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    println!("사진 {n}장 · 얼굴을 찾음 {found}장 ({:.2}%) · 원래 얼굴 크기 평균 {px:.0}px\n", 100.0 * found as f64 / n.max(1) as f64);
    let keys: Vec<(String, u32)> = conn.prepare("SELECT DISTINCT model, size FROM lfw ORDER BY size = 0 DESC, size DESC, model")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.flatten().collect();
    println!("| 얼굴 크기 | 모델 | 쌍 | 정확도 (10묶음 교차) | 다른 사람 1%만 통과시킬 때 맞힘 | 0.1%만 통과시킬 때 맞힘 | 같은 사람 중간 | 다른 사람 99% |");
    println!("|---|---|---|---|---|---|---|---|");
    for (model, size) in keys {
        let mut st = conn.prepare("SELECT image, emb FROM lfw WHERE model = ?1 AND size = ?2")?;
        let embs: HashMap<String, Vec<f32>> = st.query_map(rusqlite::params![model, size], |r| Ok((r.get(0)?, unblob(&r.get::<_, Vec<u8>>(1)?))))?.flatten().collect();
        // 지문이 다 있는 묶음만
        let folds: Vec<Vec<(f32, bool)>> = all
            .iter()
            .filter(|f| f.iter().all(|p| embs.contains_key(&p.0) && embs.contains_key(&p.1)))
            .map(|f| f.iter().map(|p| (similarity_of(&embs[&p.0], &embs[&p.1]), p.2)).collect())
            .collect();
        if folds.is_empty() {
            continue;
        }
        let acc_at = |v: &[&(f32, bool)], t: f32| v.iter().filter(|p| (p.0 >= t) == p.1).count() as f64 / v.len().max(1) as f64;
        let mut accs = Vec::new();
        for k in 0..folds.len() {
            let train: Vec<&(f32, bool)> = folds.iter().enumerate().filter(|(i, _)| *i != k || folds.len() == 1).flat_map(|(_, f)| f.iter()).collect();
            let test: Vec<&(f32, bool)> = folds[k].iter().collect();
            let best = (0..400).map(|i| -0.2 + i as f32 * 0.0025).max_by(|a, b| acc_at(&train, *a).total_cmp(&acc_at(&train, *b))).unwrap_or(0.3);
            accs.push(acc_at(&test, best));
        }
        let mean = accs.iter().sum::<f64>() / accs.len() as f64;
        let sd = (accs.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / accs.len() as f64).sqrt();
        let mut pos: Vec<f32> = folds.iter().flatten().filter(|p| p.1).map(|p| p.0).collect();
        let mut neg: Vec<f32> = folds.iter().flatten().filter(|p| !p.1).map(|p| p.0).collect();
        pos.sort_by(f32::total_cmp);
        neg.sort_by(f32::total_cmp);
        println!(
            "| {} | {model} | {} | **{:.2}%** ±{:.2} | {:.1}% | {:.1}% | {:.2} | {:.2} |",
            if size == 0 { "원래 (약 100px)".to_string() } else { format!("{size}px로 줄임") },
            pos.len() + neg.len(),
            100.0 * mean,
            100.0 * sd,
            100.0 * rate_ge(&pos, thr_at_far(&neg, 0.01)),
            100.0 * rate_ge(&pos, thr_at_far(&neg, 0.001)),
            quant(&pos, 0.5),
            quant(&neg, 0.99)
        );
    }
    Ok(())
}
