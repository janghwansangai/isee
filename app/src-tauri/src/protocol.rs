//! isee:// 주소로 썸네일과 뷰어 이미지를 웹뷰에 보낸다.
//!   /thumb?p=경로&s=크기&m=수정시각&i=목록위치   → JPEG/PNG 썸네일
//!   /view?p=경로&w=상자가로&h=상자세로(0=원본)    → 압축 없는 RGBA (헤더에 가로·세로)
//!   /file?p=경로                                → 파일 그대로, 범위 요청 지원 (동영상 재생)
//! IPC(JSON)를 거치지 않으므로 큰 바이트도 복사 한 번으로 간다.

use crate::AppState;
use isee_core::thumbs::Req;
use isee_core::viewer::Payload;
use percent_encoding::percent_decode_str;
use std::collections::HashMap;
use tauri::http::{Request, Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder, Wry};

fn query(req: &Request<Vec<u8>>) -> HashMap<String, String> {
    req.uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((
                k.to_string(),
                percent_decode_str(&v.replace('+', "%20"))
                    .decode_utf8_lossy()
                    .into_owned(),
            ))
        })
        .collect()
}

fn not_found() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("Access-Control-Allow-Origin", "*")
        .body(Vec::new())
        .unwrap()
}

pub fn handle(ctx: UriSchemeContext<'_, Wry>, req: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let state = ctx.app_handle().state::<AppState>();
    let q = query(&req);
    let num = |k: &str| q.get(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let path = q.get("p").cloned().or_else(|| (req.uri().path() == "/face").then(String::new));
    let Some(path) = path else {
        responder.respond(not_found());
        return;
    };
    match req.uri().path() {
        "/thumb" => {
            let r = Req {
                path,
                size: num("s") as u64,
                mtime: num("m"),
                index: num("i"),
            };
            state.thumbs.request(
                r,
                Box::new(move |t| {
                    let Some(t) = t else {
                        responder.respond(not_found());
                        return;
                    };
                    responder.respond(
                        Response::builder()
                            .header("Content-Type", t.mime)
                            .header("Access-Control-Allow-Origin", "*")
                            // 주소에 크기·수정시각이 들어 있어서 파일이 바뀌면 주소도 바뀐다.
                            .header("Cache-Control", "max-age=31536000, immutable")
                            .body(t.bytes.clone())
                            .unwrap(),
                    );
                }),
            );
        }
        "/view" => {
            let (w, h) = (num("w") as u32, num("h") as u32);
            state.remember_view_box(w, h);
            state.viewer.request(
                path,
                w,
                h,
                Box::new(move |v| {
                    let Some(v) = v else {
                        responder.respond(not_found());
                        return;
                    };
                    let b = Response::builder()
                        .header("Access-Control-Allow-Origin", "*")
                        .header(
                            "Access-Control-Expose-Headers",
                            "x-w, x-h, x-fw, x-fh, x-ms, x-route",
                        )
                        .header("Cache-Control", "no-store")
                        .header("x-fw", v.full_width.to_string())
                        .header("x-fh", v.full_height.to_string())
                        .header("x-ms", format!("{:.1}", v.prep_ms))
                        .header("x-route", v.route());
                    let resp = match &v.payload {
                        Payload::Pixels(p) => b
                            .header("Content-Type", "application/octet-stream")
                            .header("x-w", p.width.to_string())
                            .header("x-h", p.height.to_string())
                            .body(p.rgba.clone()),
                        Payload::Original { bytes, mime } => b
                            .header("Content-Type", *mime)
                            .header("x-w", v.full_width.to_string())
                            .header("x-h", v.full_height.to_string())
                            .body(bytes.clone()),
                    };
                    responder.respond(resp.unwrap());
                }),
            );
        }
        "/face" => {
            // 저장된 썸네일을 찾는 것도 화면 스레드 밖에서. 없으면 얼굴 썸네일 일꾼(2개)이 차례로 만든다.
            let id = num("id");
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                app.state::<AppState>().face_crops.request(
                    id,
                    Box::new(move |b| {
                        responder.respond(match b {
                            Some(b) => Response::builder()
                                .header("Content-Type", "image/jpeg")
                                .header("Access-Control-Allow-Origin", "*")
                                .header("Cache-Control", "max-age=3600")
                                .body(b)
                                .unwrap(),
                            None => not_found(),
                        })
                    }),
                );
            });
        }
        "/file" => {
            let range = req.headers().get("range").and_then(|v| v.to_str().ok()).map(str::to_string);
            std::thread::spawn(move || responder.respond(file_range(&path, range.as_deref())));
        }
        _ => responder.respond(not_found()),
    }
}

/// 동영상은 웹뷰가 조금씩 범위를 바꿔 가며 요청한다. 한 번에 최대 8MB씩 읽어 준다.
fn file_range(path: &str, range: Option<&str>) -> Response<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 8 << 20;
    let Ok(mut f) = std::fs::File::open(path) else { return not_found() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mime = match std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    };
    // "bytes=시작-끝" (끝은 없을 수 있음)
    let (start, end) = range
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.split(',').next())
        .and_then(|r| {
            let (a, b) = r.split_once('-')?;
            let a: u64 = a.trim().parse().ok()?;
            let b: u64 = b.trim().parse().unwrap_or(len.saturating_sub(1));
            Some((a, b.min(len.saturating_sub(1))))
        })
        .unwrap_or((0, len.saturating_sub(1)));
    if len == 0 || start > end {
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header("Content-Range", format!("bytes */{len}"))
            .body(Vec::new())
            .unwrap();
    }
    let end = end.min(start + CHUNK - 1);
    let mut buf = vec![0u8; (end - start + 1) as usize];
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() {
        return not_found();
    }
    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header("Content-Type", mime)
        .header("Accept-Ranges", "bytes")
        .header("Content-Range", format!("bytes {start}-{end}/{len}"))
        .header("Content-Length", buf.len().to_string())
        .header("Access-Control-Allow-Origin", "*")
        .body(buf)
        .unwrap()
}
