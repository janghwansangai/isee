//! 메뉴바. "보기 > 파일 종류"에서 체크한 종류만 목록과 썸네일에 포함한다 (요구사항 7).

use crate::AppState;
use crate::settings::Settings;
use isee_core::convert::{self, OutFormat};
use std::collections::HashMap;
use tauri::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// 설정과 연결된 체크 항목. 설정이 바뀌면 체크 상태를 맞춘다.
pub type Checks = HashMap<&'static str, CheckMenuItem<Wry>>;

const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Finder에서 보기"
} else {
    "탐색기에서 보기"
};

const SHELL_INSTALL: &str = if cfg!(target_os = "macos") {
    "Finder 우클릭(빠른 동작)에 iSee 변환 넣기"
} else {
    "탐색기 우클릭 메뉴에 iSee 변환 넣기"
};
const SHELL_UNINSTALL: &str = if cfg!(target_os = "macos") {
    "Finder 우클릭(빠른 동작)에서 iSee 변환 빼기"
} else {
    "탐색기 우클릭 메뉴에서 iSee 변환 빼기"
};

/// 맥에서만 메뉴 단축키를 건다. 윈도우는 웹뷰에 포커스가 있으면 메뉴 단축키가
/// 전달되지 않을 수 있어서 화면(JS)에서 직접 키를 처리한다.
fn mac_accel(a: &'static str) -> Option<&'static str> {
    cfg!(target_os = "macos").then_some(a)
}

pub fn build(app: &AppHandle, s: &Settings) -> tauri::Result<(Menu<Wry>, Checks)> {
    let sep = || PredefinedMenuItem::separator(app);
    let mut checks: Checks = HashMap::new();
    let mut check = |id: &'static str, label: &str, on: bool, accel: Option<&'static str>| -> tauri::Result<CheckMenuItem<Wry>> {
        let c = CheckMenuItem::with_id(app, id, label, true, on, accel)?;
        checks.insert(id, c.clone());
        Ok(c)
    };

    let kinds = Submenu::with_items(
        app,
        "파일 종류",
        true,
        &[
            &check("kind_image", "이미지", s.kinds.image, None)?,
            &check("kind_video", "동영상", s.kinds.video, None)?,
            &check("kind_pdf", "PDF", s.kinds.pdf, None)?,
            &check("kind_doc", "문서", s.kinds.doc, None)?,
            &check("kind_other", "기타 파일", s.kinds.other, None)?,
        ],
    )?;
    let sort = Submenu::with_items(
        app,
        "정렬",
        true,
        &[
            &check("sort_name", "이름", s.sort_by == "name", None)?,
            &check("sort_date", "수정한 날짜", s.sort_by == "date", None)?,
            &check("sort_size", "크기", s.sort_by == "size", None)?,
            &check("sort_type", "종류", s.sort_by == "type", None)?,
            &check("sort_rating", "별점", s.sort_by == "rating", None)?,
            &sep()?,
            &check("sort_desc", "내림차순", s.sort_desc, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "보기",
        true,
        &[
            &check("view_grid", "썸네일로 보기", s.view_mode != "list", None)?,
            &check("view_list", "목록으로 보기", s.view_mode == "list", None)?,
            &sep()?,
            &kinds,
            &sort,
            &sep()?,
            &MenuItem::with_id(app, "thumbs_bigger", "썸네일 크게", true, mac_accel("CmdOrCtrl+="))?,
            &MenuItem::with_id(app, "thumbs_smaller", "썸네일 작게", true, mac_accel("CmdOrCtrl+-"))?,
            &sep()?,
            &check("show_info", "정보 패널", s.show_info, None)?,
            &check("show_hidden", "숨김 파일 보기", s.show_hidden, None)?,
            &check("show_hud", "속도 표시", s.show_hud, None)?,
            &MenuItem::with_id(app, "bg_panel", "백그라운드 작업…", true, None::<&str>)?,
            &sep()?,
            &PredefinedMenuItem::fullscreen(app, Some("전체 화면"))?,
        ],
    )?;
    let open_folder = MenuItem::with_id(app, "open_folder", "폴더 열기…", true, mac_accel("CmdOrCtrl+O"))?;
    let new_folder = MenuItem::with_id(app, "new_folder", "새 폴더", true, mac_accel("CmdOrCtrl+Shift+N"))?;
    let reveal = MenuItem::with_id(app, "reveal", REVEAL_LABEL, true, None::<&str>)?;
    let rename = MenuItem::with_id(app, "rename", "이름 바꾸기", true, None::<&str>)?;
    let trash = MenuItem::with_id(app, "trash", "휴지통으로 이동", true, None::<&str>)?;
    let (sep1, sep2) = (sep()?, sep()?);
    let close = PredefinedMenuItem::close_window(app, Some("창 닫기"))?;
    // 윈도우는 앱 이름 메뉴가 없으므로 업데이트 확인을 파일 메뉴에 둔다
    #[cfg(not(target_os = "macos"))]
    let update = MenuItem::with_id(app, "check_update", "업데이트 확인…", true, None::<&str>)?;
    #[allow(unused_mut)]
    let mut file_items: Vec<&dyn tauri::menu::IsMenuItem<Wry>> =
        vec![&open_folder, &new_folder, &sep1, &reveal, &rename, &trash, &sep2];
    #[cfg(not(target_os = "macos"))]
    file_items.push(&update);
    file_items.push(&close);
    let file = Submenu::with_items(app, "파일", true, &file_items)?;
    let edit = Submenu::with_items(
        app,
        "편집",
        true,
        &[
            &MenuItem::with_id(app, "undo", "실행 취소", true, mac_accel("CmdOrCtrl+Z"))?,
            &sep()?,
            &PredefinedMenuItem::cut(app, Some("오려두기"))?,
            &PredefinedMenuItem::copy(app, Some("복사하기"))?,
            &PredefinedMenuItem::paste(app, Some("붙여넣기"))?,
            &MenuItem::with_id(app, "select_all", "모두 선택", true, mac_accel("CmdOrCtrl+A"))?,
        ],
    )?;
    let go = Submenu::with_items(
        app,
        "이동",
        true,
        &[
            &MenuItem::with_id(app, "go_back", "뒤로", true, mac_accel("CmdOrCtrl+["))?,
            &MenuItem::with_id(app, "go_forward", "앞으로", true, mac_accel("CmdOrCtrl+]"))?,
            &MenuItem::with_id(app, "go_up", "상위 폴더", true, mac_accel("CmdOrCtrl+Up"))?,
            &sep()?,
            &MenuItem::with_id(app, "go_home", "홈", true, None::<&str>)?,
            &MenuItem::with_id(app, "go_pictures", "사진", true, None::<&str>)?,
            &MenuItem::with_id(app, "go_downloads", "다운로드", true, None::<&str>)?,
        ],
    )?;
    let organize = Submenu::with_items(
        app,
        "정리",
        true,
        &[
            &MenuItem::with_id(app, "edit", "사진 편집…", true, Some("CmdOrCtrl+E"))?,
            &MenuItem::with_id(app, "compare", "선택한 사진 비교", true, None::<&str>)?,
            &check("culling", "빠른 고르기 모드 (P 남김 · X 버림 · 숫자 별점)", s.culling, None)?,
            &MenuItem::with_id(app, "trash_rejected", "버린 사진 모두 휴지통으로", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "similar", "비슷한 사진 정리…", true, Some("CmdOrCtrl+Shift+S"))?,
            &MenuItem::with_id(app, "organize", "날짜·장소별 정리…", true, Some("CmdOrCtrl+Shift+D"))?,
            &MenuItem::with_id(app, "people", "인물…", true, Some("CmdOrCtrl+Shift+P"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quick_folders", "빠른 이동 폴더 설정…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "shell_install", SHELL_INSTALL, true, None::<&str>)?,
            &MenuItem::with_id(app, "shell_uninstall", SHELL_UNINSTALL, true, None::<&str>)?,
        ],
    )?;

    let mut top: Vec<Submenu<Wry>> = Vec::new();
    #[cfg(target_os = "macos")]
    top.push(Submenu::with_items(
        app,
        "iSee",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("iSee 정보"), None)?,
            &MenuItem::with_id(app, "check_update", "업데이트 확인…", true, None::<&str>)?,
            &sep()?,
            &PredefinedMenuItem::hide(app, Some("iSee 가리기"))?,
            &PredefinedMenuItem::hide_others(app, Some("기타 가리기"))?,
            &PredefinedMenuItem::show_all(app, Some("모두 보기"))?,
            &sep()?,
            &PredefinedMenuItem::quit(app, Some("iSee 종료"))?,
        ],
    )?);
    top.extend([file, edit, view, go, organize]);
    #[cfg(target_os = "macos")]
    top.push(Submenu::with_items(
        app,
        "윈도우",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("최소화"))?,
            &PredefinedMenuItem::maximize(app, Some("확대/축소"))?,
        ],
    )?);
    let refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> =
        top.iter().map(|m| m as &dyn tauri::menu::IsMenuItem<Wry>).collect();
    Ok((Menu::with_items(app, &refs)?, checks))
}

fn format_label(f: OutFormat) -> &'static str {
    match f {
        OutFormat::Jpeg => "JPG",
        OutFormat::Png => "PNG",
        OutFormat::Webp => "WebP",
        OutFormat::Heic => "HEIC",
        OutFormat::Avif => "AVIF",
        OutFormat::Tiff => "TIFF",
        OutFormat::Bmp => "BMP",
    }
}

fn format_id(f: OutFormat) -> String {
    serde_json::to_value(f)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// 사진 우클릭 메뉴 (알씨 방식: 회전·크기·형식·용량·화질을 바로 저장).
/// images: 고른 것 중 사진 수, total: 전체 수.
pub fn context_menu(app: &AppHandle, s: &Settings, images: usize, total: usize) -> tauri::Result<Menu<Wry>> {
    let img = images > 0;
    let single = total == 1;
    let item = |id: &str, label: &str, enabled: bool| MenuItem::with_id(app, id, label, enabled, None::<&str>);
    let sep = || PredefinedMenuItem::separator(app);
    let many = if images > 1 { format!(" ({images}장)") } else { String::new() };

    let turn = Submenu::with_items(
        app,
        "회전",
        img,
        &[
            &item("ctx:turn:cw90", "시계 방향으로 90°   R", true)?,
            &item("ctx:turn:ccw90", "반시계 방향으로 90°   ⇧R", true)?,
            &item("ctx:turn:r180", "180°", true)?,
            &sep()?,
            &item("ctx:turn:flip_h", "좌우 뒤집기", true)?,
            &item("ctx:turn:flip_v", "상하 뒤집기", true)?,
        ],
    )?;
    let size = Submenu::with_items(
        app,
        "크기",
        img,
        &[
            &item("ctx:resize:pct:50", "50%", true)?,
            &item("ctx:resize:pct:25", "25%", true)?,
            &sep()?,
            &item("ctx:resize:long:3840", "긴 변 3840px (4K)", true)?,
            &item("ctx:resize:long:2560", "긴 변 2560px", true)?,
            &item("ctx:resize:long:1920", "긴 변 1920px (FHD)", true)?,
            &item("ctx:resize:long:1280", "긴 변 1280px", true)?,
            &item("ctx:resize:long:800", "긴 변 800px", true)?,
            &sep()?,
            &item("ctx:resize:custom", "직접 입력…", true)?,
        ],
    )?;
    let fmt_items: Vec<MenuItem<Wry>> = convert::available_formats()
        .into_iter()
        .map(|f| item(&format!("ctx:format:{}", format_id(f)), format_label(f), true))
        .collect::<tauri::Result<_>>()?;
    let fmt_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> =
        fmt_items.iter().map(|m| m as &dyn tauri::menu::IsMenuItem<Wry>).collect();
    let format = Submenu::with_items(app, "형식", img, &fmt_refs)?;
    let bytes = Submenu::with_items(
        app,
        "용량",
        img,
        &[
            &item("ctx:size:300000", "300KB 이하", true)?,
            &item("ctx:size:500000", "500KB 이하", true)?,
            &item("ctx:size:1000000", "1MB 이하", true)?,
            &item("ctx:size:2000000", "2MB 이하", true)?,
            &item("ctx:size:5000000", "5MB 이하", true)?,
            &sep()?,
            &item("ctx:size:custom", "직접 입력…", true)?,
        ],
    )?;
    let quality = Submenu::with_items(
        app,
        "화질",
        img,
        &[
            &item("ctx:quality:95", "95 (최고)", true)?,
            &item("ctx:quality:90", "90", true)?,
            &item("ctx:quality:80", "80", true)?,
            &item("ctx:quality:70", "70", true)?,
            &item("ctx:quality:60", "60 (작게)", true)?,
            &sep()?,
            &item("ctx:quality:custom", "직접 입력…", true)?,
        ],
    )?;
    let folder_label = match &s.convert_folder {
        Some(f) if s.convert_save == "folder" => format!(
            "지정한 폴더에 저장: {}",
            std::path::Path::new(f).file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
        ),
        _ => "지정한 폴더에 저장…".to_string(),
    };
    let save = Submenu::with_items(
        app,
        "저장 방식",
        true,
        &[
            &CheckMenuItem::with_id(app, "ctx:save:new", "새 파일로 저장 (원본 보존)", true, s.convert_save == "new", None::<&str>)?,
            &CheckMenuItem::with_id(app, "ctx:save:overwrite", "원본에 덮어쓰기 (되돌리기 가능)", true, s.convert_save == "overwrite", None::<&str>)?,
            &CheckMenuItem::with_id(app, "ctx:save:folder", &folder_label, true, s.convert_save == "folder", None::<&str>)?,
        ],
    )?;
    let quick_items: Vec<MenuItem<Wry>> = s
        .quick_folders
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let label = match q {
                Some(p) => format!(
                    "{}  {}",
                    i + 1,
                    std::path::Path::new(p).file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                ),
                None => format!("{}  (지정 안 됨)", i + 1),
            };
            item(&format!("ctx:quick:{}", i + 1), &label, q.is_some())
        })
        .collect::<tauri::Result<_>>()?;
    let quick_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> =
        quick_items.iter().map(|m| m as &dyn tauri::menu::IsMenuItem<Wry>).collect();
    let quick = Submenu::with_items(app, "빠른 이동", true, &quick_refs)?;

    let mark = Submenu::with_items(
        app,
        "표시",
        img,
        &[
            &item("ctx:flag:1", "남기기   P", true)?,
            &item("ctx:flag:-1", "버리기   X", true)?,
            &item("ctx:flag:0", "표시 지우기   U", true)?,
            &sep()?,
            &item("ctx:rate:5", "★★★★★", true)?,
            &item("ctx:rate:4", "★★★★", true)?,
            &item("ctx:rate:3", "★★★", true)?,
            &item("ctx:rate:2", "★★", true)?,
            &item("ctx:rate:1", "★", true)?,
            &item("ctx:rate:0", "별점 지우기", true)?,
        ],
    )?;

    Menu::with_items(
        app,
        &[
            &item("ctx:open", "열기", single)?,
            &item(
                "ctx:compare",
                &if images > 9 { "비교하기 (처음 9장)   C".to_string() } else { format!("비교하기{many}   C") },
                images >= 2,
            )?,
            &item("ctx:edit", "편집…   E", single && img)?,
            &mark,
            &sep()?,
            &turn,
            &size,
            &format,
            &bytes,
            &quality,
            &item("ctx:share", &format!("공유용으로 저장{many}  (2048px · JPG · 위치 정보 삭제)"), img)?,
            &item("ctx:watermark", "워터마크 넣기…", img)?,
            &item("ctx:batch_rename", "이름 일괄 변경…", total > 1)?,
            &save,
            &sep()?,
            &quick,
            &item("ctx:move_to", "다른 폴더로 이동…", true)?,
            &item("ctx:copy_to", "다른 폴더로 복사…", true)?,
            &item("ctx:organize", "날짜·장소별로 정리…", true)?,
            &sep()?,
            &item("ctx:rename", "이름 바꾸기", single)?,
            &item("ctx:reveal", REVEAL_LABEL, true)?,
            &item("ctx:copy_path", "경로 복사", true)?,
            &sep()?,
            &item(
                "ctx:trash",
                &if total > 1 { format!("휴지통으로 이동 ({total}개)") } else { "휴지통으로 이동".into() },
                true,
            )?,
        ],
    )
}

/// 설정 상태에 맞게 체크 표시를 맞춘다.
pub fn sync_checks(checks: &Checks, s: &Settings) {
    let set = |id: &str, on: bool| {
        if let Some(c) = checks.get(id) {
            let _ = c.set_checked(on);
        }
    };
    set("kind_image", s.kinds.image);
    set("kind_video", s.kinds.video);
    set("kind_pdf", s.kinds.pdf);
    set("kind_doc", s.kinds.doc);
    set("kind_other", s.kinds.other);
    for by in ["name", "date", "size", "type", "rating"] {
        set(&format!("sort_{by}"), s.sort_by == by);
    }
    set("sort_desc", s.sort_desc);
    set("view_grid", s.view_mode != "list");
    set("view_list", s.view_mode == "list");
    set("show_info", s.show_info);
    set("show_hidden", s.show_hidden);
    set("show_hud", s.show_hud);
    set("culling", s.culling);
}

/// 메뉴가 닫힌 뒤 웹뷰가 다시 클릭·키 입력을 받도록 포커스를 준다.
pub fn refocus_webview(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = AsRef::<tauri::Webview<Wry>>::as_ref(&w).set_focus();
    }
}

pub fn on_event(app: &AppHandle, ev: MenuEvent) {
    let id = ev.id().0.as_str();
    refocus_webview(app);
    let state = app.state::<AppState>();
    let changed = {
        let mut s = state.settings.lock();
        let before = serde_json::to_value(&*s).ok();
        match id {
            "kind_image" => s.kinds.image = !s.kinds.image,
            "kind_video" => s.kinds.video = !s.kinds.video,
            "kind_pdf" => s.kinds.pdf = !s.kinds.pdf,
            "kind_doc" => s.kinds.doc = !s.kinds.doc,
            "kind_other" => s.kinds.other = !s.kinds.other,
            "sort_name" => s.sort_by = "name".into(),
            "sort_date" => s.sort_by = "date".into(),
            "sort_size" => s.sort_by = "size".into(),
            "sort_type" => s.sort_by = "type".into(),
            "sort_rating" => s.sort_by = "rating".into(),
            "culling" => s.culling = !s.culling,
            "view_grid" => s.view_mode = "grid".into(),
            "view_list" => s.view_mode = "list".into(),
            "sort_desc" => s.sort_desc = !s.sort_desc,
            "show_info" => s.show_info = !s.show_info,
            "show_hidden" => s.show_hidden = !s.show_hidden,
            "show_hud" => s.show_hud = !s.show_hud,
            "ctx:save:new" => s.convert_save = "new".into(),
            "ctx:save:overwrite" => s.convert_save = "overwrite".into(),
            _ => {}
        }
        let after = serde_json::to_value(&*s).ok();
        (before != after).then(|| s.clone())
    };
    if let Some(s) = changed {
        s.save(&state.settings_path);
        sync_checks(&state.checks.lock(), &s);
        let _ = app.emit("settings-changed", &s);
    } else if id.starts_with("kind_") || id.starts_with("sort_") || id.starts_with("show_") || id.starts_with("view_") || id == "culling" {
        // 체크 항목을 눌렀지만 값이 그대로면 macOS가 바꾼 체크 표시를 되돌린다.
        sync_checks(&state.checks.lock(), &state.settings.lock());
    } else {
        let _ = app.emit("menu", id);
    }
}
