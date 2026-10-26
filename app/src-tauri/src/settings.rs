use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Kinds {
    pub image: bool,
    pub video: bool,
    pub pdf: bool,
    pub doc: bool,
    pub other: bool,
}

impl Default for Kinds {
    /// 기본은 이미지만 보여준다 (요구사항 7: 필요한 종류만 켜서 속도 유지).
    fn default() -> Self {
        Self {
            image: true,
            video: false,
            pdf: false,
            doc: false,
            other: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[allow(dead_code)] // 일부 필드는 화면 쪽에서만 쓴다
pub struct Settings {
    pub kinds: Kinds,
    /// "name" | "date" | "size" | "type"
    pub sort_by: String,
    pub sort_desc: bool,
    pub show_hidden: bool,
    pub show_info: bool,
    /// 속도 표시 (디코딩 시간 등)
    pub show_hud: bool,
    /// 창을 닫아도 앱을 띄워 두어 다음에 바로 열리게 한다 (맥)
    pub keep_running: bool,
    /// 썸네일 칸 크기 (CSS px)
    pub thumb_size: u32,
    /// 숫자키 1~9 빠른 이동 폴더
    pub quick_folders: Vec<Option<String>>,
    pub last_folder: Option<String>,
    /// 마지막 뷰어 화면 크기 (실제 픽셀). 사진을 열라는 요청이 오면 화면보다 먼저 이 크기로 디코딩을 시작한다.
    pub view_box: (u32, u32),
    /// 변환 결과 저장 방식: "new"(새 파일) | "overwrite"(덮어쓰기) | "folder"(지정 폴더)
    pub convert_save: String,
    pub convert_folder: Option<String>,
    /// 컬링 모드: P 남김, X 버림, 숫자키 별점
    pub culling: bool,
    /// 컬링 모드 보기: "all" | "picked" | "unflagged" | "rejected" | "rated"
    pub cull_filter: String,
    /// 비슷한 사진: 묶는 정도, 추천 장수, 기준 순서
    pub similar: isee_core::similar::Options,
    /// 날짜·장소별 정리: 폴더 이름 규칙, 장소 단위 등
    pub organize: isee_core::organize::Options,
    /// 정리할 때 복사 (기본은 이동)
    pub organize_copy: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            kinds: Kinds::default(),
            sort_by: "name".into(),
            sort_desc: false,
            show_hidden: false,
            show_info: true,
            show_hud: false,
            keep_running: true,
            thumb_size: 168,
            quick_folders: vec![None; 9],
            last_folder: None,
            view_box: (0, 0),
            convert_save: "new".into(),
            convert_folder: None,
            culling: false,
            cull_filter: "all".into(),
            similar: Default::default(),
            organize: Default::default(),
            organize_copy: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        let mut s: Settings = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        s.quick_folders.resize(9, None);
        s
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, b);
        }
    }
}
