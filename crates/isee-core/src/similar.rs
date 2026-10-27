//! 비슷한 사진 묶기와 베스트 추천. 저장된 특징값만 쓰므로 슬라이더를 움직일 때마다 즉시 다시 계산된다.

use crate::analysis::{Features, content_hash};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub struct Item {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    /// 사용자가 직접 남김(1) / 버림(-1) 표시한 것
    pub flag: i8,
    pub rating: u8,
    pub f: Features,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Rule {
    Sharp,
    Exposure,
    Noise,
    Resolution,
    Newest,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Options {
    /// 묶는 정도 1(아주 비슷한 것만) ~ 5(대충 비슷해도)
    pub level: u8,
    /// 그룹마다 추천할 장수
    pub keep: usize,
    /// 앞에 있을수록 중요한 기준
    pub rules: Vec<Rule>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            level: 3,
            keep: 1,
            rules: vec![Rule::Sharp, Rule::Exposure, Rule::Noise, Rule::Resolution, Rule::Newest],
        }
    }
}

/// 화면에 썸네일을 띄우는 데 필요한 정보
#[derive(Clone, Debug, Serialize)]
pub struct Photo {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub width: u32,
    pub height: u32,
}

impl Item {
    fn photo(&self) -> Photo {
        Photo { path: self.path.clone(), size: self.size, mtime: self.mtime, width: self.f.width, height: self.f.height }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Member {
    #[serde(flatten)]
    pub photo: Photo,
    /// 그룹 안에서의 점수 (0~1, 높을수록 좋음)
    pub score: f32,
    pub keep: bool,
    /// 짧은 설명: "가장 선명", "흐림 (추천 사진의 30%)" 등
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Group {
    pub members: Vec<Member>,
    pub taken: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub groups: Vec<Group>,
    /// 내용이 완전히 같은 파일 묶음 (첫 번째가 남길 것)
    pub duplicates: Vec<Vec<Photo>>,
    pub blurry: Vec<Photo>,
    pub dark: Vec<Photo>,
    pub bright: Vec<Photo>,
    pub analyzed: usize,
    pub total: usize,
    /// 클라우드에만 있어(내려받지 않음) 분석하지 못한 사진 수 (앱이 채운다)
    pub cloud: usize,
    pub ms: f64,
}

/// 묶는 정도별 지문 차이 허용치 (64비트 중 다른 비트 수)
fn threshold(level: u8) -> u32 {
    [4, 7, 10, 13, 16][level.clamp(1, 5) as usize - 1]
}

/// 연달아 찍은 것으로 보는 시간 간격 (초)
const RAPID_SECS: f64 = 10.0;
const BURST_SECS: f64 = 90.0;

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, mut a: usize) -> usize {
        while self.0[a] != a {
            self.0[a] = self.0[self.0[a]];
            a = self.0[a];
        }
        a
    }
}

fn dist(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 비슷한 것끼리 묶는다. 결과는 2장 이상인 묶음의 인덱스들 (촬영 순).
pub fn cluster(items: &[Item], level: u8) -> Vec<Vec<usize>> {
    let t = threshold(level);
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[a].f.taken.total_cmp(&items[b].f.taken));
    let mut dsu = Dsu((0..items.len()).collect());
    // 사진이 아주 많으면 촬영 시각이 가까운 것끼리만 비교 (같은 장면은 대개 연달아 찍힌다)
    let window = if items.len() <= 6000 { items.len() } else { 400 };
    for (oi, &a) in order.iter().enumerate() {
        for &b in order.iter().skip(oi + 1).take(window) {
            let (fa, fb) = (&items[a].f, &items[b].f);
            let dt = (fb.taken - fa.taken).abs();
            // 몇 초 안에 연달아 찍은 것은 조금 너그럽게, 시간이 떨어져 있으면 더 엄격하게
            // (다른 날 비슷한 구도를 같은 사진으로 묶지 않도록)
            let limit = if dt <= RAPID_SECS {
                t * 13 / 10
            } else if dt <= BURST_SECS {
                t
            } else {
                t * 6 / 10
            };
            if dist(fa.phash, fb.phash) > limit {
                continue;
            }
            let (ra, rb) = (dsu.find(a), dsu.find(b));
            if ra == rb {
                continue;
            }
            // 사슬처럼 이어져 전혀 다른 사진까지 끌려오지 않게: 대표끼리도 너무 멀면 합치지 않는다
            if dist(items[ra].f.phash, items[rb].f.phash) > limit * 2 {
                continue;
            }
            let (keep, other) = if items[ra].f.taken <= items[rb].f.taken { (ra, rb) } else { (rb, ra) };
            dsu.0[other] = keep;
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &order {
        let r = dsu.find(i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() >= 2).collect();
    out.sort_by(|a, b| items[a[0]].f.taken.total_cmp(&items[b[0]].f.taken));
    out
}

/// 노출 점수 (0~1): 날아가거나 묻힌 부분, 너무 어둡거나 밝은 평균을 깎는다
fn exposure(f: &Features) -> f32 {
    let off = ((f.mean - 0.47).abs() - 0.15).max(0.0) * 1.6;
    (1.0 - f.clip_hi * 3.0 - f.clip_lo * 2.0 - off).clamp(0.0, 1.0)
}

const WEIGHTS: [f32; 5] = [1.0, 0.5, 0.25, 0.12, 0.06];

/// 그룹 안에서 기준별 점수(0~1, 그룹 최고 = 1)를 매긴다
fn criteria(items: &[Item], g: &[usize], rule: Rule) -> Vec<f32> {
    let vals: Vec<f32> = g
        .iter()
        .map(|&i| {
            let f = &items[i].f;
            match rule {
                // 선명도는 사람 눈에 비율로 느껴지므로 제곱근
                Rule::Sharp => f.sharp.max(0.0).sqrt(),
                Rule::Exposure => exposure(f),
                Rule::Noise => 1.0 / (f.noise + 0.5),
                Rule::Resolution => (f.width as f32 * f.height as f32).sqrt(),
                Rule::Newest => f.taken as f32,
            }
        })
        .collect();
    let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    if rule == Rule::Newest {
        // 같은 장면이면 나중 사진이 조금 낫다는 정도의 약한 기준
        return vals.iter().map(|&v| if hi > lo { (v - lo) / (hi - lo) } else { 1.0 }).collect();
    }
    vals.iter().map(|&v| if hi > 0.0 { v / hi } else { 1.0 }).collect()
}

fn pct(v: f32) -> i32 {
    (v * 100.0).round() as i32
}

fn score_group(items: &[Item], g: &[usize], opt: &Options) -> Vec<Member> {
    let rules: Vec<Rule> = if opt.rules.is_empty() { Options::default().rules } else { opt.rules.clone() };
    let per: Vec<(Rule, Vec<f32>)> = rules.iter().map(|&r| (r, criteria(items, g, r))).collect();
    let wsum: f32 = rules.iter().enumerate().map(|(i, _)| WEIGHTS[i.min(4)]).sum();
    // (그룹 안 위치, 사진 점수, 순위용 점수)
    let mut scored: Vec<(usize, f32, f32)> = g
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let s: f32 = per.iter().enumerate().map(|(ri, (_, v))| v[k] * WEIGHTS[ri.min(4)]).sum::<f32>() / wsum;
            // 사용자가 직접 남긴 것·별점 준 것은 우선, 버린 것은 뒤로
            let it = &items[i];
            let user = it.flag as f32 + it.rating as f32 * 0.1;
            // 크게 날아가거나 묻힌 사진은 다른 기준이 좋아도 추천하지 않는다
            let s = if it.f.clip_hi > 0.25 || it.f.clip_lo > 0.4 { s * 0.6 } else { s };
            (k, s, s + user)
        })
        .collect();
    // 점수가 같으면 (똑같은 파일 등) 이름이 짧은 것 → 오래된 것
    scored.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| items[g[a.0]].path.len().cmp(&items[g[b.0]].path.len()))
            .then_with(|| items[g[a.0]].mtime.cmp(&items[g[b.0]].mtime))
    });
    let keep_n = opt.keep.clamp(1, g.len().saturating_sub(1).max(1));
    let get = |r: Rule, k: usize| per.iter().find(|(pr, _)| *pr == r).map(|(_, v)| v[k]);
    scored
        .iter()
        .enumerate()
        .map(|(rank, &(k, s, _))| {
            let it = &items[g[k]];
            let keep = rank < keep_n;
            let mut reasons = Vec::new();
            if it.flag > 0 {
                reasons.push("직접 남김".into());
            }
            if it.rating > 0 {
                reasons.push(format!("별점 {}", it.rating));
            }
            if it.flag < 0 {
                reasons.push("직접 버림".into());
            }
            // 좋은 점은 추천 사진에만, 아쉬운 점은 모두에게 적는다
            if let Some(v) = get(Rule::Sharp, k) {
                if v >= 0.999 && keep {
                    reasons.push("가장 선명".into());
                } else if v < 0.75 {
                    // 표시는 실제 분산 비율로 (제곱해서 되돌림)
                    reasons.push(format!("덜 선명 (최고의 {}%)", pct(v * v)));
                }
            }
            if it.f.clip_hi > 0.08 {
                reasons.push(format!("밝은 부분 날아감 {}%", pct(it.f.clip_hi)));
            } else if it.f.clip_lo > 0.15 {
                reasons.push(format!("어두운 부분 묻힘 {}%", pct(it.f.clip_lo)));
            } else if exposure(&it.f) < 0.85 {
                reasons.push(if it.f.mean > 0.47 { "너무 밝음" } else { "너무 어두움" }.into());
            } else if let Some(v) = get(Rule::Exposure, k)
                && v >= 0.999
                && keep
            {
                reasons.push("노출 좋음".into());
            }
            if let Some(v) = get(Rule::Noise, k)
                && v < 0.7
                && it.f.noise > 2.0
            {
                reasons.push("노이즈 많음".into());
            }
            if let Some(v) = get(Rule::Resolution, k)
                && v < 0.9
            {
                reasons.push(format!("해상도 낮음 {}×{}", it.f.width, it.f.height));
            }
            Member { photo: it.photo(), score: s, keep, reasons }
        })
        .collect()
}

/// 내용이 똑같은 파일 묶음: 크기가 같은 것만 해시를 비교한다
pub fn duplicates(items: &[Item]) -> Vec<Vec<usize>> {
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, it) in items.iter().enumerate() {
        if it.size > 0 {
            by_size.entry(it.size).or_default().push(i);
        }
    }
    let cands: Vec<usize> = by_size.into_values().filter(|v| v.len() > 1).flatten().collect();
    let hashed: Vec<(usize, Option<u64>)> =
        cands.par_iter().map(|&i| (i, content_hash(Path::new(&items[i].path)))).collect();
    let mut by_hash: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (i, h) in hashed {
        if let Some(h) = h {
            by_hash.entry((items[i].size, h)).or_default().push(i);
        }
    }
    let mut out: Vec<Vec<usize>> = by_hash
        .into_values()
        .filter(|v| v.len() > 1)
        .map(|mut v| {
            // 남길 것: 직접 남긴 것 → 이름이 짧은 것("사본", " 2"가 없는 것) → 오래된 것
            v.sort_by(|&a, &b| {
                items[b].flag.cmp(&items[a].flag)
                    .then_with(|| items[a].path.len().cmp(&items[b].path.len()))
                    .then_with(|| items[a].mtime.cmp(&items[b].mtime))
                    .then_with(|| items[a].path.cmp(&items[b].path))
            });
            v
        })
        .collect();
    out.sort_by(|a, b| items[a[0]].path.cmp(&items[b[0]].path));
    out
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

/// 분석이 끝난 사진들로 보고서를 만든다 (total: 폴더 안 전체 사진 수)
pub fn report(items: &[Item], total: usize, opt: &Options) -> Report {
    let t0 = std::time::Instant::now();
    let dups = duplicates(items);
    // 사본(같은 파일 묶음의 두 번째부터) → 원본 경로
    let copy_of: HashMap<&str, &str> = dups
        .iter()
        .flat_map(|v| v[1..].iter().map(|&i| (items[i].path.as_str(), items[v[0]].path.as_str())))
        .collect();
    let groups = cluster(items, opt.level)
        .iter()
        .map(|g| {
            let members = score_group(items, g, opt);
            let keep_n = members.iter().filter(|m| m.keep).count();
            // 같은 묶음 안에 원본이 있는 사본은 맨 뒤로 (똑같은 사진을 두 장 추천하지 않게)
            let paths: Vec<&str> = members.iter().map(|m| m.photo.path.as_str()).collect();
            let is_copy = |m: &Member| copy_of.get(m.photo.path.as_str()).is_some_and(|o| paths.contains(o));
            let (mut members, copies): (Vec<Member>, Vec<Member>) = members.iter().cloned().partition(|m| !is_copy(m));
            for mut m in copies {
                let orig = copy_of[m.photo.path.as_str()];
                let name = Path::new(orig).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                m.reasons = vec![format!("{name}와 똑같은 파일")];
                members.push(m);
            }
            for (i, m) in members.iter_mut().enumerate() {
                m.keep = i < keep_n.min(paths.len().saturating_sub(1)).max(1);
            }
            Group { members, taken: items[g[0]].f.taken }
        })
        .collect();
    let duplicates = dups.into_iter().map(|v| v.into_iter().map(|i| items[i].photo()).collect()).collect();
    // 흐린 사진: 폴더 안 보통 선명도의 30%도 안 되는 것 (폴더마다 카메라·장면이 달라 상대값으로)
    let typical = median(items.iter().map(|it| it.f.sharp).collect());
    let pick = |pred: &dyn Fn(&Features) -> bool| -> Vec<Photo> {
        items.iter().filter(|it| it.flag <= 0 && pred(&it.f)).map(|it| it.photo()).collect()
    };
    let blurry = if items.len() >= 3 { pick(&|f| f.sharp < typical * 0.3) } else { Vec::new() };
    let dark = pick(&|f| f.mean < 0.12 || f.clip_lo > 0.5);
    let bright = pick(&|f| f.mean > 0.9 || f.clip_hi > 0.35);
    Report {
        groups,
        duplicates,
        blurry,
        dark,
        bright,
        analyzed: items.len(),
        total,
        cloud: 0,
        ms: t0.elapsed().as_secs_f64() * 1000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, phash: u64, taken: f64, sharp: f32) -> Item {
        Item {
            path: format!("/x/{name}"),
            size: 1000 + name.len() as u64,
            mtime: 0,
            flag: 0,
            rating: 0,
            f: Features { phash, sharp, mean: 0.45, noise: 2.0, width: 4000, height: 3000, taken, ..Default::default() },
        }
    }

    #[test]
    fn groups_and_best() {
        let a = 0x0123_4567_89ab_cdefu64;
        let b = !a;
        let items = vec![
            item("a1.jpg", a, 0.0, 300.0),
            item("a2.jpg", a ^ 0b111, 2.0, 900.0), // 같은 장면, 더 선명
            item("a3.jpg", a ^ 0b1_1000, 4.0, 100.0),
            item("b1.jpg", b, 10.0, 500.0),
            item("b2.jpg", b ^ 1, 11.0, 400.0),
            item("c.jpg", a ^ 0xffff_0000, 5000.0, 400.0), // 시간도 멀고 많이 다름
        ];
        let r = report(&items, items.len(), &Options::default());
        assert_eq!(r.groups.len(), 2);
        let g = &r.groups[0];
        assert_eq!(g.members.len(), 3);
        assert_eq!(g.members[0].photo.path, "/x/a2.jpg");
        assert!(g.members[0].keep && !g.members[1].keep);
        assert!(g.members[0].reasons.iter().any(|s| s == "가장 선명"));
        assert_eq!(r.groups[1].members[0].photo.path, "/x/b1.jpg");
    }

    #[test]
    fn user_pick_wins() {
        let a = 42u64;
        let mut items = vec![item("a1.jpg", a, 0.0, 900.0), item("a2.jpg", a, 1.0, 100.0)];
        items[1].flag = 1;
        let r = report(&items, 2, &Options::default());
        assert_eq!(r.groups[0].members[0].photo.path, "/x/a2.jpg");
    }

    #[test]
    fn distant_time_is_stricter() {
        let a = 0u64;
        let items = vec![item("a.jpg", a, 0.0, 1.0), item("b.jpg", 0b11_1111_1111, 100_000.0, 1.0)];
        // 10비트 차이: 연속 촬영이면 묶이지만(level 3 = 10) 하루 넘게 떨어져 있으면 따로
        assert!(cluster(&items, 3).is_empty());
        let near = vec![item("a.jpg", a, 0.0, 1.0), item("b.jpg", 0b11_1111_1111, 30.0, 1.0)];
        assert_eq!(cluster(&near, 3).len(), 1);
    }
}
