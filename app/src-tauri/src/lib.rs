mod cli;
mod menu;
mod protocol;
mod settings;
mod shell;

use isee_core::analysis::{AnalysisService, FeatureStore};
use isee_core::cache::ThumbCache;
use isee_core::convert;
use isee_core::rename;
use isee_core::similar;
use isee_core::formats::Kind;
use isee_core::fsops::{self, Moved, Trashed};
use isee_core::info::Info;
use isee_core::listing::{self, DirNode, Entry, Place};
use isee_core::marks::{Mark, Marks};
use isee_core::organize;
use isee_core::people::{self, FaceDb, FaceService};
use isee_core::thumbs::{Req, ThumbService};
use isee_core::viewer::ViewerService;
use parking_lot::Mutex;
use serde::Serialize;
use settings::Settings;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

/// 되돌리기 기록
enum UndoOp {
    Trash(Vec<Trashed>),
    Move(Vec<Moved>),
    Copy(Vec<String>),
    Rename(Moved),
    Created(String),
    Convert(Vec<convert::Done>),
    RenameMany(Vec<Moved>),
    Marks(Vec<(String, Mark)>),
    Organize(organize::Applied),
}

pub struct AppState {
    pub thumbs: ThumbService,
    pub viewer: ViewerService,
    /// 비슷한 사진 정리용 특징값 계산 (4단계)
    pub analysis: AnalysisService,
    /// 뷰어·비교 화면이 열려 있으면 분석을 멈춘다 (보기 속도 우선)
    analysis_paused: Arc<AtomicBool>,
    /// 뷰어·비교·편집 화면이 열려 있음
    viewer_active: AtomicBool,
    /// 사용자가 "백그라운드 작업 모두 멈춤"을 누름
    user_paused: AtomicBool,
    /// 지금(또는 마지막으로) 분석하는 범위 이름 (백그라운드 작업 창에 보여 준다)
    similar_label: Mutex<String>,
    faces_label: Mutex<String>,
    /// 얼굴별 분류 (8단계, 선택 모듈)
    pub faces: FaceService,
    models_dir: PathBuf,
    /// 인물 화면 얼굴 썸네일 (저장해 둔 것, 없으면 사진을 몇 장씩만 읽어 만든다)
    pub face_crops: people::CropService,
    /// 인물 묶기는 한 번에 하나만 (얼굴이 많을 때 겹쳐 계산하면 메모리가 늘어난다)
    report_lock: Mutex<()>,
    /// 날짜·장소별 정리: 마지막으로 읽은 사진들 (규칙을 바꿀 때 다시 읽지 않게)
    organize_items: Mutex<Vec<organize::Item>>,
    /// 남김·버림·별점
    pub marks: Marks,
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub checks: Mutex<menu::Checks>,
    undo: Mutex<Vec<UndoOp>>,
    pending_open: Mutex<Option<String>>,
    started: Instant,
    started_unix_ms: f64,
    setup_ms: f64,
    /// 덮어쓰기 전 원본을 보관하는 곳 (되돌리기용)
    undo_dir: PathBuf,
    /// 띄운 우클릭 메뉴 (닫힐 때까지 살아 있어야 함)
    ctx_menu: Mutex<Option<tauri::menu::Menu<tauri::Wry>>>,
}

impl AppState {
    /// 앱이 파일을 옮기거나 이름을 바꿨을 때: 표시와 얼굴 정보가 따라간다
    fn path_moved(&self, from: &str, to: &str) {
        self.marks.rename(from, to);
        self.faces.db().rename(from, to);
    }

    /// 뷰어 화면 크기를 기억한다 (바뀌었을 때만 저장).
    pub fn remember_view_box(&self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let mut s = self.settings.lock();
        if s.view_box != (w, h) {
            s.view_box = (w, h);
            s.save(&self.settings_path);
        }
    }

    /// 사진을 열라는 요청이 오면, 화면이 폴더를 여는 동안 엔진이 먼저 디코딩을 시작한다.
    fn prewarm(&self, path: &str) {
        let (w, h) = self.settings.lock().view_box;
        if w > 0 && isee_core::formats::kind_of(Path::new(path)) == Kind::Image {
            self.viewer.request(path.to_string(), w, h, Box::new(|_| {}));
        }
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ───────────────────────── 폴더 ─────────────────────────

#[tauri::command]
fn places() -> Vec<Place> {
    listing::places()
}

#[tauri::command]
async fn subdirs(state: State<'_, AppState>, path: String) -> CmdResult<Vec<DirNode>> {
    let hidden = state.settings.lock().show_hidden;
    listing::list_subdirs(Path::new(&path), hidden).map_err(err)
}

#[derive(Serialize)]
struct Listing {
    path: String,
    parent: Option<String>,
    entries: Vec<Entry>,
    /// 이 폴더 사진들의 남김·버림·별점 (표시가 있는 것만, 보기 필터와 무관하게 폴더 전체)
    marks: std::collections::HashMap<String, Mark>,
    /// 보기 필터와 무관한 폴더 전체 사진 수 (빠른 고르기 개수 표시용)
    image_total: usize,
    /// 하위 폴더·여러 폴더를 함께 볼 때 읽은 폴더 수 (폴더 하나면 1)
    dirs: usize,
    /// 파일이 너무 많아 한도까지만 모았다
    truncated: bool,
    ms: f64,
}

/// 지금 보는 범위: 폴더 하나(기본), 그 하위 폴더까지, 다른 폴더를 더해 함께.
/// 하위 폴더·여러 폴더는 사진이 많아질 수 있어 사용자가 켤 때만 쓴다.
#[derive(Clone, Debug, Default)]
struct Scope {
    path: String,
    extra: Vec<String>,
    recursive: bool,
}

impl Scope {
    /// 경로는 NFC로 맞춘다 (대화상자로 고른 폴더는 NFD로 올 수 있다. listing::nfc 참고)
    fn new(path: String, extra: Option<Vec<String>>, recursive: Option<bool>) -> Self {
        let path = listing::nfc_string(path);
        let mut extra: Vec<String> = extra.unwrap_or_default().into_iter().map(listing::nfc_string).collect();
        extra.retain(|p| p != &path);
        extra.dedup();
        Self { path, extra, recursive: recursive.unwrap_or(false) }
    }
    /// 폴더 하나만 (예전과 같은 빠른 길)
    fn single(&self) -> bool {
        self.extra.is_empty() && !self.recursive
    }
    fn roots(&self) -> Vec<std::path::PathBuf> {
        std::iter::once(&self.path).chain(&self.extra).map(std::path::PathBuf::from).collect()
    }
}

/// 범위 안의 항목: (항목, 읽은 폴더 수, 한도에 걸림). 여러 폴더면 폴더 항목은 빼고 파일만.
fn scope_entries(sc: &Scope, hidden: bool) -> CmdResult<(Vec<Entry>, usize, bool)> {
    if sc.single() {
        return Ok((listing::list_dir(Path::new(&sc.path), hidden).map_err(err)?, 1, false));
    }
    let t = listing::list_tree(&sc.roots(), sc.recursive, hidden, listing::TreeLimits::default());
    if t.dirs == 0 {
        return Err(format!("폴더를 읽을 수 없습니다: {}", sc.path));
    }
    Ok((t.files, t.dirs, t.truncated))
}

/// 범위를 사람이 읽는 이름으로: "1026세계박람회 (하위 폴더 포함) 외 2곳"
fn scope_label(sc: &Scope) -> String {
    let mut s = Path::new(&sc.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| sc.path.clone());
    if sc.recursive {
        s.push_str(" (하위 폴더 포함)");
    }
    if !sc.extra.is_empty() {
        s.push_str(&format!(" 외 {}곳", sc.extra.len()));
    }
    s
}

/// 범위 안의 남김·버림·별점 (폴더마다 하위 폴더 포함으로 읽는다)
fn scope_marks(state: &AppState, sc: &Scope) -> std::collections::HashMap<String, Mark> {
    let mut m = std::collections::HashMap::new();
    for root in sc.roots() {
        m.extend(state.marks.in_dir(&root));
    }
    m
}

fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 폴더를 열고(설정의 파일 종류·정렬 적용), 그 순서대로 썸네일을 미리 만들기 시작한다.
#[tauri::command]
async fn open_folder(
    state: State<'_, AppState>,
    path: String,
    extra: Option<Vec<String>>,
    recursive: Option<bool>,
) -> CmdResult<Listing> {
    let t0 = Instant::now();
    let s = state.settings.lock().clone();
    let sc = Scope::new(path, extra, recursive);
    let path = sc.path.clone();
    let dir = Path::new(&path);
    let (mut entries, dirs, truncated) = scope_entries(&sc, s.show_hidden)?;
    let mut marks = scope_marks(&state, &sc);
    // 밖에서 지우거나 옮긴 파일의 표시는 빼고 넘긴다
    let images: std::collections::HashSet<&str> =
        entries.iter().filter(|e| e.kind == Kind::Image).map(|e| e.path.as_str()).collect();
    marks.retain(|p, _| images.contains(p.as_str()));
    let image_total = images.len();
    drop(images);
    let mark = |p: &str| marks.get(p).copied().unwrap_or_default();
    // 빠른 고르기 모드의 보기 필터: 남김 / 미정 / 버림 / 별점 있는 것
    let cull = if s.culling { s.cull_filter.as_str() } else { "all" };
    entries.retain(|e| {
        let kind_ok = match e.kind {
            Kind::Dir => true,
            Kind::Image => s.kinds.image,
            Kind::Video => s.kinds.video,
            Kind::Pdf => s.kinds.pdf,
            Kind::Doc => s.kinds.doc,
            Kind::Other => s.kinds.other,
        };
        let m = mark(&e.path);
        let cull_ok = e.kind == Kind::Dir
            || match cull {
                "picked" => m.flag == 1,
                "unflagged" => m.flag == 0,
                "rejected" => m.flag == -1,
                "rated" => m.rating > 0,
                _ => true,
            };
        kind_ok && cull_ok
    });
    // list_dir가 이미 "폴더 먼저 + 이름 자연 정렬"이므로 그 순서를 같은 값끼리의 기준으로 쓴다.
    let by = s.sort_by.as_str();
    // 여러 폴더를 모았으면 폴더마다의 순서가 이어 붙어 있으므로 항상 다시 정렬한다
    if by != "name" || s.sort_desc || !sc.single() {
        entries.sort_by(|a, b| {
            let dirs = (a.kind != Kind::Dir).cmp(&(b.kind != Kind::Dir));
            let key = match by {
                "date" => a.mtime.cmp(&b.mtime),
                "size" => a.size.cmp(&b.size),
                "type" => ext_of(&a.name).cmp(&ext_of(&b.name)),
                "rating" => mark(&a.path).rating.cmp(&mark(&b.path).rating),
                _ => std::cmp::Ordering::Equal,
            };
            let key = if key == std::cmp::Ordering::Equal {
                natord_cmp(&a.name, &b.name)
            } else {
                key
            };
            dirs.then(if s.sort_desc { key.reverse() } else { key })
        });
    }
    // 동영상·PDF도 보이게 켰으면 OS 썸네일 엔진으로 미리 만든다
    let reqs: Vec<Req> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, Kind::Image | Kind::Video | Kind::Pdf))
        .map(|(i, e)| Req {
            path: e.path.clone(),
            size: e.size,
            mtime: e.mtime,
            index: i as i64,
        })
        .collect();
    state.thumbs.set_folder(reqs);
    {
        let mut st = state.settings.lock();
        if st.last_folder.as_deref() != Some(path.as_str()) {
            st.last_folder = Some(path.clone());
            st.save(&state.settings_path);
        }
    }
    Ok(Listing {
        parent: dir.parent().map(|p| p.to_string_lossy().into_owned()),
        path,
        entries,
        marks,
        image_total,
        dirs,
        truncated,
        ms: t0.elapsed().as_secs_f64() * 1000.0,
    })
}

fn natord_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    // 대소문자 무시 자연 정렬 (isee-core의 정렬 규칙과 같게)
    let (la, lb) = (a.to_lowercase(), b.to_lowercase());
    let (mut ia, mut ib) = (la.chars().peekable(), lb.chars().peekable());
    loop {
        match (ia.peek().copied(), ib.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ia.peek().copied().filter(|c| c.is_ascii_digit()) {
                    na.push(c);
                    ia.next();
                }
                let mut nb = String::new();
                while let Some(c) = ib.peek().copied().filter(|c| c.is_ascii_digit()) {
                    nb.push(c);
                    ib.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then(ta.cmp(tb));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(ca), Some(cb)) => {
                if ca != cb {
                    return ca.cmp(&cb);
                }
                ia.next();
                ib.next();
            }
        }
    }
}

#[tauri::command]
fn set_viewport(state: State<'_, AppState>, start: i64, end: i64) {
    state.thumbs.set_viewport(start, end);
}

#[tauri::command]
fn set_viewer_active(state: State<'_, AppState>, active: bool) {
    state.thumbs.set_viewer_active(active);
    state.viewer_active.store(active, Ordering::Relaxed);
    state.analysis_paused.store(active || state.user_paused.load(Ordering::Relaxed), Ordering::Relaxed);
}

#[tauri::command]
async fn file_info(path: String) -> CmdResult<Info> {
    isee_core::info::file_info(Path::new(&path)).map_err(err)
}

// ───────────────────────── 파일 작업 ─────────────────────────

#[derive(Serialize)]
struct Failure {
    path: String,
    error: String,
}

#[derive(Serialize)]
struct OpResult {
    /// 성공한 항목 (이동·복사는 새 경로)
    ok: Vec<String>,
    failed: Vec<Failure>,
}

#[tauri::command]
async fn trash(state: State<'_, AppState>, paths: Vec<String>) -> CmdResult<OpResult> {
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for p in paths {
        match fsops::trash(Path::new(&p)) {
            Ok(t) => {
                state.viewer.forget(&p);
                done.push(t);
            }
            Err(e) => failed.push(Failure { path: p, error: e.to_string() }),
        }
    }
    let ok = done.iter().map(|t| t.original.clone()).collect();
    if !done.is_empty() {
        state.undo.lock().push(UndoOp::Trash(done));
    }
    Ok(OpResult { ok, failed })
}

#[tauri::command]
async fn transfer(
    state: State<'_, AppState>,
    paths: Vec<String>,
    dest: String,
    copy: bool,
) -> CmdResult<OpResult> {
    let mut moved = Vec::new();
    let mut failed = Vec::new();
    for p in paths {
        match fsops::transfer(Path::new(&p), Path::new(&dest), copy) {
            Ok(m) => {
                if !copy {
                    state.viewer.forget(&p);
                    state.path_moved(&m.from, &m.to);
                }
                moved.push(m);
            }
            Err(e) => failed.push(Failure { path: p, error: e.to_string() }),
        }
    }
    let ok = moved.iter().map(|m| m.to.clone()).collect();
    if !moved.is_empty() {
        state.undo.lock().push(if copy {
            UndoOp::Copy(moved.into_iter().map(|m| m.to).collect())
        } else {
            UndoOp::Move(moved)
        });
    }
    Ok(OpResult { ok, failed })
}

#[tauri::command]
async fn rename(state: State<'_, AppState>, path: String, name: String) -> CmdResult<String> {
    let m = fsops::rename(Path::new(&path), &name).map_err(err)?;
    state.viewer.forget(&path);
    state.path_moved(&m.from, &m.to);
    let to = m.to.clone();
    state.undo.lock().push(UndoOp::Rename(m));
    Ok(to)
}

#[tauri::command]
async fn create_folder(state: State<'_, AppState>, parent: String, name: String) -> CmdResult<String> {
    let p = fsops::create_dir(Path::new(&parent), &name).map_err(err)?;
    state.undo.lock().push(UndoOp::Created(p.clone()));
    Ok(p)
}

#[derive(Serialize)]
struct UndoInfo {
    message: String,
}

#[tauri::command]
async fn undo(state: State<'_, AppState>) -> CmdResult<Option<UndoInfo>> {
    let Some(op) = state.undo.lock().pop() else {
        return Ok(None);
    };
    let message = match op {
        UndoOp::Trash(items) => {
            fsops::restore(&items).map_err(err)?;
            format!("{}개를 휴지통에서 되돌렸습니다", items.len())
        }
        UndoOp::Move(items) => {
            for m in items.iter().rev() {
                fsops::undo_move(m).map_err(err)?;
                state.path_moved(&m.to, &m.from);
            }
            format!("{}개 이동을 되돌렸습니다", items.len())
        }
        UndoOp::Copy(paths) => {
            for p in &paths {
                fsops::trash(Path::new(p)).map_err(err)?;
            }
            format!("복사한 {}개를 휴지통으로 보냈습니다", paths.len())
        }
        UndoOp::Rename(m) => {
            fsops::undo_move(&m).map_err(err)?;
            state.path_moved(&m.to, &m.from);
            "이름 바꾸기를 되돌렸습니다".to_string()
        }
        UndoOp::Created(p) => {
            fsops::trash(Path::new(&p)).map_err(err)?;
            "새 폴더 만들기를 되돌렸습니다".to_string()
        }
        UndoOp::Convert(done) => {
            for d in done.iter().rev() {
                convert::undo(d).map_err(err)?;
                state.viewer.forget(&d.source);
                state.viewer.forget(&d.output);
            }
            format!("변환 {}개를 되돌렸습니다", done.len())
        }
        UndoOp::RenameMany(done) => {
            rename::undo(&done).map_err(err)?;
            for m in &done {
                state.path_moved(&m.to, &m.from);
            }
            format!("이름 {}개를 되돌렸습니다", done.len())
        }
        UndoOp::Organize(a) => {
            organize::undo(&a).map_err(err)?;
            for m in &a.moved {
                state.path_moved(&m.to, &m.from);
            }
            let n = a.moved.len() + a.copied.len();
            format!("날짜·장소별 정리 {n}개를 되돌렸습니다")
        }
        UndoOp::Marks(old) => {
            state.marks.restore(&old);
            format!("표시 {}개를 되돌렸습니다", old.len())
        }
    };
    Ok(Some(UndoInfo { message }))
}

#[tauri::command]
fn reveal(path: String) -> CmdResult<()> {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(&path).spawn();
    #[cfg(windows)]
    let r = std::process::Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let r = std::process::Command::new("xdg-open")
        .arg(Path::new(&path).parent().unwrap_or(Path::new("/")))
        .spawn();
    r.map(|_| ()).map_err(err)
}

// ───────────────────────── 표시 (3단계) ─────────────────────────

/// 남김(1)·버림(-1)·표시 없음(0), 별점(0~5). 넘기지 않은 값은 그대로 둔다.
#[tauri::command]
async fn set_marks(
    state: State<'_, AppState>,
    paths: Vec<String>,
    flag: Option<i8>,
    rating: Option<u8>,
) -> CmdResult<std::collections::HashMap<String, Mark>> {
    let old = state.marks.set(&paths, flag, rating);
    state.undo.lock().push(UndoOp::Marks(old));
    Ok(paths.into_iter().map(|p| {
        let m = state.marks.get(&p);
        (p, m)
    }).collect())
}

// ───────────────────────── 비슷한 사진 (4단계) ─────────────────────────

/// 범위 안 사진 전부 (보기 필터와 무관, 숨김 파일 설정만 따른다)
fn scope_images(state: &AppState, sc: &Scope) -> CmdResult<Vec<Req>> {
    let hidden = state.settings.lock().show_hidden;
    let (entries, _, _) = scope_entries(sc, hidden)?;
    Ok(entries
        .into_iter()
        .filter(|e| e.kind == Kind::Image)
        .enumerate()
        .map(|(i, e)| Req { path: e.path, size: e.size, mtime: e.mtime, index: i as i64 })
        .collect())
}

/// 폴더 사진의 특징값 계산을 백그라운드로 시작한다 (계산해 둔 것은 건너뜀).
/// (끝난 수, 전체)를 돌려주고, 이후 진행은 "analysis-progress" 이벤트로 알린다.
#[tauri::command]
async fn analyze_folder(app: AppHandle, path: String, extra: Option<Vec<String>>, recursive: Option<bool>) -> CmdResult<(usize, usize)> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let sc = Scope::new(path, extra, recursive);
        let reqs = scope_images(&state, &sc)?;
        *state.similar_label.lock() = scope_label(&sc);
        Ok(state.analysis.analyze(reqs))
    })
    .await
    .map_err(err)?
}

/// 분석에 걸릴 시간 어림 (시작하지 않음). 사진이 많으면 시작 전에 사용자에게 묻는다.
#[derive(Serialize)]
struct AnalysisEstimate {
    total: usize,
    /// 비슷한 사진: 새로 분석할 수, 클라우드라 넘길 수, 어림 시간
    similar_todo: usize,
    cloud: usize,
    similar_ms: f64,
    /// 얼굴(설치했을 때만): 새로 분석할 수, 어림 시간
    faces_todo: usize,
    faces_ms: f64,
    /// 클라우드 드라이브 안의 폴더면 그 이름 (예: "구글 드라이브")
    cloud_drive: Option<&'static str>,
}

#[tauri::command]
async fn analysis_estimate(app: AppHandle, path: String, extra: Option<Vec<String>>, recursive: Option<bool>) -> CmdResult<AnalysisEstimate> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let sc = Scope::new(path, extra, recursive);
        let reqs = scope_images(&state, &sc)?;
        let cloud_drive = sc.roots().iter().find_map(|r| listing::cloud_drive(r));
        let (similar_todo, cloud) = state.analysis.pending(&reqs);
        let faces_todo = if isee_core::faces::installed(&state.models_dir) {
            reqs.iter().filter(|r| !state.faces.db().analyzed(r) && !listing::is_online_only(Path::new(&r.path))).count()
        } else {
            0
        };
        // 한 장 시간(12MP 기준, 측정값에 여유) ÷ 일꾼 수 (분석은 코어 절반, 얼굴은 4분의 1)
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let sim_workers = (cores / 2).clamp(1, 6) as f64;
        let face_workers = (cores / 4).clamp(1, 3) as f64;
        Ok(AnalysisEstimate {
            total: reqs.len(),
            similar_todo,
            cloud,
            similar_ms: similar_todo as f64 * 30.0 / sim_workers,
            faces_todo,
            faces_ms: faces_todo as f64 * 150.0 / face_workers,
            cloud_drive,
        })
    })
    .await
    .map_err(err)?
}

/// 비슷한 사진 분석 중단 (백그라운드 작업 창의 "중단")
#[tauri::command]
fn stop_analysis(state: State<'_, AppState>) {
    state.analysis.analyze(Vec::new());
}

/// 분석이 끝난 사진으로 묶음·추천·중복·흐림 보고서를 만든다. 고른 기준은 설정에 기억한다.
#[tauri::command]
async fn similar_report(
    app: AppHandle,
    path: String,
    options: similar::Options,
    extra: Option<Vec<String>>,
    recursive: Option<bool>,
) -> CmdResult<similar::Report> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let mut s = state.settings.lock();
            if s.similar != options {
                s.similar = options.clone();
                s.save(&state.settings_path);
            }
        }
        let sc = Scope::new(path, extra, recursive);
        let reqs = scope_images(&state, &sc)?;
        let total = reqs.len();
        let feats = state.analysis.features(&reqs);
        let marks = scope_marks(&state, &sc);
        let mut cloud = 0;
        let items: Vec<similar::Item> = reqs
            .into_iter()
            .zip(feats)
            .filter_map(|(r, f)| {
                if f.is_none() && listing::is_online_only(Path::new(&r.path)) {
                    cloud += 1;
                }
                let m = marks.get(&r.path).copied().unwrap_or_default();
                Some(similar::Item { path: r.path, size: r.size, mtime: r.mtime, flag: m.flag, rating: m.rating, f: f? })
            })
            .collect();
        let mut report = similar::report(&items, total, &options);
        report.cloud = cloud;
        Ok(report)
    })
    .await
    .map_err(err)?
}

// ───────────────────────── 날짜·장소별 정리 (5단계) ─────────────────────────

#[derive(Serialize)]
struct OrganizeScan {
    photos: usize,
    videos: usize,
    ms: f64,
}

/// 폴더 안 사진·동영상 (하위 폴더 포함 가능, 숨김 파일 설정 따름)
fn media_in(dir: &Path, hidden: bool, recursive: bool, out: &mut Vec<String>) {
    let Ok(entries) = listing::list_dir(dir, hidden) else { return };
    for e in entries {
        match e.kind {
            Kind::Image | Kind::Video => out.push(e.path),
            Kind::Dir if recursive => media_in(Path::new(&e.path), hidden, true, out),
            _ => {}
        }
    }
}

/// 정리할 사진의 날짜·위치를 읽어 둔다. paths가 비면 folder 전체. 진행은 "organize-progress"로.
#[tauri::command]
async fn organize_scan(
    app: AppHandle,
    paths: Vec<String>,
    folder: String,
    recursive: bool,
) -> CmdResult<OrganizeScan> {
    tauri::async_runtime::spawn_blocking(move || {
        let t0 = Instant::now();
        // 지명 데이터는 읽는 동안 같이 준비한다
        std::thread::spawn(isee_core::places::warm);
        let state = app.state::<AppState>();
        let mut files = Vec::new();
        let hidden = state.settings.lock().show_hidden;
        if paths.is_empty() {
            media_in(Path::new(&folder), hidden, recursive, &mut files);
        } else {
            for p in paths {
                let path = Path::new(&p);
                if path.is_dir() {
                    media_in(path, hidden, recursive, &mut files);
                } else {
                    files.push(p);
                }
            }
        }
        let handle = app.clone();
        let items = organize::scan(&files, &move |done, total| {
            let _ = handle.emit("organize-progress", (done, total));
        });
        let videos = items.iter().filter(|i| isee_core::formats::kind_of(Path::new(&i.path)) == Kind::Video).count();
        let r = OrganizeScan { photos: items.len() - videos, videos, ms: t0.elapsed().as_secs_f64() * 1000.0 };
        *state.organize_items.lock() = items;
        r
    })
    .await
    .map_err(err)
}

/// 규칙대로 어느 폴더에 몇 장이 갈지 (파일은 건드리지 않음). 고른 규칙은 설정에 기억한다.
#[tauri::command]
async fn organize_plan(app: AppHandle, options: organize::Options) -> CmdResult<organize::Plan> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let mut s = state.settings.lock();
            if s.organize != options {
                s.organize = options.clone();
                s.save(&state.settings_path);
            }
        }
        let items = state.organize_items.lock();
        organize::plan(&items, &options)
    })
    .await
    .map_err(err)
}

#[derive(Serialize)]
struct OrganizeReport {
    files: usize,
    folders: usize,
    unchanged: usize,
    failed: Vec<Failure>,
    ms: f64,
}

/// 정리 실행: renames(계획 폴더 → 바꾼 이름), skip(뺄 폴더)을 반영해 root 아래로 옮기거나 복사한다.
/// 전체를 ⌘Z 한 번으로 되돌릴 수 있다. 진행은 "organize-progress"로.
#[tauri::command]
async fn organize_apply(
    app: AppHandle,
    root: String,
    options: organize::Options,
    renames: std::collections::HashMap<String, String>,
    skip: Vec<String>,
    copy: bool,
) -> CmdResult<OrganizeReport> {
    tauri::async_runtime::spawn_blocking(move || {
        let t0 = Instant::now();
        let state = app.state::<AppState>();
        let plan = organize::plan(&state.organize_items.lock(), &options);
        let folders: Vec<organize::ApplyFolder> = plan
            .folders
            .into_iter()
            .filter(|f| !skip.contains(&f.rel))
            .map(|f| organize::ApplyFolder {
                rel: renames.get(&f.rel).cloned().unwrap_or(f.rel),
                items: f
                    .items
                    .into_iter()
                    .map(|i| organize::ApplyItem { path: i.path, companions: i.companions })
                    .collect(),
            })
            .collect();
        let handle = app.clone();
        let a = organize::apply(Path::new(&root), &folders, copy, &move |done, total| {
            let _ = handle.emit("organize-progress", (done, total));
        });
        for m in &a.moved {
            state.viewer.forget(&m.from);
            state.path_moved(&m.from, &m.to);
        }
        let report = OrganizeReport {
            files: a.moved.len() + a.copied.len(),
            folders: a.created.len(),
            unchanged: a.unchanged,
            failed: a.failed.iter().map(|(p, e)| Failure { path: p.clone(), error: e.clone() }).collect(),
            ms: t0.elapsed().as_secs_f64() * 1000.0,
        };
        if report.files > 0 || !a.created.is_empty() {
            state.undo.lock().push(UndoOp::Organize(a));
        }
        report
    })
    .await
    .map_err(err)
}

// ───────────────────────── 인물 (8단계) ─────────────────────────

#[derive(Serialize)]
struct FacesStatus {
    installed: bool,
    /// 내려받을 크기 (바이트)
    download: u64,
    analyzing: (usize, usize),
}

#[tauri::command]
fn faces_status(state: State<'_, AppState>) -> FacesStatus {
    FacesStatus {
        installed: isee_core::faces::installed(&state.models_dir),
        download: isee_core::faces::MODELS.iter().map(|m| m.3).sum(),
        analyzing: state.faces.progress(),
    }
}

/// 얼굴 모델 내려받기 (사용자가 설치를 눌렀을 때만). 진행은 "faces-install" (받은 바이트, 전체).
/// 받은 파일은 SHA-256으로 확인하고, 맞을 때만 제자리에 둔다.
#[tauri::command]
async fn faces_install(app: AppHandle) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};
        let state = app.state::<AppState>();
        let dir = state.models_dir.clone();
        std::fs::create_dir_all(&dir)?;
        let total: u64 = isee_core::faces::MODELS.iter().map(|m| m.3).sum();
        let mut got = 0u64;
        let mut last = Instant::now();
        for (url, name, sha, size) in isee_core::faces::MODELS {
            let dest = dir.join(name);
            if std::fs::metadata(&dest).is_ok_and(|m| m.len() == size) {
                got += size;
                continue;
            }
            let resp = ureq::get(url).timeout(std::time::Duration::from_secs(60)).call()?;
            let mut reader = resp.into_reader();
            let tmp = dir.join(format!("{name}.part"));
            let mut f = std::fs::File::create(&tmp)?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 256 << 10];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                f.write_all(&buf[..n])?;
                hasher.update(&buf[..n]);
                got += n as u64;
                if last.elapsed().as_millis() > 100 {
                    last = Instant::now();
                    let _ = app.emit("faces-install", (got, total));
                }
            }
            drop(f);
            let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if digest != sha {
                let _ = std::fs::remove_file(&tmp);
                anyhow::bail!("내려받은 모델이 손상되었습니다 ({name})");
            }
            std::fs::rename(&tmp, &dest)?;
        }
        let _ = app.emit("faces-install", (total, total));
        state.faces.load_models(&dir)?;
        Ok(())
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// 폴더(하위 폴더 포함 가능)의 사진

/// 얼굴 찾기를 백그라운드로 시작 (분석한 사진은 건너뜀). 진행은 "faces-progress".
#[tauri::command]
async fn faces_analyze(app: AppHandle, folder: String, recursive: bool, extra: Option<Vec<String>>) -> CmdResult<(usize, usize)> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if !isee_core::faces::installed(&state.models_dir) {
            return Err("얼굴 분류 기능이 설치되지 않았습니다".to_string());
        }
        state.faces.load_models(&state.models_dir).map_err(err)?;
        let sc = Scope::new(folder, extra, Some(recursive));
        let reqs = scope_images(&state, &sc)?;
        *state.faces_label.lock() = scope_label(&sc);
        Ok(state.faces.analyze(reqs))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn faces_stop(state: State<'_, AppState>) {
    state.faces.analyze(Vec::new());
}

/// 설정의 묶는 방식 (정확도·옆모습)
fn face_options(state: &AppState) -> people::Options {
    let s = state.settings.lock();
    people::Options { accuracy: s.face_accuracy, profile: s.face_profile }
}

/// 얼굴 방향을 모르는(0.1.6 전에 분석한) 사진을 다시 분석하도록 되돌린다. 화면이 이어서 분석을 시작한다.
#[tauri::command]
async fn faces_redo_turn(app: AppHandle, folder: String, recursive: bool, extra: Option<Vec<String>>) -> CmdResult<usize> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let reqs = scope_images(&state, &Scope::new(folder, extra, Some(recursive)))?;
        let paths: Vec<String> = reqs.into_iter().map(|r| r.path).collect();
        Ok(state.faces.db().forget_without_turn(&paths))
    })
    .await
    .map_err(err)?
}

/// 인물 화면을 닫으면 아직 만들지 못한 얼굴 썸네일 요청은 버린다
#[tauri::command]
fn faces_crops_cancel(state: State<'_, AppState>) {
    state.face_crops.cancel();
}

/// 사람별 묶음 (이 폴더의 얼굴 + 이름 붙은 사람 전체)
#[tauri::command]
async fn faces_report(app: AppHandle, folder: String, recursive: bool, extra: Option<Vec<String>>) -> CmdResult<people::Report> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _one = state.report_lock.lock();
        let reqs = scope_images(&state, &Scope::new(folder, extra, Some(recursive))).unwrap_or_default();
        let total = reqs.len();
        let db = state.faces.db();
        let analyzed = reqs.iter().filter(|r| db.analyzed(r)).count();
        let paths: Vec<String> = reqs.into_iter().map(|r| r.path).collect();
        people::group(db.faces_of(&paths), &db.people(), &db.apart_pairs(), analyzed, total, &face_options(&state))
    })
    .await
    .map_err(err)
}

/// 얼굴들에 이름 붙이기: person이 있으면 그 사람으로, 없으면 새 사람. 이름이 있으면 바꾼다.
#[tauri::command]
fn faces_name(state: State<'_, AppState>, faces: Vec<i64>, person: Option<i64>, name: Option<String>) -> CmdResult<i64> {
    let db = state.faces.db();
    let id = match person {
        Some(p) => {
            if let Some(n) = name.as_deref().filter(|n| !n.trim().is_empty()) {
                db.rename_person(p, n).map_err(err)?;
            }
            p
        }
        None => db.create_person(name.as_deref().unwrap_or("이름 없음")).map_err(err)?,
    };
    db.assign(&faces, id).map_err(err)?;
    Ok(id)
}

/// "이 사람 아님"
#[tauri::command]
fn faces_reject(state: State<'_, AppState>, faces: Vec<i64>, person: i64) -> CmdResult<()> {
    state.faces.db().reject(&faces, person).map_err(err)
}

/// 이름 없는 묶음에서 "빼기": 고른 얼굴을 남은 얼굴과 다시 묶지 않는다
#[tauri::command]
fn faces_apart(state: State<'_, AppState>, faces: Vec<i64>, others: Vec<i64>) -> CmdResult<()> {
    state.faces.db().keep_apart(&faces, &others).map_err(err)
}

#[tauri::command]
fn faces_delete_person(state: State<'_, AppState>, person: i64) -> CmdResult<()> {
    state.faces.db().delete_person(person).map_err(err)
}

/// 얼굴 데이터와 이름 전부 지우기 (모델은 남긴다)
#[tauri::command]
fn faces_clear(state: State<'_, AppState>) -> CmdResult<()> {
    state.faces.analyze(Vec::new());
    state.face_crops.cancel();
    state.faces.db().clear().map_err(err)
}

/// 인물별 폴더로: "copy"(인물마다 복사, 여러 사람이 나온 사진은 사람 수만큼) 또는 "main"(주인공 기준 이동, 비슷한 크기로 여럿이면 "단체 사진")
#[tauri::command]
async fn faces_organize(
    app: AppHandle,
    folder: String,
    recursive: bool,
    root: String,
    mode: String,
    people_ids: Vec<i64>,
    extra: Option<Vec<String>>,
) -> CmdResult<OrganizeReport> {
    tauri::async_runtime::spawn_blocking(move || {
        let t0 = Instant::now();
        let state = app.state::<AppState>();
        let reqs = scope_images(&state, &Scope::new(folder, extra, Some(recursive))).unwrap_or_default();
        let db = state.faces.db();
        let paths: Vec<String> = reqs.into_iter().map(|r| r.path).collect();
        let report = people::group(db.faces_of(&paths), &db.people(), &db.apart_pairs(), 0, 0, &face_options(&state));
        // 얼굴 → 사람 (자동으로 넣은 것 포함), 사람 → 이름
        let mut person_of: HashMap<i64, i64> = HashMap::new();
        let mut names: HashMap<i64, String> = HashMap::new();
        for g in &report.groups {
            if let (Some(p), Some(n)) = (g.person, &g.name) {
                names.insert(p, n.clone());
                for f in &g.faces {
                    person_of.insert(f.id, p);
                }
            }
        }
        let mut by_folder: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        let wanted = |p: &i64| people_ids.is_empty() || people_ids.contains(p);
        if mode == "copy" {
            for g in &report.groups {
                let (Some(p), Some(n)) = (g.person, &g.name) else { continue };
                if !wanted(&p) {
                    continue;
                }
                let mut photos: Vec<String> = g.faces.iter().map(|f| f.path.clone()).collect();
                photos.sort();
                photos.dedup();
                by_folder.entry(n.clone()).or_default().extend(photos);
            }
        } else {
            let all = db.faces_of(&paths);
            let mut per_photo: HashMap<&str, Vec<&people::FaceRow>> = HashMap::new();
            for f in &all {
                per_photo.entry(f.path.as_str()).or_default().push(f);
            }
            for (path, faces) in per_photo {
                let dest = match people::main_person(&faces, &|f| person_of.get(&f.id).copied()) {
                    Ok(Some(p)) if wanted(&p) => names.get(&p).cloned(),
                    Ok(_) => None,
                    Err(()) => Some("단체 사진".to_string()),
                };
                if let Some(d) = dest {
                    by_folder.entry(d).or_default().push(path.to_string());
                }
            }
        }
        let folders: Vec<organize::ApplyFolder> = by_folder
            .into_iter()
            .map(|(rel, items)| organize::ApplyFolder {
                rel,
                items: items.into_iter().map(|path| organize::ApplyItem { path, companions: Vec::new() }).collect(),
            })
            .collect();
        let copy = mode == "copy";
        let a = organize::apply(Path::new(&root), &folders, copy, &|_, _| {});
        for m in &a.moved {
            state.viewer.forget(&m.from);
            state.path_moved(&m.from, &m.to);
        }
        let report = OrganizeReport {
            files: a.moved.len() + a.copied.len(),
            folders: a.created.len(),
            unchanged: a.unchanged,
            failed: a.failed.iter().map(|(p, e)| Failure { path: p.clone(), error: e.clone() }).collect(),
            ms: t0.elapsed().as_secs_f64() * 1000.0,
        };
        if report.files > 0 || !a.created.is_empty() {
            state.undo.lock().push(UndoOp::Organize(a));
        }
        report
    })
    .await
    .map_err(err)
}

// ───────────────────────── 편집기 (6단계) ─────────────────────────

#[derive(serde::Deserialize)]
struct EditHead {
    path: String,
    width: u32,
    height: u32,
    spec: convert::Spec,
}

/// 편집기 저장. 본문은 편집 결과 RGBA(원본 해상도), 머리글 x-edit에 {path, width, height, spec} (URL 인코딩 JSON).
/// 변환과 같은 저장 방식·되돌리기를 쓰고, 원본의 촬영 정보는 이어받는다.
#[tauri::command]
async fn save_edit(app: AppHandle, request: tauri::ipc::Request<'_>) -> CmdResult<convert::Done> {
    let head = request.headers().get("x-edit").and_then(|v| v.to_str().ok()).ok_or("편집 정보 없음")?;
    let head = percent_encoding::percent_decode_str(head).decode_utf8().map_err(err)?;
    let head: EditHead = serde_json::from_str(&head).map_err(err)?;
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("픽셀이 없음".into());
    };
    if bytes.len() != head.width as usize * head.height as usize * 4 {
        return Err(format!("픽셀 크기가 맞지 않음 ({}×{}, {}바이트)", head.width, head.height, bytes.len()));
    }
    let px = isee_core::decode::Pixels { rgba: bytes.clone(), width: head.width, height: head.height };
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let backup = state.undo_dir.join(stamp.to_string());
        let d = convert::run_pixels(Path::new(&head.path), px, &head.spec, Some(&backup)).map_err(err)?;
        state.viewer.forget(&d.source);
        state.viewer.forget(&d.output);
        state.undo.lock().push(UndoOp::Convert(vec![d.clone()]));
        Ok(d)
    })
    .await
    .map_err(err)?
}

// ───────────────────────── 빠른 변환 (2단계) ─────────────────────────

/// 사진 우클릭 메뉴를 커서 위치에 띄운다. 고른 항목은 "menu" 이벤트로 화면에 전달된다.
#[tauri::command]
fn context_menu(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, AppState>,
    images: usize,
    total: usize,
) -> CmdResult<()> {
    let s = state.settings.lock().clone();
    let menu = menu::context_menu(&app, &s, images, total).map_err(err)?;
    window.popup_menu(&menu).map_err(err)?;
    *state.ctx_menu.lock() = Some(menu);
    // OS 메뉴가 닫히면 웹뷰가 포커스를 잃어 다음 클릭 한 번이 무시된다 → 메뉴가 닫힌 뒤 포커스를 돌려준다
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || menu::refocus_webview(&handle));
    Ok(())
}

#[derive(Serialize)]
struct ConvertReport {
    done: Vec<convert::Done>,
    failed: Vec<convert::Failed>,
    ms: f64,
}

/// 여러 장 변환. 진행 상황은 "convert-progress" 이벤트 [끝난 수, 전체]로 알린다.
#[tauri::command]
async fn convert(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    spec: convert::Spec,
) -> CmdResult<ConvertReport> {
    let t0 = Instant::now();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let backup = state.undo_dir.join(stamp.to_string());
    let emitter = app.clone();
    let results = tauri::async_runtime::spawn_blocking(move || {
        convert::run_batch(&paths, &spec, Some(&backup), &|d, t| {
            let _ = emitter.emit("convert-progress", (d, t));
        })
    })
    .await
    .map_err(err)?;
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for r in results {
        match r {
            Ok(d) => {
                state.viewer.forget(&d.source);
                state.viewer.forget(&d.output);
                done.push(d);
            }
            Err(f) => failed.push(f),
        }
    }
    if !done.is_empty() {
        state.undo.lock().push(UndoOp::Convert(done.clone()));
    }
    Ok(ConvertReport { done, failed, ms: t0.elapsed().as_secs_f64() * 1000.0 })
}

#[tauri::command]
async fn rename_plan(paths: Vec<String>, rule: rename::Rule) -> Vec<rename::Planned> {
    rename::plan(&paths, &rule)
}

#[tauri::command]
async fn rename_apply(state: State<'_, AppState>, items: Vec<rename::Planned>) -> CmdResult<usize> {
    let items: Vec<rename::Planned> = items
        .into_iter()
        .filter(|it| Path::new(&it.from).file_name().map(|n| n.to_string_lossy() != it.name).unwrap_or(true))
        .collect();
    let done = rename::apply(&items).map_err(err)?;
    for m in &done {
        state.viewer.forget(&m.from);
        state.path_moved(&m.from, &m.to);
    }
    let n = done.len();
    if n > 0 {
        state.undo.lock().push(UndoOp::RenameMany(done));
    }
    Ok(n)
}

#[derive(Serialize)]
struct ShellStatus {
    installed: bool,
    count: usize,
}

/// Finder 빠른 동작(맥)·탐색기 우클릭 메뉴(윈도우)에 넣거나 뺀다.
#[tauri::command]
async fn shell_integration(install: Option<bool>) -> CmdResult<ShellStatus> {
    let count = match install {
        Some(true) => shell::install().map_err(err)?,
        Some(false) => shell::uninstall().map_err(err)?,
        None => 0,
    };
    Ok(ShellStatus { installed: shell::installed(), count })
}

/// 오래된 되돌리기 보관본을 지운다 (최근 20번만 남김).
fn prune_undo_dir(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subs: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
    subs.sort();
    let extra = subs.len().saturating_sub(20);
    for p in subs.into_iter().take(extra) {
        let _ = std::fs::remove_dir_all(p);
    }
}

/// 만든 사람 홈페이지 (메뉴 "홈페이지", 정보 창)
pub const HOMEPAGE: &str = "https://hwansang.kr";
/// 프로그램 오류 기록 (앱 데이터 폴더)
const ERROR_LOG: &str = "errors.log";

/// 홈페이지를 기본 브라우저로 연다 (주소는 고정, 화면에서 다른 주소를 넘길 수 없다)
#[tauri::command]
fn open_homepage() -> CmdResult<()> {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(HOMEPAGE).spawn();
    #[cfg(windows)]
    let r = std::process::Command::new("explorer").arg(HOMEPAGE).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let r = std::process::Command::new("xdg-open").arg(HOMEPAGE).spawn();
    r.map(|_| ()).map_err(err)
}

#[derive(Serialize)]
struct AppInfo {
    version: String,
    homepage: &'static str,
    /// 오류 기록 파일 (있을 때만)
    error_log: Option<String>,
}

#[tauri::command]
fn app_info(app: AppHandle) -> AppInfo {
    let log = app.path().app_data_dir().ok().map(|d| d.join(ERROR_LOG)).filter(|p| p.exists());
    AppInfo {
        version: app.package_info().version.to_string(),
        homepage: HOMEPAGE,
        error_log: log.map(|p| p.to_string_lossy().into_owned()),
    }
}

/// 이미지가 아닌 파일은 OS 기본 앱으로 연다.
#[tauri::command]
fn open_default(path: String) -> CmdResult<()> {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&path).spawn();
    #[cfg(windows)]
    let r = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(&path)
        .spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let r = std::process::Command::new("xdg-open").arg(&path).spawn();
    r.map(|_| ()).map_err(err)
}

#[tauri::command]
fn is_dir(path: String) -> bool {
    Path::new(&path).is_dir()
}

// ───────────────────────── 설정·기타 ─────────────────────────

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().clone()
}

/// 화면에서 바꾼 설정 일부를 합친다. 예: {"thumb_size": 200}
#[tauri::command]
fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> CmdResult<Settings> {
    let s = {
        let mut cur = state.settings.lock();
        let mut v = serde_json::to_value(&*cur).map_err(err)?;
        if let (Some(obj), Some(p)) = (v.as_object_mut(), patch.as_object()) {
            for (k, val) in p {
                obj.insert(k.clone(), val.clone());
            }
        }
        *cur = serde_json::from_value(v).map_err(err)?;
        cur.quick_folders.resize(9, None);
        cur.clone()
    };
    s.save(&state.settings_path);
    menu::sync_checks(&state.checks.lock(), &s);
    let _ = app.emit("settings-changed", &s);
    Ok(s)
}

#[tauri::command]
fn take_pending_open(state: State<'_, AppState>) -> Option<String> {
    state.pending_open.lock().take()
}

/// 화면이 처음 그려졌을 때 호출: 프로세스 시작부터의 시간 (ms).
/// marks는 화면 쪽 단계별 시각 (웹페이지 시작 기준 ms), page_origin은 웹페이지 시작 시각 (Unix ms).
#[tauri::command]
fn frontend_ready(
    state: State<'_, AppState>,
    page_origin: f64,
    marks: Vec<(String, f64)>,
) -> f64 {
    let ms = state.started.elapsed().as_secs_f64() * 1000.0;
    let page_at = page_origin - state.started_unix_ms;
    let steps: Vec<String> = marks
        .iter()
        .map(|(k, v)| format!("{k} {:.0}", page_at + v))
        .collect();
    eprintln!(
        "[isee] 첫 화면까지 {ms:.0}ms (엔진 준비 {:.0} · 웹페이지 시작 {page_at:.0} · {})",
        state.setup_ms,
        steps.join(" · ")
    );
    ms
}

#[derive(Serialize)]
struct Stats {
    generated: u64,
    avg_ms: f64,
    mem_hits: u64,
    disk_hits: u64,
    pending: usize,
    /// 백그라운드 작업 현황
    bg: BgStatus,
}

#[derive(Serialize)]
struct BgStatus {
    /// 모두 멈춤
    paused: bool,
    /// 폴더 썸네일 미리 만들기 켜짐
    prefill: bool,
    /// 미리 만들기 남은 수
    thumbs: usize,
    /// 비슷한 사진 분석 (끝난 수, 전체)
    similar: (usize, usize),
    /// 그중 클라우드에만 있어 분석하지 않고 넘긴 수
    similar_cloud: usize,
    /// 분석하는(했던) 범위 이름
    similar_label: String,
    faces_label: String,
    /// 얼굴 분석 (끝난 수, 전체)
    faces: (usize, usize),
}

fn bg_status(state: &AppState) -> BgStatus {
    BgStatus {
        paused: state.user_paused.load(Ordering::Relaxed),
        prefill: state.settings.lock().bg_prefill,
        thumbs: state.thumbs.background_pending(),
        similar: state.analysis.progress(),
        similar_cloud: state.analysis.skipped(),
        similar_label: state.similar_label.lock().clone(),
        faces_label: state.faces_label.lock().clone(),
        faces: state.faces.progress(),
    }
}

/// 백그라운드 작업 조절: pause / resume (모두), prefill_on / prefill_off (썸네일 미리 만들기),
/// stop_similar / stop_faces (지금 하는 분석 중단)
#[tauri::command]
fn bg_control(state: State<'_, AppState>, action: String) -> CmdResult<BgStatus> {
    match action.as_str() {
        "pause" | "resume" => {
            let p = action == "pause";
            state.user_paused.store(p, Ordering::Relaxed);
            state.thumbs.set_paused(p);
            state.analysis_paused.store(p || state.viewer_active.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        "prefill_on" | "prefill_off" => {
            let on = action == "prefill_on";
            state.thumbs.set_prefill(on);
            let mut s = state.settings.lock();
            s.bg_prefill = on;
            s.save(&state.settings_path);
        }
        "stop_similar" => {
            state.analysis.analyze(Vec::new());
        }
        "stop_faces" => {
            state.faces.analyze(Vec::new());
        }
        _ => return Err(format!("알 수 없는 동작: {action}")),
    }
    Ok(bg_status(&state))
}

#[tauri::command]
fn stats(state: State<'_, AppState>) -> Stats {
    let s = state.thumbs.stats();
    let generated = s.generated.load(Ordering::Relaxed);
    Stats {
        generated,
        avg_ms: s.gen_micros.load(Ordering::Relaxed) as f64 / 1000.0 / generated.max(1) as f64,
        mem_hits: s.mem_hits.load(Ordering::Relaxed),
        disk_hits: s.disk_hits.load(Ordering::Relaxed),
        pending: state.thumbs.pending(),
        bg: bg_status(&state),
    }
}

#[derive(Clone, Serialize)]
struct OpenRequest {
    path: String,
    /// 요청을 받은 시각 (Unix ms). 화면에 뜨기까지의 시간 측정용.
    at: f64,
}

/// 화면 쪽에서 잰 시간을 로그로 남긴다 (속도 확인용).
#[tauri::command]
fn perf_log(label: String, ms: f64) {
    eprintln!("[isee] {label} {ms:.0}ms");
}

/// 열려고 하는 파일·폴더를 화면에 알린다 (Finder의 "다음으로 열기", 더블클릭 등).
/// 앱 준비(setup)가 끝나기 전에 도착한 열기 요청. 맥은 Finder에서 파일로 앱을 켜면
/// 이 요청이 준비보다 먼저 온다. setup에서 꺼내 처리한다.
static EARLY_OPEN: Mutex<Option<String>> = Mutex::new(None);

fn open_path(app: &AppHandle, path: String) {
    // 명령줄·Finder에서 온 경로도 폴더 목록과 같은 꼴(NFC)로
    let path = listing::nfc_string(path);
    let Some(state) = app.try_state::<AppState>() else {
        *EARLY_OPEN.lock() = Some(path);
        return;
    };
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    state.prewarm(&path);
    *state.pending_open.lock() = Some(path.clone());
    let _ = app.emit("open-path", OpenRequest { path, at });
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Finder 빠른 동작·탐색기 우클릭에서 "--convert"로 불리면 창 없이 변환만 하고 끝낸다
    if let Some(code) = cli::run_if_requested() {
        std::process::exit(code);
    }
    let started = Instant::now();
    let started_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(p) = argv.get(1) {
                open_path(app, p.clone());
            } else if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .register_asynchronous_uri_scheme_protocol("isee", protocol::handle)
        .setup(move |app| {
            let handle = app.handle();
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let cache_dir = app.path().app_cache_dir()?;
            // 프로그램 안에서 오류(패닉)가 나면 앱 데이터 폴더의 errors.log에 남긴다 (꺼진 원인을 찾을 수 있게)
            {
                let log = data_dir.join(ERROR_LOG);
                let version = app.package_info().version.to_string();
                let default = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    use std::io::Write;
                    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let thread = std::thread::current().name().unwrap_or("이름 없음").to_string();
                    let bt = std::backtrace::Backtrace::force_capture();
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
                        let _ = writeln!(f, "[{secs}] iSee {version} · {} · 스레드 {thread}\n{info}\n{bt}\n", std::env::consts::OS);
                    }
                    default(info);
                }));
            }
            let settings_path = config_dir.join("settings.json");
            let settings = Settings::load(&settings_path);
            let cache = ThumbCache::open(Some(&cache_dir.join("thumbs.db")), 20_000)?;
            let undo_dir = cache_dir.join("undo");
            {
                let d = undo_dir.clone();
                std::thread::spawn(move || prune_undo_dir(&d));
            }
            let cores = std::thread::available_parallelism().map_or(8, |n| n.get());
            let analysis_paused = Arc::new(AtomicBool::new(false));
            // 얼굴 분석: 모델은 설치했을 때만 읽는다. 사진 넘기는 동안에는 멈춘다.
            let faces = FaceService::new(
                Arc::new(FaceDb::open(Some(&data_dir.join("faces.db")))?),
                analysis_paused.clone(),
                (cores / 4).clamp(1, 3),
            );
            // 0.1.5까지 경로 꼴(NFC/NFD)만 달라 두 벌로 저장된 얼굴을 한 번 합친다.
            // 사진 파일 정보를 읽어야 해서 켜는 시간을 늦추지 않게 따로 돌린다 (한 번 하면 다음부터는 바로 끝남).
            {
                let db = faces.db().clone();
                std::thread::Builder::new().name("faces-merge".into()).spawn(move || db.merge_spellings())?;
            }
            {
                let handle = handle.clone();
                let last = AtomicU64::new(0);
                faces.set_progress(Arc::new(move |done, total| {
                    let now = started.elapsed().as_millis() as u64;
                    if done < total && now.saturating_sub(last.load(Ordering::Relaxed)) < 200 {
                        return;
                    }
                    last.store(now, Ordering::Relaxed);
                    let _ = handle.emit("faces-progress", (done, total));
                }));
            }
            // 분석은 코어 절반만 써서, 분석 중에도 썸네일·화면 이동이 느려지지 않게 한다
            let analysis = AnalysisService::new(
                FeatureStore::open(Some(&cache_dir.join("analysis.db")))?,
                analysis_paused.clone(),
                (cores / 2).clamp(1, 6),
            );
            {
                // 진행 알림은 0.1초에 한 번 (마지막은 꼭)
                let handle = handle.clone();
                let last = AtomicU64::new(0);
                analysis.set_progress(Arc::new(move |done, total| {
                    let now = started.elapsed().as_millis() as u64;
                    if done < total && now.saturating_sub(last.load(Ordering::Relaxed)) < 100 {
                        return;
                    }
                    last.store(now, Ordering::Relaxed);
                    let _ = handle.emit("analysis-progress", (done, total));
                }));
            }
            // 얼굴 썸네일을 만들 때 사진 읽기는 2개씩만 (메모리를 일정하게)
            let face_crops = people::CropService::new(faces.db().clone(), 2);
            let (menu, checks) = menu::build(handle, &settings)?;
            app.set_menu(menu)?;
            app.on_menu_event(menu::on_event);
            let pending = EARLY_OPEN
                .lock()
                .take()
                .or_else(|| std::env::args().nth(1).filter(|a| !a.starts_with('-')).map(listing::nfc_string));
            let state_prefill = settings.bg_prefill;
            app.manage(AppState {
                thumbs: ThumbService::new(cache, isee_core::THUMB_PX, cores.saturating_sub(2).max(2)),
                // 비교 화면은 최대 9장을 한꺼번에 요청하므로 4개로 동시에 준비하고 12장을 남겨 둔다
                viewer: ViewerService::new(4, 12),
                analysis,
                analysis_paused,
                viewer_active: AtomicBool::new(false),
                user_paused: AtomicBool::new(false),
                similar_label: Mutex::new(String::new()),
                faces_label: Mutex::new(String::new()),
                organize_items: Mutex::new(Vec::new()),
                faces,
                models_dir: data_dir.join("models"),
                face_crops,
                report_lock: Mutex::new(()),
                marks: Marks::open(Some(&data_dir.join("marks.db")))?,
                settings: Mutex::new(settings),
                settings_path,
                checks: Mutex::new(checks),
                undo: Mutex::new(Vec::new()),
                pending_open: Mutex::new(pending),
                started,
                started_unix_ms,
                setup_ms: started.elapsed().as_secs_f64() * 1000.0,
                undo_dir,
                ctx_menu: Mutex::new(None),
            });
            // 사용자가 꺼 둔 썸네일 미리 만들기
            if !state_prefill {
                app.state::<AppState>().thumbs.set_prefill(false);
            }
            // 파일을 더블클릭해서 앱이 켜진 경우: 화면이 뜨는 동안 디코딩을 먼저 시작
            let state = app.state::<AppState>();
            if let Some(p) = state.pending_open.lock().clone() {
                state.prewarm(&p);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // 맥: 창을 닫아도 앱은 남겨 두어 다음에 사진을 열 때 바로 뜨게 한다.
            if let WindowEvent::CloseRequested { api, .. } = event
                && cfg!(target_os = "macos")
                && window.app_handle().state::<AppState>().settings.lock().keep_running
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            places,
            subdirs,
            open_folder,
            set_viewport,
            set_viewer_active,
            file_info,
            trash,
            transfer,
            rename,
            create_folder,
            undo,
            reveal,
            open_default,
            is_dir,
            context_menu,
            set_marks,
            analyze_folder,
            stop_analysis,
            similar_report,
            organize_scan,
            organize_plan,
            organize_apply,
            save_edit,
            faces_status,
            faces_install,
            faces_analyze,
            faces_stop,
            faces_crops_cancel,
            faces_redo_turn,
            open_homepage,
            app_info,
            faces_report,
            faces_name,
            faces_reject,
            faces_apart,
            analysis_estimate,
            faces_delete_person,
            faces_clear,
            faces_organize,
            bg_control,
            convert,
            rename_plan,
            rename_apply,
            shell_integration,
            get_settings,
            update_settings,
            take_pending_open,
            frontend_ready,
            perf_log,
            stats,
        ])
        .build(tauri::generate_context!())
        .expect("iSee를 시작할 수 없습니다");

    app.run(|app, event| match event {
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => {
            if let Some(p) = urls.into_iter().find_map(|u| u.to_file_path().ok()) {
                open_path(app, p.to_string_lossy().into_owned());
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        _ => {}
    });
}
