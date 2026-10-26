//! iSee 속도 측정 도구.
//!   isee-bench gen <폴더>               테스트 사진 세트 만들기 (맥)
//!   isee-bench run <폴더> [결과.md]      측정하고 결과를 표로 출력
//!   isee-bench gen-similar <폴더>       비슷한 사진 정답 세트 만들기 (맥)
//!   isee-bench similar <폴더>           특징값 계산 속도·묶기 정확도 측정
//!   isee-bench gen-organize <폴더>      날짜·장소 정리용 사진 세트 (EXIF 날짜·GPS, 짝 파일, 동영상)
//!   isee-bench organize <폴더> [큰 폴더]  날짜·위치 읽기 속도, 규칙별 폴더, 정리·되돌리기 시간

use isee_core::cache::ThumbCache;
use isee_core::decode::{self, generic};
use isee_core::thumbs::{Req, ThumbService};
use isee_core::{THUMB_PX, listing, viewer};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const SCREEN_PX: u32 = 2560;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("gen") => generate(Path::new(args.get(2).expect("폴더 경로"))),
        #[cfg(target_os = "macos")]
        Some("probe") => {
            // 한 파일의 화면 크기 처리 단계를 나눠서 측정: 디코딩만 / 디코딩+JPEG 인코딩 / 디코딩+RGBA 변환
            for a in &args[2..] {
                let p = Path::new(a);
                let (d, _) = median_ms(5, || decode::imageio::decode_only(p, SCREEN_PX).ok());
                let (e, _) = median_ms(5, || decode::imageio::screen(p, SCREEN_PX).ok());
                let (r, _) = median_ms(5, || decode::imageio::rgba(p, SCREEN_PX).ok());
                let (g, _) = median_ms(3, || generic::screen_rgba(p, SCREEN_PX).ok());
                let (v, _) = median_ms(5, || decode::screen_rgba(p, SCREEN_PX).ok());
                println!("{a}\n  ImageIO 디코딩 {d:.1}ms | +JPEG {e:.1}ms | +RGBA {r:.1}ms | Rust RGBA {g:.1}ms | 실제 경로 {v:.1}ms");
                // 실제 창 크기 (14인치 맥북, 레티나) 상자에 맞춘 뷰어 준비
                for (bw, bh) in [(2816u32, 1792u32), (3840, 2176)] {
                    let (t, r) = median_ms(5, || viewer::prepare(p, bw, bh).ok());
                    let px = r.and_then(|v| match &v.payload { viewer::Payload::Pixels(p) => Some((p.width, p.height)), _ => None });
                    println!("  뷰어 준비 상자 {bw}×{bh}: {t:.1}ms → {px:?}");
                }
            }
            Ok(())
        }
        Some("gen-similar") => gen_similar(Path::new(args.get(2).expect("폴더 경로"))),
        Some("gen-organize") => gen_organize(Path::new(args.get(2).expect("폴더 경로"))),
        Some("organize") => organize_bench(Path::new(args.get(2).expect("폴더 경로")), args.get(3).map(PathBuf::from)),
        Some("similar") => similar_bench(Path::new(args.get(2).expect("폴더 경로"))),
        Some("thumb") => {
            // 파일별 썸네일 시간: isee-bench thumb 파일…
            for a in &args[2..] {
                let p = Path::new(a);
                let t = Instant::now();
                let r = decode::thumbnail(p, THUMB_PX);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let t2 = Instant::now();
                let _ = decode::thumbnail(p, THUMB_PX);
                let ms2 = t2.elapsed().as_secs_f64() * 1000.0;
                match r {
                    Ok(e) => println!("{:<28} {ms:>6.1}ms (두 번째 {ms2:.1}ms) {}×{} {}KB {}", p.file_name().unwrap().to_string_lossy(), e.width, e.height, e.bytes.len() / 1000, e.mime),
                    Err(e) => println!("{:<28} {ms:>6.1}ms 실패: {e}", p.file_name().unwrap().to_string_lossy()),
                }
            }
            Ok(())
        }
        Some("info") => {
            for a in &args[2..] {
                let t = Instant::now();
                let i = isee_core::info::file_info(Path::new(a))?;
                println!("{:.1}ms 클라우드 {} 크기 {:?}×{:?} 촬영 {:?}", t.elapsed().as_secs_f64() * 1000.0, i.cloud, i.width, i.height, i.taken);
            }
            Ok(())
        }
        Some("faces") => faces_bench(Path::new(args.get(2).expect("모델 폴더")), Path::new(args.get(3).expect("사진 폴더"))),
        Some("place") => {
            // 좌표 → 지명 확인: isee-bench place 37.56,126.97 33.25,126.41 …
            use isee_core::places::{Level, lookup};
            for a in &args[2..] {
                let v: Vec<f64> = a.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                let p = lookup(v[0], v[1]);
                let show = |l| p.as_ref().map(|p| p.label(l)).unwrap_or_else(|| "?".into());
                println!("{a:<24} {} | {} | {} | {}", show(Level::Country), show(Level::Province), show(Level::City), show(Level::Town));
            }
            Ok(())
        }
        Some("convert") => convert_bench(Path::new(args.get(2).expect("폴더 경로"))),
        Some("run") => run(
            Path::new(args.get(2).expect("폴더 경로")),
            args.get(3).map(PathBuf::from),
        ),
        _ => {
            eprintln!("사용법: isee-bench gen <폴더> | run <폴더> [결과.md]");
            Ok(())
        }
    }
}

// ───────────────────────── 테스트 사진 만들기 ─────────────────────────

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393)
        ^ (y as u32).wrapping_mul(668_265_263)
        ^ seed.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0
}

fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = hash(xi, yi, seed);
    let b = hash(xi + 1, yi, seed);
    let c = hash(xi, yi + 1, seed);
    let d = hash(xi + 1, yi + 1, seed);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += noise(x * freq, y * freq, seed + o) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

/// 사진과 비슷한 압축률이 나오도록 하늘·지형·질감·센서 노이즈를 섞은 RGBX.
fn synth_photo(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut buf = vec![255u8; w as usize * h as usize * 4];
    let pal = |s: u32, k: u32| 40.0 + hash(s as i32, k as i32, 99) * 200.0;
    let sky = [pal(seed, 1), pal(seed, 2), pal(seed, 3)];
    let ground = [pal(seed, 4), pal(seed, 5), pal(seed, 6)];
    let detail = [pal(seed, 7), pal(seed, 8), pal(seed, 9)];
    let scale = w.max(h) as f32;
    let threads = std::thread::available_parallelism().map_or(8, |n| n.get());
    let rows_per = (h as usize).div_ceil(threads);
    std::thread::scope(|s| {
        for (ci, chunk) in buf.chunks_mut(rows_per * w as usize * 4).enumerate() {
            s.spawn(move || {
                for (ri, row) in chunk.chunks_mut(w as usize * 4).enumerate() {
                    let y = (ci * rows_per + ri) as f32;
                    for x in 0..w as usize {
                        let (u, v) = (x as f32 / scale, y / scale);
                        let land = fbm(u * 6.0, v * 6.0, seed, 4);
                        let horizon = (v * 1.6 - 0.35 + (land - 0.5) * 0.6).clamp(0.0, 1.0);
                        let tex = fbm(u * 180.0, v * 180.0, seed + 50, 3);
                        let grain = hash(x as i32, y as i32, seed + 7) * 14.0 - 7.0;
                        let px = &mut row[x * 4..x * 4 + 4];
                        for c in 0..3 {
                            let base = sky[c] * (1.0 - horizon) * (1.0 - v * 0.3)
                                + ground[c] * horizon;
                            let val = base * (0.75 + land * 0.5)
                                + (tex - 0.5) * detail[c] * 0.35 * horizon
                                + grain;
                            px[c] = val.clamp(0.0, 255.0) as u8;
                        }
                    }
                }
            });
        }
    });
    buf
}

struct Spec {
    name: &'static str,
    w: u32,
    h: u32,
    uti: &'static str,
    ext: &'static str,
    quality: f64,
    thumb: bool,
    orient: u32,
}

const SINGLES: &[Spec] = &[
    Spec { name: "jpeg_12mp_phone", w: 4032, h: 3024, uti: "public.jpeg", ext: "jpg", quality: 0.85, thumb: true, orient: 1 },
    Spec { name: "jpeg_24mp_camera", w: 6000, h: 4000, uti: "public.jpeg", ext: "jpg", quality: 0.92, thumb: true, orient: 1 },
    Spec { name: "jpeg_24mp_nothumb", w: 6000, h: 4000, uti: "public.jpeg", ext: "jpg", quality: 0.92, thumb: false, orient: 1 },
    Spec { name: "jpeg_24mp_rot90", w: 6000, h: 4000, uti: "public.jpeg", ext: "jpg", quality: 0.92, thumb: true, orient: 6 },
    Spec { name: "jpeg_48mp", w: 8064, h: 6048, uti: "public.jpeg", ext: "jpg", quality: 0.9, thumb: true, orient: 1 },
    Spec { name: "jpeg_96mp", w: 12000, h: 8000, uti: "public.jpeg", ext: "jpg", quality: 0.9, thumb: true, orient: 1 },
    Spec { name: "heic_12mp_phone", w: 4032, h: 3024, uti: "public.heic", ext: "heic", quality: 0.8, thumb: true, orient: 1 },
    Spec { name: "heic_48mp", w: 8064, h: 6048, uti: "public.heic", ext: "heic", quality: 0.8, thumb: true, orient: 1 },
    Spec { name: "png_24mp", w: 6000, h: 4000, uti: "public.png", ext: "png", quality: 1.0, thumb: false, orient: 1 },
    Spec { name: "tiff_24mp", w: 6000, h: 4000, uti: "public.tiff", ext: "tif", quality: 1.0, thumb: false, orient: 1 },
];

#[cfg(target_os = "macos")]
fn write(spec: &Spec, seed: u32, path: &Path) -> anyhow::Result<()> {
    let mut px = synth_photo(spec.w, spec.h, seed);
    decode::imageio::write_rgbx(
        path, &mut px, spec.w, spec.h, spec.uti, spec.quality, spec.thumb, spec.orient,
    )
}

#[cfg(not(target_os = "macos"))]
fn write(_spec: &Spec, _seed: u32, _path: &Path) -> anyhow::Result<()> {
    anyhow::bail!("테스트 세트 생성은 지금은 맥에서만 지원")
}

fn generate(dir: &Path) -> anyhow::Result<()> {
    let singles = dir.join("singles");
    let folder = dir.join("folder");
    std::fs::create_dir_all(&singles)?;
    std::fs::create_dir_all(&folder)?;
    for (i, spec) in SINGLES.iter().enumerate() {
        let p = singles.join(format!("{}.{}", spec.name, spec.ext));
        if p.exists() {
            continue;
        }
        let t = Instant::now();
        write(spec, 1000 + i as u32, &p)?;
        println!("{:<24} {:>6.1} MB  ({:.1}s)", spec.name, mb(&p), t.elapsed().as_secs_f64());
    }
    // PSD는 ImageIO가 쓰지 못하므로 sips로 변환한다.
    let psd = singles.join("psd_24mp.psd");
    if cfg!(target_os = "macos") && !psd.exists() {
        let status = std::process::Command::new("sips")
            .args(["-s", "format", "psd"])
            .arg(singles.join("png_24mp.png"))
            .arg("--out")
            .arg(&psd)
            .stdout(std::process::Stdio::null())
            .status()?;
        if status.success() {
            println!("{:<24} {:>6.1} MB", "psd_24mp", mb(&psd));
        }
    }

    // 폴더 측정용: 휴대폰 사진 12MP 300장 + 카메라 24MP 60장 + HEIC 40장.
    // 서로 다른 원본 몇 장을 만들고 복제한다 (APFS에서는 복제가 디스크 공간을 거의 쓰지 않음).
    let mut n = 0;
    let plan: [(&Spec, usize, usize); 3] = [(&SINGLES[0], 6, 300), (&SINGLES[1], 2, 60), (&SINGLES[6], 2, 40)];
    for (spec, variants, count) in plan {
        let bases: Vec<PathBuf> = (0..variants)
            .map(|v| {
                let p = dir.join(format!("base_{}_{v}.{}", spec.name, spec.ext));
                if !p.exists() {
                    write(spec, 5000 + v as u32 * 31 + spec.w, &p).unwrap();
                }
                p
            })
            .collect();
        for i in 0..count {
            n += 1;
            let p = folder.join(format!("IMG_{n:04}.{}", spec.ext));
            if !p.exists() {
                std::fs::copy(&bases[i % variants], &p)?;
            }
        }
    }
    println!("folder: {n}장");
    Ok(())
}

// ───────────────────────── 비슷한 사진 정답 세트 ─────────────────────────

/// 가로 세로 상자 흐림 (반지름 r), 여러 스레드
fn box_blur(px: &mut [u8], w: usize, h: usize, r: usize) {
    let pass = |src: &[u8], dst: &mut [u8], horizontal: bool| {
        let (lines, len) = if horizontal { (h, w) } else { (w, h) };
        let at = |line: usize, i: usize| if horizontal { (line * w + i) * 4 } else { (i * w + line) * 4 };
        // 세로 패스는 줄마다 메모리가 흩어져 있어 스레드로 나누기 번거로우니 한 번에 처리
        for line in 0..lines {
            for c in 0..3 {
                let mut acc = 0u32;
                let get = |i: isize| src[at(line, i.clamp(0, len as isize - 1) as usize) + c] as u32;
                for i in -(r as isize)..=(r as isize) {
                    acc += get(i);
                }
                for i in 0..len {
                    dst[at(line, i) + c] = (acc / (2 * r as u32 + 1)) as u8;
                    acc += get(i as isize + r as isize + 1);
                    acc -= get(i as isize - r as isize);
                }
            }
        }
    };
    let mut tmp = px.to_vec();
    pass(px, &mut tmp, true);
    pass(&tmp, px, false);
}

fn variant(base: &[u8], w: usize, h: usize, kind: char, seed: u32) -> Vec<u8> {
    let mut v = base.to_vec();
    match kind {
        // 살짝 옮겨 찍고 아주 약간 흔들림
        'b' => {
            let dx = w * 3 / 100;
            for y in 0..h {
                for x in 0..w {
                    let sx = (x + dx).min(w - 1);
                    v[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&base[(y * w + sx) * 4..(y * w + sx) * 4 + 3]);
                }
            }
            box_blur(&mut v, w, h, 1);
        }
        'c' => box_blur(&mut v, w, h, 5),
        'd' => v.iter_mut().for_each(|p| *p = (*p as f32 * 2.2 + 30.0).min(255.0) as u8),
        'e' => v.iter_mut().for_each(|p| *p = (*p as f32 * 0.28) as u8),
        'f' => {
            for (i, p) in v.iter_mut().enumerate() {
                let n = hash(i as i32, (i >> 16) as i32, seed) * 70.0 - 35.0;
                *p = (*p as f32 + n).clamp(0.0, 255.0) as u8;
            }
        }
        _ => {}
    }
    v
}

/// 경계가 뚜렷한 물체(사각형·원·가는 막대)를 얹는다: 실제 사진처럼 초점이 맞으면 또렷한 경계가 생기게
fn add_objects(px: &mut [u8], w: usize, h: usize, seed: u32) {
    for k in 0..40 {
        let r = |j: i32| hash(k, j, seed + 300);
        let (cx, cy) = ((r(1) * w as f32) as isize, (r(2) * h as f32) as isize);
        let size = (20.0 + r(3) * w as f32 * 0.08) as isize;
        let color = [r(4) * 255.0, r(5) * 255.0, r(6) * 255.0];
        let shape = (r(7) * 3.0) as u32;
        let (hw, hh) = match shape {
            2 => (size * 3, (size / 10).max(3)), // 가는 막대
            _ => (size, size),
        };
        for y in (cy - hh).max(0)..(cy + hh).min(h as isize) {
            for x in (cx - hw).max(0)..(cx + hw).min(w as isize) {
                let inside = shape != 1 || ((x - cx).pow(2) + (y - cy).pow(2)) < size * size;
                if inside {
                    let i = (y as usize * w + x as usize) * 4;
                    for c in 0..3 {
                        px[i + c] = (color[c] * 0.85 + px[i + c] as f32 * 0.15) as u8;
                    }
                }
            }
        }
    }
}

/// 장면 24개. 장면마다 원본(a)과 변형(b 옮김, c 흐림, d 과노출, e 저노출, f 노이즈, g 같은 파일)을 섞어서 만든다.
/// 5번째 장면마다는 한 장뿐이다 (다른 사진과 묶이면 안 됨). 정답은 truth.tsv.
fn gen_similar(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let (w, h) = (4032usize, 3024usize);
    let start = std::time::SystemTime::now() - Duration::from_secs(30 * 86_400);
    let mut truth = String::from("file\tscene\tvariant\n");
    let mut n = 0;
    let t0 = Instant::now();
    for scene in 0..24u32 {
        let seed = 7000 + scene * 13;
        let mut base = synth_photo(w as u32, h as u32, seed);
        add_objects(&mut base, w, h, seed);
        let kinds: Vec<char> = if scene % 5 == 4 {
            vec!['a']
        } else {
            // 장면마다 변형 조합과 순서를 바꾼다 (원본이 늘 첫 장이 아니게)
            let all = ['a', 'b', 'c', 'd', 'e', 'f', 'g'];
            let mut k: Vec<char> = all.iter().copied().filter(|&c| c == 'a' || hash(scene as i32, c as i32, 5) > 0.35).collect();
            k.sort_by(|x, y| hash(scene as i32, *x as i32, 9).total_cmp(&hash(scene as i32, *y as i32, 9)));
            k
        };
        let mut base_path = None;
        for (i, &k) in kinds.iter().enumerate() {
            if k == 'g' {
                continue; // 같은 파일은 원본을 만든 뒤 복사
            }
            n += 1;
            let p = dir.join(format!("IMG_{n:04}.jpg"));
            if !p.exists() {
                let mut px = variant(&base, w, h, k, seed);
                decode::imageio::write_rgbx(&p, &mut px, w as u32, h as u32, "public.jpeg", 0.85, true, 1)?;
            }
            set_time(&p, start + Duration::from_secs(scene as u64 * 1800 + i as u64 * 2))?;
            truth += &format!("IMG_{n:04}.jpg\t{scene}\t{k}\n");
            if k == 'a' {
                base_path = Some(p);
            }
        }
        if kinds.contains(&'g')
            && let Some(bp) = &base_path
        {
            let p = dir.join(format!("{} 사본.jpg", bp.file_stem().unwrap().to_string_lossy()));
            std::fs::copy(bp, &p)?;
            set_time(&p, start + Duration::from_secs(scene as u64 * 1800 + 20))?;
            truth += &format!("{}\t{scene}\tg\n", p.file_name().unwrap().to_string_lossy());
        }
        print!(".");
        use std::io::Write as _;
        std::io::stdout().flush().ok();
    }
    std::fs::write(dir.join("truth.tsv"), truth)?;
    println!("\n{n}장 + 사본 ({:.1}s)", t0.elapsed().as_secs_f64());
    Ok(())
}

fn set_time(p: &Path, t: std::time::SystemTime) -> anyhow::Result<()> {
    std::fs::File::options().write(true).open(p)?.set_modified(t)?;
    Ok(())
}

fn similar_bench(dir: &Path) -> anyhow::Result<()> {
    use isee_core::analysis::{self, AnalysisService, FeatureStore};
    use isee_core::similar::{self, Options};
    use std::collections::HashMap;
    let truth: HashMap<String, (u32, char)> = std::fs::read_to_string(dir.join("truth.tsv"))?
        .lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            Some((c[0].to_string(), (c[1].parse().ok()?, c[2].chars().next()?)))
        })
        .collect();
    let entries = listing::list_dir(dir, false)?;
    let reqs: Vec<Req> = entries
        .iter()
        .filter(|e| e.kind == isee_core::formats::Kind::Image)
        .enumerate()
        .map(|(i, e)| Req { path: e.path.clone(), size: e.size, mtime: e.mtime, index: i as i64 })
        .collect();
    let n = reqs.len();
    let cores = std::thread::available_parallelism().map_or(8, |n| n.get());
    let workers = (cores / 2).clamp(1, 6);

    // 1) 한 장 특징값 계산 시간
    let one = Path::new(&reqs[0].path);
    let (ms_one, _) = median_ms(5, || analysis::compute(one, 0).ok());
    println!("한 장 특징값 (12MP JPEG): {ms_one:.1}ms");

    // 2) 폴더 전체 백그라운드 분석 (앱과 같은 스레드 수)
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let svc = AnalysisService::new(FeatureStore::open(None)?, paused.clone(), workers);
    let t = Instant::now();
    svc.analyze(reqs.clone());
    wait_until(|| svc.progress().0 >= n);
    let all_s = t.elapsed().as_secs_f64();
    println!("폴더 {n}장 분석: {all_s:.2}초 ({:.0}장/초, 스레드 {workers}개)", n as f64 / all_s);

    // 3) 분석 중 썸네일 첫 화면이 얼마나 느려지나
    let thumb_workers = cores.saturating_sub(2).max(2);
    let alone = {
        let t = ThumbService::new(ThumbCache::open(None, 20_000)?, THUMB_PX, thumb_workers);
        first_screen(&t, &reqs, 48)
    };
    let busy = {
        let svc2 = AnalysisService::new(FeatureStore::open(None)?, paused.clone(), workers);
        svc2.analyze(reqs.clone());
        std::thread::sleep(Duration::from_millis(150));
        let t = ThumbService::new(ThumbCache::open(None, 20_000)?, THUMB_PX, thumb_workers);
        let r = first_screen(&t, &reqs, 48);
        svc2.analyze(Vec::new());
        r
    };
    println!("썸네일 첫 화면 48칸: 혼자 {alone:.0}ms / 분석 중 {busy:.0}ms");

    // 4) 단계별 묶기 정확도와 베스트 추천 정답률
    let feats = svc.features(&reqs);
    let items: Vec<similar::Item> = reqs
        .iter()
        .zip(feats)
        .filter_map(|(r, f)| {
            Some(similar::Item { path: r.path.clone(), size: r.size, mtime: r.mtime, flag: 0, rating: 0, f: f? })
        })
        .collect();
    let name = |p: &str| Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
    // 장면 몇 개의 변형별 특징값 (기준을 고칠 때 보는 표)
    println!("\n| 장면 | 변형 | 선명 | 노이즈 | 밝기 | 날아감 | 묻힘 | 원본과 지문 차이 |");
    let base_hash = |scene: u32| {
        items.iter().find(|it| truth.get(&name(&it.path)) == Some(&(scene, 'a'))).map(|it| it.f.phash)
    };
    let mut by_variant: HashMap<char, Vec<u32>> = HashMap::new();
    for it in &items {
        if let Some(&(sc, v)) = truth.get(&name(&it.path)) {
            let f = &it.f;
            let d = base_hash(sc).map(|b| (b ^ f.phash).count_ones()).unwrap_or(0);
            by_variant.entry(v).or_default().push(d);
            if sc < 3 {
                println!("| {sc} | {v} | {:.1} | {:.2} | {:.2} | {:.3} | {:.3} | {d} |", f.sharp, f.noise, f.mean, f.clip_hi, f.clip_lo);
            }
        }
    }
    let mut vs: Vec<_> = by_variant.into_iter().collect();
    vs.sort();
    for (v, mut ds) in vs {
        ds.sort();
        println!("변형 {v}: 원본과 지문 차이 {ds:?}");
    }
    // 서로 다른 장면 원본끼리의 가장 가까운 차이
    let bases: Vec<u64> = (0..24).filter_map(base_hash).collect();
    let mut closest = Vec::new();
    for i in 0..bases.len() {
        for j in i + 1..bases.len() {
            closest.push((bases[i] ^ bases[j]).count_ones());
        }
    }
    closest.sort();
    println!("다른 장면끼리 지문 차이 (작은 것부터): {:?}", &closest[..closest.len().min(12)]);
    let scene_of = |p: &str| truth.get(&name(p)).map(|t| t.0);
    // 같은 장면 쌍
    let mut true_pairs = 0;
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            if scene_of(&items[i].path) == scene_of(&items[j].path) {
                true_pairs += 1;
            }
        }
    }
    println!("\n| 묶는 정도 | 그룹 | 정밀도 | 재현율 | 베스트 정답 | 보고서 계산 |");
    println!("|---|---|---|---|---|---|");
    for level in 1..=5u8 {
        let opt = Options { level, ..Options::default() };
        let r = similar::report(&items, n, &opt);
        let (mut tp, mut fp, mut best_ok, mut best_n) = (0, 0, 0, 0);
        for g in &r.groups {
            let ps: Vec<&str> = g.members.iter().map(|m| m.photo.path.as_str()).collect();
            for i in 0..ps.len() {
                for j in i + 1..ps.len() {
                    if scene_of(ps[i]) == scene_of(ps[j]) { tp += 1 } else { fp += 1 }
                }
            }
            best_n += 1;
            let b = truth.get(&name(ps[0])).map(|t| t.1);
            // 'b'(3% 옮겨 찍고 원본 해상도에서 1픽셀 흐림)는 분석 해상도에서 원본과 같은 품질이라 정답으로 친다
            if matches!(b, Some('a') | Some('b') | Some('g')) {
                best_ok += 1;
            } else {
                println!("  베스트 오답: {} ({:?}) 이유 {:?}", name(ps[0]), b, g.members[0].reasons);
            }
        }
        println!(
            "| {level} | {} | {:.0}% | {:.0}% | {best_ok}/{best_n} | {:.1}ms |",
            r.groups.len(),
            100.0 * tp as f64 / (tp + fp).max(1) as f64,
            100.0 * tp as f64 / true_pairs.max(1) as f64,
            r.ms
        );
    }
    let r = similar::report(&items, n, &Options::default());
    let flagged = |v: &Vec<similar::Photo>| v.iter().map(|p| format!("{}{:?}", name(&p.path), truth.get(&name(&p.path)).map(|t| t.1))).collect::<Vec<_>>().join(" ");
    println!("\n같은 파일 {}세트, 흐림 {}장: {}", r.duplicates.len(), r.blurry.len(), flagged(&r.blurry));
    println!("어두움 {}장: {}", r.dark.len(), flagged(&r.dark));
    println!("밝음 {}장: {}", r.bright.len(), flagged(&r.bright));
    Ok(())
}

// ───────────────────────── 날짜·장소 정리 세트 ─────────────────────────

/// JPEG 맨 앞에 EXIF(촬영 시각, GPS)를 넣는다
fn with_exif(jpeg: &[u8], when: &str, gps: Option<(f64, f64)>) -> Vec<u8> {
    use exif::experimental::Writer;
    use exif::{Field, In, Rational, Tag, Value};
    let ascii = |s: &str| Value::Ascii(vec![s.as_bytes().to_vec()]);
    let dms = |v: f64| {
        let v = v.abs();
        let d = v.floor();
        let m = ((v - d) * 60.0).floor();
        let s = ((v - d) * 60.0 - m) * 60.0;
        Value::Rational(vec![
            Rational { num: d as u32, denom: 1 },
            Rational { num: m as u32, denom: 1 },
            Rational { num: (s * 1000.0).round() as u32, denom: 1000 },
        ])
    };
    let mut fields = vec![Field { tag: Tag::DateTimeOriginal, ifd_num: In::PRIMARY, value: ascii(when) }];
    if let Some((la, lo)) = gps {
        fields.push(Field { tag: Tag::GPSLatitudeRef, ifd_num: In::PRIMARY, value: ascii(if la < 0.0 { "S" } else { "N" }) });
        fields.push(Field { tag: Tag::GPSLatitude, ifd_num: In::PRIMARY, value: dms(la) });
        fields.push(Field { tag: Tag::GPSLongitudeRef, ifd_num: In::PRIMARY, value: ascii(if lo < 0.0 { "W" } else { "E" }) });
        fields.push(Field { tag: Tag::GPSLongitude, ifd_num: In::PRIMARY, value: dms(lo) });
    }
    let mut w = Writer::new();
    for f in &fields {
        w.push_field(f);
    }
    let mut tiff = std::io::Cursor::new(Vec::new());
    w.write(&mut tiff, false).expect("EXIF 쓰기");
    let tiff = tiff.into_inner();
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    out.extend_from_slice(&jpeg[2..]);
    out
}

fn mp4_box(kind: &[u8], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// 아이폰처럼 촬영 시각·위치가 든 작은 MOV, 또는 mvhd·©xyz만 있는 MP4
fn fake_video(apple: Option<(&str, &str)>, mvhd_unix: Option<i64>, xyz: Option<&str>) -> Vec<u8> {
    let mut moov = Vec::new();
    if let Some(t) = mvhd_unix {
        let mut mvhd = vec![0u8; 4];
        mvhd.extend_from_slice(&((t + 2_082_844_800) as u32).to_be_bytes());
        mvhd.extend_from_slice(&[0; 92]);
        moov.extend(mp4_box(b"mvhd", &mvhd));
    }
    if let Some((when, loc)) = apple {
        let key = |s: &str| mp4_box(b"mdta", s.as_bytes());
        let mut keys = vec![0u8; 4];
        keys.extend_from_slice(&2u32.to_be_bytes());
        keys.extend(key("com.apple.quicktime.location.ISO6709"));
        keys.extend(key("com.apple.quicktime.creationdate"));
        let data = |s: &str| mp4_box(b"data", &[&[0, 0, 0, 1, 0, 0, 0, 0][..], s.as_bytes()].concat());
        let ilst = [mp4_box(&1u32.to_be_bytes(), &data(loc)), mp4_box(&2u32.to_be_bytes(), &data(when))].concat();
        moov.extend(mp4_box(b"meta", &[mp4_box(b"hdlr", &[0; 24]), mp4_box(b"keys", &keys), mp4_box(b"ilst", &ilst)].concat()));
    }
    if let Some(x) = xyz {
        let body = [&[0u8, x.len() as u8, 0x15, 0xc7][..], x.as_bytes()].concat();
        moov.extend(mp4_box(b"udta", &mp4_box(&[0xA9, b'x', b'y', b'z'], &body)));
    }
    [mp4_box(b"ftyp", b"qt  \0\0\0\0qt  "), mp4_box(b"mdat", &[0; 64]), mp4_box(b"moov", &moov)].concat()
}

/// 여행·일상 사진을 흉내 낸 세트 (정답은 이름과 아래 표로 확인)
///   제주 2024-05-12~14 (중문·성산·제주시, 세 장마다 한 장은 GPS 없음), 서울 종로 2024-06-01,
///   오사카 2024-07-20~22 + 교토 당일치기, 카카오톡 받은 사진(이름에만 날짜), 스크린샷 PNG,
///   정보 없는 사진(수정 시각만), 라이브 포토 짝(JPG+MOV+AAE), RAW 짝(JPG+ARW+xmp), 동영상
fn gen_organize(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let (w, h) = (1024u32, 768u32);
    let jpeg_of = |seed: u32| -> anyhow::Result<Vec<u8>> {
        let tmp = std::env::temp_dir().join(format!("isee-org-{seed}-{}.jpg", std::process::id()));
        let mut px = synth_photo(w, h, seed);
        add_objects(&mut px, w as usize, h as usize, seed);
        decode::imageio::write_rgbx(&tmp, &mut px, w, h, "public.jpeg", 0.8, false, 1)?;
        let b = std::fs::read(&tmp)?;
        let _ = std::fs::remove_file(&tmp);
        Ok(b)
    };
    let local = |y: i64, mo: i64, d: i64, hh: i64, mi: i64| isee_core::meta::days_from_civil(y, mo, d) * 86_400 + hh * 3600 + mi * 60;
    let fmt = |t: i64| {
        let (y, m, d) = isee_core::meta::civil(t);
        let r = t.rem_euclid(86_400);
        format!("{y:04}:{m:02}:{d:02} {:02}:{:02}:{:02}", r / 3600, r % 3600 / 60, r % 60)
    };
    let put = |name: &str, bytes: &[u8]| -> anyhow::Result<()> {
        std::fs::write(dir.join(name), bytes)?;
        Ok(())
    };
    let jungmun = (33.2489, 126.4106);
    let seongsan = (33.4581, 126.9425);
    let jejusi = (33.4996, 126.5312);
    let gyeongbok = (37.5796, 126.9770);
    let osaka = (34.6873, 135.5262);
    let kyoto = (35.0116, 135.7681);
    // (시작 시각, 장 수, 간격 분, 장소, 세 장마다 GPS 빼기)
    let trips: [(i64, usize, i64, (f64, f64), bool); 7] = [
        (local(2024, 5, 12, 10, 0), 6, 25, jungmun, true),
        (local(2024, 5, 13, 9, 30), 6, 30, seongsan, true),
        (local(2024, 5, 14, 11, 0), 4, 20, jejusi, true),
        (local(2024, 6, 1, 13, 0), 8, 9, gyeongbok, false),
        (local(2024, 7, 20, 15, 0), 6, 40, osaka, true),
        (local(2024, 7, 21, 10, 0), 5, 30, kyoto, false),
        (local(2024, 7, 22, 10, 0), 4, 30, osaka, false),
    ];
    let mut img = 1000;
    for (ti, &(start, count, step, place, drop_gps)) in trips.iter().enumerate() {
        for k in 0..count {
            img += 1;
            let t = start + k as i64 * step * 60;
            // 세 장마다 한 장은 GPS가 없다 (위치 추정 대상: 앞뒤 사진과 30분 안)
            let gps = (!(drop_gps && k % 3 == 2)).then_some(place);
            let bytes = with_exif(&jpeg_of(9000 + ti as u32 * 31 + k as u32)?, &fmt(t), gps);
            put(&format!("IMG_{img}.JPG"), &bytes)?;
        }
    }
    // 카카오톡으로 받은 사진: EXIF 없음, 이름에만 날짜
    for k in 0..4 {
        put(&format!("KakaoTalk_20240815_10{:02}00123.jpg", k * 5), &jpeg_of(9500 + k)?)?;
    }
    // 스크린샷 (PNG, 이름에 날짜)
    for (k, name) in ["스크린샷 2024-08-01 오후 3.10.22.png", "Screenshot_2024-08-01-21-05-11.png"].iter().enumerate() {
        let src = std::env::temp_dir().join(format!("isee-org-shot-{k}.png"));
        let mut px = synth_photo(640, 480, 9600 + k as u32);
        decode::imageio::write_rgbx(&src, &mut px, 640, 480, "public.png", 1.0, false, 1)?;
        std::fs::rename(&src, dir.join(name))?;
    }
    // 정보가 전혀 없는 사진: 수정 시각(2023-12-25)으로
    for name in ["image.jpg", "다운로드 (1).jpg"] {
        put(name, &jpeg_of(9700 + name.len() as u32)?)?;
        set_time(&dir.join(name), std::time::UNIX_EPOCH + Duration::from_secs(1_703_502_000))?;
    }
    // 라이브 포토 짝: 사진·동영상·편집 정보
    let t = local(2024, 5, 13, 16, 0);
    put("IMG_5001.JPG", &with_exif(&jpeg_of(9800)?, &fmt(t), Some(seongsan)))?;
    put("IMG_5001.MOV", &fake_video(Some(("2024-05-13T16:00:00+0900", "+33.4581+126.9425+010.000/")), None, None))?;
    put("IMG_5001.AAE", b"<plist/>")?;
    // RAW 짝 + 보정 정보
    put("DSC_0001.JPG", &with_exif(&jpeg_of(9810)?, &fmt(local(2024, 6, 1, 14, 30)), Some(gyeongbok)))?;
    put("DSC_0001.ARW", b"RAW")?;
    put("DSC_0001.ARW.xmp", b"<x:xmpmeta/>")?;
    // 혼자 있는 동영상: mvhd(UTC) + ©xyz (교토)
    let utc = local(2024, 7, 21, 6, 0); // 한국·일본 15:00 = UTC 06:00
    put("VID_20240721_150000.mp4", &fake_video(None, Some(utc), Some("+35.0116+135.7681/")))?;
    println!("{}개 파일 ({})", std::fs::read_dir(dir)?.count(), dir.display());
    Ok(())
}

fn organize_bench(dir: &Path, big: Option<PathBuf>) -> anyhow::Result<()> {
    use isee_core::organize::{self, ApplyFolder, ApplyItem, Options};
    use isee_core::places::Level;
    let media = |d: &Path| -> Vec<String> {
        listing::list_dir(d, false)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| matches!(e.kind, isee_core::formats::Kind::Image | isee_core::formats::Kind::Video))
            .map(|e| e.path)
            .collect()
    };
    let t = Instant::now();
    isee_core::places::warm();
    println!("지명 데이터 읽기: {:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
    if let Some(big) = &big {
        let files = media(big);
        let t = Instant::now();
        let items = organize::scan(&files, &|_, _| {});
        let s = t.elapsed().as_secs_f64();
        println!("큰 폴더 {}개 날짜·위치 읽기: {:.0}ms (장당 {:.2}ms)", items.len(), s * 1000.0, s * 1000.0 / items.len().max(1) as f64);
    }
    let files = media(dir);
    let t = Instant::now();
    let items = organize::scan(&files, &|_, _| {});
    println!("정리 세트 {}개 파일 → {}묶음 읽기: {:.1}ms", files.len(), items.len(), t.elapsed().as_secs_f64() * 1000.0);
    let show = |opt: &Options| {
        let p = organize::plan(&items, opt);
        println!(
            "\n규칙 \"{}\" ({:?}{}) — 폴더 {}개, {:.2}ms · 수정시각 {} · 이름 {} · GPS {} · 추정 {} · 위치없음 {} · 짝 {}",
            opt.pattern,
            opt.level,
            if opt.pattern.contains("{기간}") { format!(", 간격 {}시간", opt.event_hours) } else { String::new() },
            p.folders.len(),
            p.ms,
            p.by_mtime,
            p.by_name,
            p.with_gps,
            p.guessed,
            p.no_place,
            p.companions
        );
        for f in &p.folders {
            let names: Vec<String> = f.items.iter().take(4).map(|i| Path::new(&i.path).file_name().unwrap().to_string_lossy().into_owned()).collect();
            println!("  {:<36} {:>3}장  {}{}", f.rel, f.items.len(), names.join(", "), if f.items.len() > 4 { " …" } else { "" });
        }
        p
    };
    show(&Options { pattern: "{년}/{년}-{월}".into(), ..Default::default() });
    show(&Options { pattern: "{년}-{월}-{일} {장소}".into(), level: Level::City, ..Default::default() });
    show(&Options { pattern: "{장소}/{년}-{월}".into(), level: Level::Province, guess_place: false, ..Default::default() });
    let plan = show(&Options { pattern: "{기간} {장소}".into(), level: Level::City, ..Default::default() });
    // 실제로 옮겨 보고 되돌리기 (복제한 폴더에서)
    let work = std::env::temp_dir().join(format!("isee-organize-apply-{}", std::process::id()));
    let status = std::process::Command::new("cp").arg("-c").arg("-R").arg(dir).arg(&work).status()?;
    anyhow::ensure!(status.success(), "복제 실패");
    let rebase = |p: &str| work.join(Path::new(p).strip_prefix(dir).unwrap()).to_string_lossy().into_owned();
    let folders: Vec<ApplyFolder> = plan
        .folders
        .iter()
        .map(|f| ApplyFolder {
            rel: f.rel.clone(),
            items: f.items.iter().map(|i| ApplyItem { path: rebase(&i.path), companions: i.companions.iter().map(|c| rebase(c)).collect() }).collect(),
        })
        .collect();
    let before = std::fs::read_dir(&work)?.count();
    let t = Instant::now();
    let a = organize::apply(&work, &folders, false, &|_, _| {});
    let apply_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    organize::undo(&a)?;
    let undo_ms = t.elapsed().as_secs_f64() * 1000.0;
    let after = std::fs::read_dir(&work)?.count();
    println!(
        "\n정리 실행: 파일 {}개 이동, 폴더 {}개 생성, 실패 {} — {apply_ms:.1}ms · 되돌리기 {undo_ms:.1}ms · 되돌린 뒤 파일 수 {before}→{after}",
        a.moved.len(),
        a.created.len(),
        a.failed.len()
    );
    let _ = std::process::Command::new("rm").arg("-rf").arg(&work).status();
    Ok(())
}

// ───────────────────────── 얼굴 ─────────────────────────

/// 파일 이름 "사람_번호.jpg"를 정답으로 보고, 얼굴 찾기·지문 시간과 같은 사람/다른 사람 비슷함 분포를 잰다
fn faces_bench(models: &Path, dir: &Path) -> anyhow::Result<()> {
    use std::collections::HashMap;
    use isee_core::faces::{Models, similarity_of};
    let t = Instant::now();
    let m = Models::load(models)?;
    println!("모델 읽기: {:.0}ms", t.elapsed().as_secs_f64() * 1000.0);
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| isee_core::formats::kind_of(p) == isee_core::formats::Kind::Image).collect();
    files.sort();
    let (mut det_ms, mut rec_ms, mut dec_ms, mut nfaces) = (0.0, 0.0, 0.0, 0);
    // (사람, 가장 큰 얼굴의 지문)
    let mut prints: Vec<(String, String, Vec<f32>)> = Vec::new();
    for f in &files {
        let name = f.file_stem().unwrap().to_string_lossy().into_owned();
        let who = name.split('_').next().unwrap_or("").to_string();
        let t = Instant::now();
        let px = decode::screen_rgba(f, 1024)?;
        dec_ms += t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let faces = m.detect(&px)?;
        det_ms += t.elapsed().as_secs_f64() * 1000.0;
        nfaces += faces.len();
        let big = faces.iter().max_by(|a, b| (a.rect[2] * a.rect[3]).total_cmp(&(b.rect[2] * b.rect[3])));
        print!("{name:<14} 얼굴 {}", faces.len());
        if let Some(b) = big {
            let t = Instant::now();
            let fp = m.fingerprint(&px, b)?;
            rec_ms += t.elapsed().as_secs_f64() * 1000.0;
            print!("  (가장 큰 얼굴 {:.0}% 점수 {:.2})", b.rect[2] * 100.0, b.score);
            prints.push((who, name, fp));
        }
        println!();
    }
    // 모든 얼굴로 자동 묶기
    {
        use isee_core::people::{FaceRow, group};
        let mut rows = Vec::new();
        let mut id = 0;
        for f in &files {
            for (face, px, emb) in isee_core::people::analyze_one(&m, f)? {
                id += 1;
                rows.push(FaceRow { id, path: f.to_string_lossy().into_owned(), rect: face.rect, px, emb, person: None, not_person: None });
            }
        }
        let r = group(rows, &[], files.len(), files.len());
        println!("\n자동 묶기: 묶음 {}개 · 혼자 {} · 작은 얼굴 {} · {:.1}ms", r.groups.len(), r.singles, r.small, r.ms);
        let (mut pure, mut total) = (0, 0);
        for g in &r.groups {
            let names: Vec<String> = g.faces.iter().map(|f| Path::new(&f.path).file_stem().unwrap().to_string_lossy().split('_').next().unwrap_or("").to_string()).collect();
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for n in &names { *counts.entry(n.as_str()).or_default() += 1; }
            let top = counts.values().max().copied().unwrap_or(0);
            pure += top;
            total += names.len();
            let mut c: Vec<_> = counts.into_iter().collect();
            c.sort_by(|a, b| b.1.cmp(&a.1));
            println!("  얼굴 {:>2}개: {:?}", names.len(), c);
        }
        println!("  묶음 순도 (가장 많은 사람 비율): {:.0}%", 100.0 * pure as f64 / total.max(1) as f64);
    }
    let n = files.len() as f64;
    println!("\n사진 {} 장, 얼굴 {nfaces}개 · 장당 디코딩 {:.1}ms · 얼굴 찾기 {:.1}ms · 지문 {:.1}ms(얼굴당)", files.len(), dec_ms / n, det_ms / n, rec_ms / prints.len().max(1) as f64);
    let (mut same, mut diff) = (Vec::new(), Vec::new());
    for i in 0..prints.len() {
        for j in i + 1..prints.len() {
            let s = similarity_of(&prints[i].2, &prints[j].2);
            if prints[i].0 == prints[j].0 { same.push(s) } else { diff.push(s) }
        }
    }
    let stat = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        let q = |p: f32| v[((v.len() - 1) as f32 * p) as usize];
        format!("{}쌍 최소 {:.2} 10% {:.2} 중간 {:.2} 90% {:.2} 최대 {:.2}", v.len(), q(0.0), q(0.1), q(0.5), q(0.9), q(1.0))
    };
    println!("같은 사람: {}", stat(&mut same));
    println!("다른 사람: {}", stat(&mut diff));
    for th in [0.30f32, 0.363, 0.40, 0.45] {
        let tp = same.iter().filter(|&&s| s >= th).count();
        let fp = diff.iter().filter(|&&s| s >= th).count();
        println!("기준 {th:.3}: 같은 사람 맞힘 {:.0}% · 다른 사람을 같다고 봄 {:.1}%", 100.0 * tp as f64 / same.len().max(1) as f64, 100.0 * fp as f64 / diff.len().max(1) as f64);
    }
    // 다른 사람과 가장 비슷하게 나온 쌍 (정답 이름이 틀렸을 수도 있어 확인용)
    let mut worst: Vec<(f32, &str, &str)> = Vec::new();
    for i in 0..prints.len() {
        for j in i + 1..prints.len() {
            if prints[i].0 != prints[j].0 {
                worst.push((similarity_of(&prints[i].2, &prints[j].2), &prints[i].1, &prints[j].1));
            }
        }
    }
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    for w in worst.iter().take(5) {
        println!("  다른 사람인데 비슷: {:.2} {} ~ {}", w.0, w.1, w.2);
    }
    let mut low: Vec<(f32, &str, &str)> = Vec::new();
    for i in 0..prints.len() {
        for j in i + 1..prints.len() {
            if prints[i].0 == prints[j].0 {
                low.push((similarity_of(&prints[i].2, &prints[j].2), &prints[i].1, &prints[j].1));
            }
        }
    }
    low.sort_by(|a, b| a.0.total_cmp(&b.0));
    for w in low.iter().take(5) {
        println!("  같은 사람인데 다름: {:.2} {} ~ {}", w.0, w.1, w.2);
    }
    Ok(())
}

fn mb(p: &Path) -> f64 {
    std::fs::metadata(p).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0)
}

// ───────────────────────── 측정 ─────────────────────────

fn median_ms<R>(reps: usize, mut f: impl FnMut() -> R) -> (f64, R) {
    let mut times = Vec::with_capacity(reps);
    let mut last = None;
    for _ in 0..reps {
        let t = Instant::now();
        let r = f();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        last = Some(r);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (times[times.len() / 2], last.unwrap())
}

fn fmt_ms(v: Option<f64>) -> String {
    match v {
        Some(v) if v < 10.0 => format!("{v:.1}"),
        Some(v) => format!("{v:.0}"),
        None => "–".into(),
    }
}

fn run(dir: &Path, out: Option<PathBuf>) -> anyhow::Result<()> {
    let mut md = String::new();
    writeln!(md, "# iSee 속도 측정 결과\n")?;
    writeln!(md, "- 측정 장비: {}", machine())?;
    writeln!(md, "- 썸네일 {THUMB_PX}px, 뷰어 화면 {SCREEN_PX}px (긴 변 기준), 각 항목 중간값\n")?;

    // 1) 사진 한 장
    writeln!(md, "## 1. 사진 한 장 처리 시간 (ms)\n")?;
    writeln!(md, "| 파일 | 용량 | 해상도 | 헤더 | **썸네일** | 내장 썸네일 | 축소 디코딩 썸네일 | 화면 크기 디코딩 | **뷰어 준비** (전달 방식) | 범용 Rust 전체 디코딩 |")?;
    writeln!(md, "|---|---|---|---|---|---|---|---|---|---|")?;
    let mut singles: Vec<PathBuf> = std::fs::read_dir(dir.join("singles"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    singles.sort();
    for p in &singles {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let (w, h) = decode::dimensions(p).unwrap_or((0, 0));
        let heavy = w as u64 * h as u64 > 40_000_000;
        let reps = if heavy { 3 } else { 7 };
        let (hdr, _) = median_ms(reps, || decode::dimensions(p).ok());
        let (thumb, _) = median_ms(reps, || decode::thumbnail(p, THUMB_PX).ok());
        let embedded = embedded(p, reps);
        let decoded = decoded_thumb(p, reps);
        let screen_dec = screen_decode_only(p, reps);
        let (view, vi) = median_ms(reps, || viewer::prepare(p, SCREEN_PX, SCREEN_PX).ok());
        let route = vi.map(|v| v.route()).unwrap_or("실패");
        let generic_full = (!name.ends_with(".heic") && !name.ends_with(".psd"))
            .then(|| median_ms(if heavy { 1 } else { 3 }, || generic::decode_full(p).ok()))
            .and_then(|(t, r)| r.map(|_| t));
        writeln!(
            md,
            "| {name} | {:.1}MB | {w}×{h} | {} | **{}** | {} | {} | {} | **{}** ({route}) | {} |",
            mb(p),
            fmt_ms(Some(hdr)),
            fmt_ms(Some(thumb)),
            fmt_ms(embedded),
            fmt_ms(decoded),
            fmt_ms(screen_dec),
            fmt_ms(Some(view)),
            fmt_ms(generic_full),
        )?;
        print!(".");
    }
    println!();

    // 2) 폴더
    let folder = dir.join("folder");
    let (list_ms, entries) = median_ms(5, || listing::list_dir(&folder, false).unwrap());
    let reqs: Vec<Req> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| Req {
            path: e.path.clone(),
            size: e.size,
            mtime: e.mtime,
            index: i as i64,
        })
        .collect();
    let n = reqs.len();
    writeln!(md, "\n## 2. 폴더 ({n}장: 12MP JPEG 300, 24MP JPEG 60, 12MP HEIC 40)\n")?;
    writeln!(md, "- 폴더 목록 읽기: **{list_ms:.1}ms**")?;
    let cores = std::thread::available_parallelism().map_or(8, |n| n.get());
    let workers = cores.saturating_sub(2).max(2);

    // 2-1) 처음 연 폴더: 첫 화면 48칸이 다 뜰 때까지
    let svc = ThumbService::new(ThumbCache::open(None, 20_000)?, THUMB_PX, workers);
    let first = first_screen(&svc, &reqs, 48);
    writeln!(md, "- 처음 연 폴더, 첫 화면 48칸 모두 표시: **{first:.0}ms** (작업 스레드 {workers}개)")?;

    // 2-2) 폴더 전체 썸네일 생성 속도 (작업 스레드 수별)
    writeln!(md, "\n| 작업 스레드 | 전체 {n}장 생성 | 초당 |")?;
    writeln!(md, "|---|---|---|")?;
    for wk in [4, 8, workers] {
        let svc = ThumbService::new(ThumbCache::open(None, 20_000)?, THUMB_PX, wk);
        let t = Instant::now();
        svc.set_folder(reqs.clone());
        wait_until(|| svc.stats().generated.load(Ordering::Relaxed) as usize + svc.stats().failed.load(Ordering::Relaxed) as usize >= n);
        let s = t.elapsed().as_secs_f64();
        writeln!(md, "| {wk} | {:.2}초 | **{:.0}장** |", s, n as f64 / s)?;
        print!(".");
    }
    println!();

    // 2-3) 디스크 캐시: 두 번째로 열 때
    let db = std::env::temp_dir().join(format!("isee-bench-{}.db", std::process::id()));
    {
        let svc = ThumbService::new(ThumbCache::open(Some(&db), 20_000)?, THUMB_PX, workers);
        svc.set_folder(reqs.clone());
        wait_until(|| svc.stats().generated.load(Ordering::Relaxed) as usize >= n);
        std::thread::sleep(Duration::from_millis(800)); // 기록 스레드가 다 쓸 때까지
    }
    let svc = ThumbService::new(ThumbCache::open(Some(&db), 20_000)?, THUMB_PX, workers);
    let first_cached = first_screen(&svc, &reqs[..48.min(n)].to_vec(), 48);
    let svc = ThumbService::new(ThumbCache::open(Some(&db), 20_000)?, THUMB_PX, workers);
    let t = Instant::now();
    request_all(&svc, &reqs);
    let all_cached = t.elapsed().as_secs_f64() * 1000.0;
    let db_mb = mb(&db) + mb(&db.with_extension("db-wal"));
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_file(db.with_extension("db-wal"));
    let _ = std::fs::remove_file(db.with_extension("db-shm"));
    writeln!(md, "\n- 다시 연 폴더(디스크 캐시), 첫 화면 48칸: **{first_cached:.1}ms**")?;
    writeln!(md, "- 다시 연 폴더(디스크 캐시), {n}장 전부: **{all_cached:.0}ms**")?;
    writeln!(md, "- 캐시 용량: {:.1}MB ({:.0}KB/장)", db_mb, db_mb * 1000.0 / n as f64)?;

    // 2-4) 미리 불러오기 없이 사진 넘기기 (최악의 경우)
    let jpgs: Vec<&Req> = reqs.iter().filter(|r| r.path.ends_with(".jpg")).take(20).collect();
    let mut flip = Vec::new();
    for r in &jpgs {
        let t = Instant::now();
        let _ = viewer::prepare(Path::new(&r.path), SCREEN_PX, SCREEN_PX);
        flip.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    flip.sort_by(|a, b| a.partial_cmp(b).unwrap());
    writeln!(
        md,
        "- 미리 불러오기 없이 다음 사진 준비 (12MP JPEG, 화면 크기): 중간값 **{:.0}ms**",
        flip[flip.len() / 2]
    )?;
    writeln!(md, "  (미리 불러온 사진은 메모리에서 바로 나가므로 0ms에 가깝다)")?;

    println!("\n{md}");
    if let Some(out) = out {
        std::fs::write(&out, &md)?;
        println!("저장: {}", out.display());
    }
    Ok(())
}

/// 변환 속도: 테스트 사진 몇 장을 임시 폴더에 복사해서 작업별 시간과 결과 크기를 잰다.
fn convert_bench(dir: &Path) -> anyhow::Result<()> {
    use isee_core::convert::{self, OutFormat, Resize, SaveMode, Spec, Turn};
    let work = std::env::temp_dir().join(format!("isee-convert-bench-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let files = ["jpeg_12mp_phone.jpg", "jpeg_24mp_camera.jpg", "heic_12mp_phone.heic", "png_24mp.png"];
    let specs: Vec<(&str, Spec)> = vec![
        ("회전 (덮어쓰기)", Spec { turn: Some(Turn::Cw90), save: SaveMode::Overwrite, ..Default::default() }),
        ("긴 변 1920px", Spec { resize: Some(Resize::LongSide(1920)), ..Default::default() }),
        ("50%", Spec { resize: Some(Resize::Percent(50)), ..Default::default() }),
        ("JPG로", Spec { format: Some(OutFormat::Jpeg), ..Default::default() }),
        ("WebP로", Spec { format: Some(OutFormat::Webp), ..Default::default() }),
        ("HEIC로", Spec { format: Some(OutFormat::Heic), ..Default::default() }),
        ("1MB 이하", Spec { max_bytes: Some(1_000_000), ..Default::default() }),
        ("공유용 (2048px·JPG·위치삭제)", Spec {
            resize: Some(Resize::LongSide(2048)),
            format: Some(OutFormat::Jpeg),
            quality: Some(85),
            strip_gps: true,
            suffix: Some("공유".into()),
            ..Default::default()
        }),
    ];
    println!("| 작업 | {} |", files.join(" | "));
    println!("|---|{}", "---|".repeat(files.len()));
    for (name, spec) in &specs {
        let mut row = format!("| {name} |");
        for f in files {
            let src = work.join(f);
            std::fs::copy(dir.join("singles").join(f), &src)?;
            let t = Instant::now();
            let r = convert::run_one(&src, spec, Some(&work.join("undo")));
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok(d) => row += &format!(
                    " {ms:.0}ms → {:.1}MB{} |",
                    d.bytes as f64 / 1e6,
                    if d.lossless { " (무손실)" } else { "" }
                ),
                Err(e) => row += &format!(" 실패: {e} |"),
            }
        }
        println!("{row}");
    }
    // 여러 장 동시: 12MP JPEG 40장 → 긴 변 1920px
    let many: Vec<String> = (0..40)
        .map(|i| {
            let p = work.join(format!("m{i:02}.jpg"));
            std::fs::copy(dir.join("singles/jpeg_12mp_phone.jpg"), &p).unwrap();
            p.to_string_lossy().into_owned()
        })
        .collect();
    let t = Instant::now();
    let rs = convert::run_batch(&many, &Spec { resize: Some(Resize::LongSide(1920)), ..Default::default() }, None, &|_, _| {});
    println!(
        "\n12MP JPEG 40장 → 긴 변 1920px (동시 처리): {:.2}초, 장당 {:.0}ms, 실패 {}",
        t.elapsed().as_secs_f64(),
        t.elapsed().as_secs_f64() * 1000.0 / 40.0,
        rs.iter().filter(|r| r.is_err()).count()
    );
    std::fs::remove_dir_all(&work)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn embedded(p: &Path, reps: usize) -> Option<f64> {
    let (t, r) = median_ms(reps, || decode::imageio::embedded_thumbnail(p, THUMB_PX).ok());
    r.map(|_| t)
}
#[cfg(not(target_os = "macos"))]
fn embedded(p: &Path, reps: usize) -> Option<f64> {
    let (t, r) = median_ms(reps, || generic::exif_thumbnail(p));
    r.map(|_| t)
}

#[cfg(target_os = "macos")]
fn decoded_thumb(p: &Path, reps: usize) -> Option<f64> {
    let (t, r) = median_ms(reps, || decode::imageio::decoded_thumbnail(p, THUMB_PX).ok());
    r.map(|_| t)
}
#[cfg(not(target_os = "macos"))]
fn decoded_thumb(_p: &Path, _reps: usize) -> Option<f64> {
    None
}

#[cfg(target_os = "macos")]
fn screen_decode_only(p: &Path, reps: usize) -> Option<f64> {
    let (t, r) = median_ms(reps, || decode::imageio::decode_only(p, SCREEN_PX).ok());
    r.map(|_| t)
}
#[cfg(not(target_os = "macos"))]
fn screen_decode_only(_p: &Path, _reps: usize) -> Option<f64> {
    None
}

fn wait_until(mut done: impl FnMut() -> bool) {
    while !done() {
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// 폴더를 열고 앞쪽 visible칸을 화면에 보이는 칸으로 요청해서, 모두 도착할 때까지의 시간 (ms).
fn first_screen(svc: &ThumbService, reqs: &[Req], visible: usize) -> f64 {
    let t = Instant::now();
    svc.set_folder(reqs.to_vec());
    let visible = visible.min(reqs.len());
    svc.set_viewport(0, visible as i64 - 1);
    let (tx, rx) = mpsc::channel();
    for r in &reqs[..visible] {
        let tx = tx.clone();
        svc.request(r.clone(), Box::new(move |_| {
            let _ = tx.send(());
        }));
    }
    for _ in 0..visible {
        rx.recv().unwrap();
    }
    t.elapsed().as_secs_f64() * 1000.0
}

fn request_all(svc: &ThumbService, reqs: &[Req]) {
    let left = Arc::new(AtomicUsize::new(reqs.len()));
    svc.set_viewport(0, reqs.len() as i64);
    for r in reqs {
        let left = left.clone();
        svc.request(r.clone(), Box::new(move |_| {
            left.fetch_sub(1, Ordering::Relaxed);
        }));
    }
    wait_until(|| left.load(Ordering::Relaxed) == 0);
}

fn machine() -> String {
    let sysctl = |k: &str| {
        std::process::Command::new("sysctl")
            .args(["-n", k])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let cpu = sysctl("machdep.cpu.brand_string");
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    let mem = sysctl("hw.memsize").parse::<u64>().unwrap_or(0) / (1 << 30);
    format!("{cpu}, {cores}코어, 메모리 {mem}GB, {}", std::env::consts::OS)
}
