//! 얼굴 찾기·얼굴 지문 (8단계, 선택 모듈).
//!   - 얼굴 찾기: YuNet (OpenCV Zoo, MIT) — 640×640 입력, 얼굴 상자 + 눈·코·입 5점
//!   - 얼굴 지문: SFace (OpenCV Zoo, Apache-2.0) — 5점으로 112×112에 맞춰 자른 얼굴 → 128개 숫자
//! 모델은 앱에 들어 있지 않고, 사용자가 "얼굴 분류 기능 설치"를 눌렀을 때 내려받는다.
//! 실행은 순수 Rust ONNX 엔진(tract)으로 한다 (윈도우·맥 같은 코드, 별도 런타임 없음).

use crate::decode::Pixels;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tract_onnx::prelude::*;

/// 내려받을 모델 (주소, 파일 이름, SHA-256, 크기)
pub const MODELS: [(&str, &str, &str, u64); 2] = [
    (
        "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
        "face_detection_yunet_2023mar.onnx",
        "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
        232_589,
    ),
    (
        "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx",
        "face_recognition_sface_2021dec.onnx",
        "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
        38_696_353,
    ),
];

const DET_SIZE: usize = 640;
const REC_SIZE: usize = 112;
/// 얼굴로 볼 최소 점수
const SCORE_MIN: f32 = 0.75;
const NMS_IOU: f32 = 0.3;

/// SFace(ArcFace 계열)가 학습한 얼굴 위치: 오른눈·왼눈·코·오른입꼬리·왼입꼬리 (112×112)
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Face {
    /// 사진 안 위치 (0~1 비율): x, y, 가로, 세로
    pub rect: [f32; 4],
    /// 눈·코·입 5점 (픽셀)
    pub marks: [[f32; 2]; 5],
    pub score: f32,
}

type Plan = std::sync::Arc<TypedRunnableModel>;

pub struct Models {
    det: Plan,
    rec: Plan,
}

/// 모델 파일이 다 있고 내용이 맞는가 (크기로 빠르게 확인)
pub fn installed(dir: &Path) -> bool {
    MODELS.iter().all(|(_, name, _, size)| std::fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == *size))
}

impl Models {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let load = |name: &str, shape: [usize; 4]| -> anyhow::Result<Plan> {
            let p = dir.join(name);
            let plan = tract_onnx::onnx()
                .model_for_path(&p)
                .with_context(|| format!("모델을 읽을 수 없음: {}", p.display()))?
                .with_input_fact(0, f32::fact(shape).into())?
                .into_optimized()?
                .into_runnable()?;
            Ok(plan)
        };
        Ok(Self {
            det: load(MODELS[0].1, [1, 3, DET_SIZE, DET_SIZE])?,
            rec: load(MODELS[1].1, [1, 3, REC_SIZE, REC_SIZE])?,
        })
    }

    /// 사진(회전 적용된 RGBA)에서 얼굴 찾기.
    ///   1) 사진 전체를 640에 맞춰 한 번
    ///   2) 단체 사진처럼 작은 얼굴이 있거나 하나도 못 찾았으면, 2배로 확대한 조각(25% 겹침)으로 다시 찾는다
    ///   3) 그래도 없으면 사진을 320으로 작게 넣어 한 번 더 (화면을 가득 채운 큰 얼굴용)
    pub fn detect(&self, px: &Pixels) -> anyhow::Result<Vec<Face>> {
        let (w, h) = (px.width as f32, px.height as f32);
        let mut found = self.detect_region(px, [0.0, 0.0, w, h], DET_SIZE)?;
        let long = w.max(h);
        // 전체 그림(640)에서 48px보다 작게 보인 얼굴 → 확대해서 다시 찾을 만하다
        let small = |f: &Face| (f.rect[2] * w).max(f.rect[3] * h) / long * (DET_SIZE as f32) < 48.0;
        if long >= 1400.0 && (found.is_empty() || found.iter().any(small)) {
            let tile = long / 2.0;
            let step = tile * 0.75;
            let mut y = 0.0;
            while y < h {
                let mut x = 0.0;
                while x < w {
                    let r = [x, y, tile.min(w - x), tile.min(h - y)];
                    if r[2] > tile * 0.3 && r[3] > tile * 0.3 {
                        found.extend(self.detect_region(px, r, DET_SIZE)?);
                    }
                    if x + tile >= w {
                        break;
                    }
                    x += step;
                }
                if y + tile >= h {
                    break;
                }
                y += step;
            }
        }
        if found.is_empty() {
            found = self.detect_region(px, [0.0, 0.0, w, h], DET_SIZE / 2)?;
        }
        // 겹치는 상자(조각 경계에서 두 번 찾은 얼굴 등)는 점수가 높은 것만
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut keep: Vec<Face> = Vec::new();
        for f in found {
            if keep.iter().all(|k| iou(&k.rect, &f.rect) < NMS_IOU && !inside(&f.rect, &k.rect)) {
                keep.push(f);
            }
        }
        Ok(keep)
    }

    /// 사진의 한 영역 [x, y, 가로, 세로](픽셀)을 긴 변 long으로 줄여 640×640 왼쪽 위에 넣고 얼굴을 찾는다
    fn detect_region(&self, px: &Pixels, region: [f32; 4], long: usize) -> anyhow::Result<Vec<Face>> {
        let (w, h) = (px.width as f32, px.height as f32);
        let [rx, ry, rw, rh] = region;
        let scale = long as f32 / rw.max(rh);
        let (sw, sh) = (((rw * scale).round() as usize).clamp(1, DET_SIZE), ((rh * scale).round() as usize).clamp(1, DET_SIZE));
        // BGR, 0~255. 줄일 때는 칸 평균(상자 필터)에 가깝게 쌍선형 샘플링
        let mut input = Tensor::zero::<f32>(&[1, 3, DET_SIZE, DET_SIZE])?;
        {
            let mut v = input.to_plain_array_view_mut::<f32>()?;
            for y in 0..sh {
                let sy = (ry + (y as f32 + 0.5) / scale).min(h - 1.0);
                for x in 0..sw {
                    let sx = (rx + (x as f32 + 0.5) / scale).min(w - 1.0);
                    let p = sample(px, sx, sy);
                    v[[0, 0, y, x]] = p[2];
                    v[[0, 1, y, x]] = p[1];
                    v[[0, 2, y, x]] = p[0];
                }
            }
        }
        let out = self.det.run(tvec!(input.into()))?;
        let view = |i: usize| -> anyhow::Result<Vec<f32>> { Ok(out[i].to_plain_array_view::<f32>()?.iter().copied().collect()) };
        let mut found = Vec::new();
        for (k, stride) in [8usize, 16, 32].into_iter().enumerate() {
            let (cls, obj, bbox, kps) = (&view(k)?, &view(k + 3)?, &view(k + 6)?, &view(k + 9)?);
            let cols = DET_SIZE / stride;
            for idx in 0..cls.len() {
                let score = (cls[idx].clamp(0.0, 1.0) * obj[idx].clamp(0.0, 1.0)).sqrt();
                if score < SCORE_MIN {
                    continue;
                }
                let (r, c) = ((idx / cols) as f32, (idx % cols) as f32);
                let s = stride as f32;
                let cx = (c + bbox[idx * 4]) * s;
                let cy = (r + bbox[idx * 4 + 1]) * s;
                let bw = bbox[idx * 4 + 2].exp() * s;
                let bh = bbox[idx * 4 + 3].exp() * s;
                // 조각 밖(채운 0 부분)에서 찾은 것은 버린다
                if cx > sw as f32 || cy > sh as f32 {
                    continue;
                }
                let mut marks = [[0f32; 2]; 5];
                for (n, m) in marks.iter_mut().enumerate() {
                    *m = [rx + (kps[idx * 10 + 2 * n] + c) * s / scale, ry + (kps[idx * 10 + 2 * n + 1] + r) * s / scale];
                }
                let (x0, y0) = (rx + (cx - bw / 2.0) / scale, ry + (cy - bh / 2.0) / scale);
                found.push(Face {
                    rect: [x0 / w, y0 / h, bw / scale / w, bh / scale / h],
                    marks,
                    score,
                });
            }
        }
        Ok(found)
    }

    /// 얼굴 지문 (128개, 길이 1로 맞춤)과 품질 (정규화 전 길이: 흐리거나 가려진 얼굴일수록 작다).
    /// 정렬한 얼굴과 좌우로 뒤집은 얼굴의 지문을 더해 쓴다 (안정성이 올라간다).
    pub fn fingerprint(&self, px: &Pixels, face: &Face) -> anyhow::Result<(Vec<f32>, f32)> {
        let m = similarity(&face.marks, &TEMPLATE);
        // 112×112의 각 점이 사진의 어디에서 왔는지: 역변환으로 샘플링 (RGB, 0~255)
        let inv = invert(&m);
        let mut aligned = vec![[0f32; 3]; REC_SIZE * REC_SIZE];
        for y in 0..REC_SIZE {
            for x in 0..REC_SIZE {
                let (fx, fy) = (x as f32, y as f32);
                aligned[y * REC_SIZE + x] = sample(px, inv[0] * fx + inv[1] * fy + inv[2], inv[3] * fx + inv[4] * fy + inv[5]);
            }
        }
        let mut sum = vec![0f32; 128];
        let mut norms = 0f32;
        for flip in [false, true] {
            let mut input = Tensor::zero::<f32>(&[1, 3, REC_SIZE, REC_SIZE])?;
            {
                let mut v = input.to_plain_array_view_mut::<f32>()?;
                for y in 0..REC_SIZE {
                    for x in 0..REC_SIZE {
                        let sx = if flip { REC_SIZE - 1 - x } else { x };
                        let p = aligned[y * REC_SIZE + sx];
                        for c in 0..3 {
                            v[[0, c, y, x]] = p[c];
                        }
                    }
                }
            }
            let out = self.rec.run(tvec!(input.into()))?;
            let f: Vec<f32> = out[0].to_plain_array_view::<f32>()?.iter().copied().collect();
            norms += f.iter().map(|v| v * v).sum::<f32>().sqrt();
            if sum.len() != f.len() {
                sum = vec![0f32; f.len()];
            }
            for (a, b) in sum.iter_mut().zip(&f) {
                *a += b;
            }
        }
        let n = sum.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
        sum.iter_mut().for_each(|v| *v /= n);
        Ok((sum, norms / 2.0))
    }
}

/// a가 b 안에 거의 들어가 있는가 (조각에서 얼굴 일부만 찾은 경우를 지우기 위해)
fn inside(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let ix = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let iy = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    ix > 0.0 && iy > 0.0 && ix * iy > 0.7 * a[2] * a[3]
}

/// 두 지문의 비슷함 (코사인, -1~1). SFace 기준 0.363 이상이면 같은 사람일 가능성이 높다.
pub fn similarity_of(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let iy = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    let inter = ix * iy;
    inter / (a[2] * a[3] + b[2] * b[3] - inter)
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

/// src 점들을 dst 점들로 보내는 닮음 변환(회전·확대·이동) [a, -b, tx, b, a, ty] (최소제곱, Umeyama)
fn similarity(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [f32; 6] {
    let n = 5.0;
    let mean = |p: &[[f32; 2]; 5]| [p.iter().map(|v| v[0]).sum::<f32>() / n, p.iter().map(|v| v[1]).sum::<f32>() / n];
    let (ms, md) = (mean(src), mean(dst));
    let (mut sxx, mut sxy, mut var) = (0f32, 0f32, 0f32);
    for i in 0..5 {
        let (sx, sy) = (src[i][0] - ms[0], src[i][1] - ms[1]);
        let (dx, dy) = (dst[i][0] - md[0], dst[i][1] - md[1]);
        // a = Σ(s·d)/Σ|s|², b = Σ(s×d)/Σ|s|²
        sxx += sx * dx + sy * dy;
        sxy += sx * dy - sy * dx;
        var += sx * sx + sy * sy;
    }
    let (a, b) = (sxx / var.max(1e-6), sxy / var.max(1e-6));
    [a, -b, md[0] - (a * ms[0] - b * ms[1]), b, a, md[1] - (b * ms[0] + a * ms[1])]
}

fn invert(m: &[f32; 6]) -> [f32; 6] {
    let det = m[0] * m[4] - m[1] * m[3];
    let (a, b, c, d) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
    [a, b, -(a * m[2] + b * m[5]), c, d, -(c * m[2] + d * m[5])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_maps_template_to_itself_and_back() {
        // 템플릿을 2배 키우고 30° 돌리고 옮긴 점들 → 템플릿으로 정확히 돌아와야 한다
        let (s, t) = (2.0f32, 30f32.to_radians());
        let src = TEMPLATE.map(|[x, y]| [s * (x * t.cos() - y * t.sin()) + 100.0, s * (x * t.sin() + y * t.cos()) + 50.0]);
        let m = similarity(&src, &TEMPLATE);
        for i in 0..5 {
            let (x, y) = (src[i][0], src[i][1]);
            let (u, v) = (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
            assert!((u - TEMPLATE[i][0]).abs() < 1e-3 && (v - TEMPLATE[i][1]).abs() < 1e-3, "{i}: {u},{v}");
        }
        let inv = invert(&m);
        let (x, y) = (TEMPLATE[2][0], TEMPLATE[2][1]);
        let (u, v) = (inv[0] * x + inv[1] * y + inv[2], inv[3] * x + inv[4] * y + inv[5]);
        assert!((u - src[2][0]).abs() < 1e-2 && (v - src[2][1]).abs() < 1e-2);
    }
}
