//! 맥 Quick Look 썸네일 (Finder와 같은 엔진): 동영상·PDF·문서, 그리고 아직 내려받지 않은 클라우드 파일.
//! 클라우드(구글 드라이브·iCloud·OneDrive 등 File Provider) 파일은 제공 앱이 가진 썸네일을 받아 오므로
//! 원본을 내려받지 않는다.

use super::Encoded;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSString, NSURL};
use objc2_quick_look_thumbnailing::{
    QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes, QLThumbnailGenerator,
    QLThumbnailRepresentation,
};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// 긴 변 max_px 썸네일. 만들 수 없거나 timeout 안에 오지 않으면 에러.
pub fn thumbnail(path: &Path, max_px: u32, timeout: Duration) -> anyhow::Result<Encoded> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let size = CGSize { width: max_px as f64, height: max_px as f64 };
    let request = unsafe {
        QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
            QLThumbnailGenerationRequest::alloc(),
            &url,
            size,
            1.0,
            // 아이콘은 빼고 실제 내용 썸네일만 (없으면 화면이 종류 아이콘을 그린다)
            QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
        )
    };
    let (tx, rx) = mpsc::sync_channel::<Option<usize>>(1);
    let block = block2::RcBlock::new(move |rep: *mut QLThumbnailRepresentation, _err: *mut objc2_foundation::NSError| {
        let img = unsafe { rep.as_ref() }.map(|r| {
            let cg = unsafe { r.CGImage() };
            // 다른 스레드로 넘기기 위해 소유권을 주소로 바꾼다 (받는 쪽이 해제)
            Retained::into_raw(cg) as usize
        });
        let _ = tx.try_send(img);
    });
    unsafe { QLThumbnailGenerator::sharedGenerator().generateBestRepresentationForRequest_completionHandler(&request, &block) };
    match rx.recv_timeout(timeout) {
        Ok(Some(ptr)) => super::imageio::encode_owned(ptr as *const std::ffi::c_void, 0.8),
        Ok(None) => anyhow::bail!("Quick Look 썸네일 없음"),
        Err(_) => {
            unsafe { QLThumbnailGenerator::sharedGenerator().cancelRequest(&request) };
            anyhow::bail!("Quick Look 썸네일 시간 초과")
        }
    }
}
