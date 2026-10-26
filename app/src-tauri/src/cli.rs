//! 화면 없이 변환만 하는 실행 방식. Finder 빠른 동작·윈도우 탐색기 우클릭 메뉴가 이것을 부른다.
//!   isee-app --convert rotate=cw  사진1.jpg 사진2.jpg …
//! 작업: rotate=cw|ccw|180, resize=1920 | resize=50%, format=jpeg|png|webp|heic|avif|tiff|bmp,
//!       size=1000000 (바이트 이하), quality=85, share (공유용)

use crate::settings::Settings;
use isee_core::convert::{self, OutFormat, Resize, SaveMode, Spec, Turn};
use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Tauri의 app_config_dir과 같은 위치 (앱과 같은 설정을 읽기 위해)
fn config_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/com.isee.viewer")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home).join("com.isee.viewer")
    } else {
        home().join(".config/com.isee.viewer")
    }
}

fn cache_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Caches/com.isee.viewer")
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(home).join("com.isee.viewer")
    } else {
        home().join(".cache/com.isee.viewer")
    }
}

/// "rotate=cw" 같은 작업 문자열을 변환 설정으로
pub fn parse_op(op: &str) -> Option<Spec> {
    let (k, v) = op.split_once('=').unwrap_or((op, ""));
    let mut spec = Spec::default();
    match k {
        "rotate" => {
            spec.turn = Some(match v {
                "cw" => Turn::Cw90,
                "ccw" => Turn::Ccw90,
                "180" => Turn::R180,
                "flip_h" => Turn::FlipH,
                "flip_v" => Turn::FlipV,
                _ => return None,
            })
        }
        "resize" => {
            spec.resize = Some(match v.strip_suffix('%') {
                Some(p) => Resize::Percent(p.parse().ok()?),
                None => Resize::LongSide(v.parse().ok()?),
            })
        }
        "format" => spec.format = serde_json::from_value(serde_json::Value::String(v.into())).ok(),
        "size" => spec.max_bytes = Some(v.parse().ok()?),
        "quality" => spec.quality = Some(v.parse().ok()?),
        "share" => {
            spec.resize = Some(Resize::LongSide(2048));
            spec.format = Some(OutFormat::Jpeg);
            spec.quality = Some(85);
            spec.strip_gps = true;
            spec.suffix = Some("공유".into());
        }
        _ => return None,
    }
    Some(spec)
}

/// --convert로 실행됐으면 변환하고 종료 코드를 돌려준다. 아니면 None (평소처럼 창을 띄움).
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == "--convert")?;
    let Some(mut spec) = args.get(i + 1).and_then(|op| parse_op(op)) else {
        eprintln!("사용법: isee-app --convert <작업> <파일…>");
        return Some(2);
    };
    let files: Vec<String> = args[i + 2..].to_vec();
    let settings = Settings::load(&config_dir().join("settings.json"));
    let folder = settings.convert_folder.clone().filter(|_| settings.convert_save == "folder");
    spec.save = if spec.turn.is_some() {
        SaveMode::Overwrite // 회전은 제자리 (JPEG·HEIC는 무손실)
    } else if let Some(f) = folder {
        SaveMode::Folder(f)
    } else if settings.convert_save == "overwrite" && spec.suffix.is_none() {
        SaveMode::Overwrite
    } else {
        SaveMode::NewFile
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let backup = cache_dir().join("undo").join(format!("cli-{stamp}"));
    let results = convert::run_batch(&files, &spec, Some(&backup), &|_, _| {});
    let mut code = 0;
    for r in results {
        match r {
            Ok(d) => println!("{} → {} ({} bytes)", d.source, d.output, d.bytes),
            Err(f) => {
                eprintln!("{}: {}", f.path, f.error);
                code = 1;
            }
        }
    }
    Some(code)
}
