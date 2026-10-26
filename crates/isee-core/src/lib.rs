//! iSee 엔진: 화면(UI)과 무관한 모든 무거운 작업을 맡는다.
//! 폴더 목록, 디코딩, 썸네일 캐시와 생성 순서, 파일 작업.

pub mod analysis;
pub mod cache;
pub mod convert;
pub mod decode;
pub mod faces;
pub mod formats;
pub mod fsops;
pub mod info;
pub mod jpegmeta;
pub mod listing;
pub mod marks;
pub mod organize;
pub mod meta;
pub mod people;
pub mod places;
pub mod rename;
pub mod similar;
pub mod thumbs;
pub mod viewer;

/// 썸네일 긴 변의 픽셀 수. 레티나 화면에서 200pt 칸을 선명하게 채우는 크기.
pub const THUMB_PX: u32 = 400;
