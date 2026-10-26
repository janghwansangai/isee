//! 오프라인 지명 찾기 (역지오코딩): GPS 좌표 → "제주 서귀포시 중문동", "일본 오사카".
//! 지명 데이터(data/places.bin, 약 2.4MB)를 앱에 넣어 두므로 인터넷 없이 바로 찾고, 좌표가 밖으로 나가지 않는다.
//!   - 국내: 행정동 경계(admdongkor) 안에 있는지로 시도·시군구·읍면동을 찾는다.
//!   - 해외: GeoNames 인구 5천 명 이상 도시 중 가장 가까운 곳.
//! 처음 쓸 때 한 번 읽는다 (약 20ms).

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

static DATA: &[u8] = include_bytes!("../data/places.bin");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// 대한민국 / 일본
    Country,
    /// 서울 / 제주 (해외는 나라+도시)
    Province,
    /// 제주 서귀포시 / 일본 오사카
    #[default]
    City,
    /// 제주 서귀포시 중문동
    Town,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub country: &'static str,
    /// 국내 행정구역 (경계 안에 있을 때)
    pub korea: Option<Korea>,
    /// 가장 가까운 도시 (해외, 또는 국내 경계 밖 바다 위 등)
    pub city: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Korea {
    pub sido: &'static str,
    /// "서울", "제주", "경북"
    pub sido_short: &'static str,
    /// "서귀포시", "수원시 장안구" (세종은 빈 문자열)
    pub sgg: &'static str,
    pub dong: &'static str,
}

impl Place {
    /// 폴더 이름에 쓸 지명
    pub fn label(&self, level: Level) -> String {
        if let Some(k) = &self.korea {
            let mut parts = vec![k.sido_short];
            if matches!(level, Level::City | Level::Town) && !k.sgg.is_empty() {
                parts.push(k.sgg);
            }
            if level == Level::Town {
                parts.push(k.dong);
            }
            return match level {
                Level::Country => self.country.to_string(),
                _ => parts.join(" "),
            };
        }
        match (level, self.city) {
            // 홍콩·싱가포르처럼 도시 이름이 나라 이름과 같으면 한 번만
            (Level::Country, _) | (_, None) => self.country.to_string(),
            (_, Some(c)) if c == self.country => c.to_string(),
            (_, Some(c)) => format!("{} {c}", self.country),
        }
    }
}

struct City {
    name: u32,
    country: u16,
    lat: f32,
    lon: f32,
    /// 큰 도시일수록 조금 더 멀어도 고른다 (km). 에펠탑 → 근처 소도시 대신 "파리"
    bonus: f32,
}

struct Dong {
    sgg: u16,
    name: u32,
    /// (시작, 끝) 점 범위 (points 안)
    rings: Vec<(u32, u32)>,
    bbox: [i32; 4],
}

struct Db {
    strings: Vec<&'static str>,
    countries: Vec<([u8; 2], u32)>,
    cities: Vec<City>,
    /// 1°×1° 칸별 도시 번호
    city_grid: Vec<Vec<u32>>,
    sidos: Vec<(u32, u32)>,
    sggs: Vec<(u16, u32)>,
    dongs: Vec<Dong>,
    /// 경도·위도 ×1e5
    points: Vec<[i32; 2]>,
    /// 국내 0.05° 칸별 읍면동 번호
    dong_grid: Vec<Vec<u16>>,
    korea_idx: Option<u16>,
}

// 국내 칸 범위
const K_LAT0: f64 = 32.5;
const K_LON0: f64 = 123.5;
const K_STEP: f64 = 0.05;
const K_ROWS: usize = 150; // 32.5 ~ 40.0
const K_COLS: usize = 180; // 123.5 ~ 132.5

struct Reader {
    pos: usize,
}

impl Reader {
    fn uint(&mut self) -> u64 {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let b = DATA[self.pos];
            self.pos += 1;
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return v;
            }
            shift += 7;
        }
    }
    fn int(&mut self) -> i64 {
        let u = self.uint();
        ((u >> 1) as i64) ^ -((u & 1) as i64)
    }
    fn bytes(&mut self, n: usize) -> &'static [u8] {
        let s = &DATA[self.pos..self.pos + n];
        self.pos += n;
        s
    }
}

fn load() -> Db {
    assert_eq!(&DATA[..4], b"ISPL", "지명 데이터 형식");
    let mut r = Reader { pos: 6 };
    let n = r.uint() as usize;
    let strings: Vec<&'static str> = (0..n)
        .map(|_| {
            let len = r.uint() as usize;
            std::str::from_utf8(r.bytes(len)).unwrap_or("")
        })
        .collect();
    let countries: Vec<([u8; 2], u32)> = (0..r.uint())
        .map(|_| {
            let c = r.bytes(2);
            ([c[0], c[1]], r.uint() as u32)
        })
        .collect();
    let cities: Vec<City> = (0..r.uint())
        .map(|_| City {
            name: r.uint() as u32,
            country: r.uint() as u16,
            lat: r.int() as f32 / 1e4,
            lon: r.int() as f32 / 1e4,
            bonus: 2.0 * ((r.uint() as f32 * 1000.0).max(5000.0) / 5000.0).ln(),
        })
        .collect();
    let mut city_grid = vec![Vec::new(); 180 * 360];
    for (i, c) in cities.iter().enumerate() {
        city_grid[grid_cell(c.lat as f64, c.lon as f64)].push(i as u32);
    }
    let sidos: Vec<(u32, u32)> = (0..r.uint()).map(|_| (r.uint() as u32, r.uint() as u32)).collect();
    let sggs: Vec<(u16, u32)> = (0..r.uint()).map(|_| (r.uint() as u16, r.uint() as u32)).collect();
    let mut points = Vec::new();
    let mut dongs = Vec::new();
    for _ in 0..r.uint() {
        let sgg = r.uint() as u16;
        let name = r.uint() as u32;
        let mut rings = Vec::new();
        let mut bbox = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for _ in 0..r.uint() {
            let start = points.len() as u32;
            let (mut x, mut y) = (0i64, 0i64);
            for _ in 0..r.uint() {
                x += r.int();
                y += r.int();
                let p = [x as i32, y as i32];
                bbox = [bbox[0].min(p[0]), bbox[1].min(p[1]), bbox[2].max(p[0]), bbox[3].max(p[1])];
                points.push(p);
            }
            rings.push((start, points.len() as u32));
        }
        dongs.push(Dong { sgg, name, rings, bbox });
    }
    let mut dong_grid = vec![Vec::new(); K_ROWS * K_COLS];
    for (i, d) in dongs.iter().enumerate() {
        let (c0, r0) = k_cell(d.bbox[1] as f64 / 1e5, d.bbox[0] as f64 / 1e5);
        let (c1, r1) = k_cell(d.bbox[3] as f64 / 1e5, d.bbox[2] as f64 / 1e5);
        for row in r0..=r1 {
            for col in c0..=c1 {
                dong_grid[row * K_COLS + col].push(i as u16);
            }
        }
    }
    let korea_idx = countries.iter().position(|(c, _)| c == b"KR").map(|i| i as u16);
    Db { strings, countries, cities, city_grid, sidos, sggs, dongs, points, dong_grid, korea_idx }
}

fn db() -> &'static Db {
    static DB: OnceLock<Db> = OnceLock::new();
    DB.get_or_init(load)
}

/// 미리 읽어 둔다 (정리 화면을 열 때 백그라운드에서)
pub fn warm() {
    db();
}

fn grid_cell(lat: f64, lon: f64) -> usize {
    let row = ((lat + 90.0).floor() as isize).clamp(0, 179) as usize;
    let col = ((lon + 180.0).floor() as isize).rem_euclid(360) as usize;
    row * 360 + col
}

/// 국내 칸 (열, 행), 범위 밖이면 가장자리로
fn k_cell(lat: f64, lon: f64) -> (usize, usize) {
    let col = (((lon - K_LON0) / K_STEP).floor() as isize).clamp(0, K_COLS as isize - 1) as usize;
    let row = (((lat - K_LAT0) / K_STEP).floor() as isize).clamp(0, K_ROWS as isize - 1) as usize;
    (col, row)
}

fn in_korea_box(lat: f64, lon: f64) -> bool {
    (K_LAT0..K_LAT0 + K_STEP * K_ROWS as f64).contains(&lat) && (K_LON0..K_LON0 + K_STEP * K_COLS as f64).contains(&lon)
}

/// 두 지점 사이 거리 (km)
fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.sqrt().asin()
}

impl Db {
    fn s(&self, i: u32) -> &'static str {
        self.strings.get(i as usize).copied().unwrap_or("")
    }

    fn contains(&self, d: &Dong, x: i32, y: i32) -> bool {
        if x < d.bbox[0] || x > d.bbox[2] || y < d.bbox[1] || y > d.bbox[3] {
            return false;
        }
        // 짝홀 규칙: 여러 조각·구멍이 있어도 그대로 맞다
        let mut inside = false;
        for &(a, b) in &d.rings {
            let ring = &self.points[a as usize..b as usize];
            let mut j = ring.len().saturating_sub(1);
            for i in 0..ring.len() {
                let ([xi, yi], [xj, yj]) = (ring[i], ring[j]);
                if (yi > y) != (yj > y) {
                    let cross = (xj - xi) as f64 * (y - yi) as f64 / (yj - yi) as f64 + xi as f64;
                    if (x as f64) < cross {
                        inside = !inside;
                    }
                }
                j = i;
            }
        }
        inside
    }

    /// 경계까지 가장 가까운 거리 (km, 근사)
    fn distance_to(&self, d: &Dong, lat: f64, lon: f64) -> f64 {
        let kx = 111.32 * lat.to_radians().cos() / 1e5;
        let ky = 110.57 / 1e5;
        let (px, py) = (lon * 1e5, lat * 1e5);
        let mut best = f64::MAX;
        for &(a, b) in &d.rings {
            let ring = &self.points[a as usize..b as usize];
            for w in ring.windows(2) {
                let (ax, ay) = ((w[0][0] as f64 - px) * kx, (w[0][1] as f64 - py) * ky);
                let (bx, by) = ((w[1][0] as f64 - px) * kx, (w[1][1] as f64 - py) * ky);
                let (dx, dy) = (bx - ax, by - ay);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 { (-(ax * dx + ay * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let (cx, cy) = (ax + t * dx, ay + t * dy);
                best = best.min((cx * cx + cy * cy).sqrt());
            }
        }
        best
    }

    fn korea(&self, lat: f64, lon: f64) -> Option<usize> {
        if !in_korea_box(lat, lon) {
            return None;
        }
        let (x, y) = ((lon * 1e5).round() as i32, (lat * 1e5).round() as i32);
        let (col, row) = k_cell(lat, lon);
        if let Some(&i) = self.dong_grid[row * K_COLS + col].iter().find(|&&i| self.contains(&self.dongs[i as usize], x, y)) {
            return Some(i as usize);
        }
        // 경계를 단순화하며 생긴 틈이나 해안선 바로 밖(배 위, 갯벌 등): 1.5km 안의 가장 가까운 동
        let mut best = (1.5, None);
        for r in row.saturating_sub(1)..=(row + 1).min(K_ROWS - 1) {
            for c in col.saturating_sub(1)..=(col + 1).min(K_COLS - 1) {
                for &i in &self.dong_grid[r * K_COLS + c] {
                    let d = self.distance_to(&self.dongs[i as usize], lat, lon);
                    if d < best.0 {
                        best = (d, Some(i as usize));
                    }
                }
            }
        }
        best.1
    }

    fn nearest_city(&self, lat: f64, lon: f64) -> Option<&City> {
        let row = ((lat + 90.0).floor() as isize).clamp(0, 179);
        let col = (lon + 180.0).floor() as isize;
        // (거리 - 큰 도시 가산점, 실제 거리, 도시)
        let mut best: Option<(f64, f64, &City)> = None;
        // 둘레 1칸(약 55~111km)까지 보고, 없으면 3칸까지 넓힌다
        for radius in 0..=3isize {
            if radius >= 2 && best.is_some() {
                break;
            }
            for dr in -radius..=radius {
                for dc in -radius..=radius {
                    if dr.abs().max(dc.abs()) != radius {
                        continue;
                    }
                    let r = row + dr;
                    if !(0..180).contains(&r) {
                        continue;
                    }
                    let c = (col + dc).rem_euclid(360);
                    for &i in &self.city_grid[r as usize * 360 + c as usize] {
                        let city = &self.cities[i as usize];
                        let d = haversine(lat, lon, city.lat as f64, city.lon as f64);
                        let score = d - city.bonus as f64;
                        if best.is_none_or(|(bs, _, _)| score < bs) {
                            best = Some((score, d, city));
                        }
                    }
                }
            }
        }
        // 150km보다 멀면 (바다 한가운데 등) 모른다고 한다
        best.filter(|(_, d, _)| *d <= 150.0).map(|(_, _, c)| c)
    }
}

/// 좌표의 지명. 모르면 None.
pub fn lookup(lat: f64, lon: f64) -> Option<Place> {
    if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0 || (lat == 0.0 && lon == 0.0) {
        return None;
    }
    let db = db();
    if let Some(i) = db.korea(lat, lon) {
        let d = &db.dongs[i];
        let (sido, sgg) = db.sggs[d.sgg as usize];
        let (sido_name, sido_short) = db.sidos[sido as usize];
        return Some(Place {
            country: db.korea_idx.map(|k| db.s(db.countries[k as usize].1)).unwrap_or("대한민국"),
            korea: Some(Korea { sido: db.s(sido_name), sido_short: db.s(sido_short), sgg: db.s(sgg), dong: db.s(d.name) }),
            city: None,
        });
    }
    let city = db.nearest_city(lat, lon)?;
    Some(Place { country: db.s(db.countries[city.country as usize].1), korea: None, city: Some(db.s(city.name)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(lat: f64, lon: f64, level: Level) -> String {
        lookup(lat, lon).map(|p| p.label(level)).unwrap_or_else(|| "?".into())
    }

    #[test]
    fn korea() {
        // 서울시청
        assert_eq!(at(37.5663, 126.9779, Level::City), "서울 중구");
        assert_eq!(at(37.5663, 126.9779, Level::Province), "서울");
        assert_eq!(at(37.5663, 126.9779, Level::Country), "대한민국");
        // 제주 중문 관광단지
        assert_eq!(at(33.2489, 126.4106, Level::City), "제주 서귀포시");
        assert!(at(33.2489, 126.4106, Level::Town).starts_with("제주 서귀포시 "));
        // 수원 화성행궁 (일반구가 있는 시)
        assert_eq!(at(37.2818, 127.0145, Level::City), "경기 수원시 팔달구");
    }

    #[test]
    fn abroad() {
        assert_eq!(at(48.8584, 2.2945, Level::City), "프랑스 파리");
        assert_eq!(at(34.6873, 135.5262, Level::City), "일본 오사카");
        assert_eq!(at(35.6764, 139.6500, Level::Country), "일본");
        // 태평양 한가운데
        assert_eq!(at(0.0, -150.0, Level::City), "?");
    }

    #[test]
    fn speed() {
        warm();
        let t = std::time::Instant::now();
        for i in 0..10_000 {
            let f = i as f64 / 10_000.0;
            lookup(33.0 + f * 5.0, 126.0 + f * 3.0);
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / 10_000.0;
        assert!(us < 200.0, "한 번에 {us:.1}µs");
    }
}
