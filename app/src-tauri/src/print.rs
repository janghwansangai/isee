//! 사진 인쇄: 화면이 인쇄할 쪽들(용지 크기 그대로 그린 이미지)을 문서에 넣어 두면 운영체제 인쇄 창을 연다.
//! 맥: WKWebView 인쇄를 용지 크기·여백 0으로 직접 연다 (wry의 기본 인쇄는 마지막에 쓴 용지를 그대로 써서
//!     쪽 크기가 어긋날 수 있다). 머리말·꼬리말은 넣지 않는다.
//! 윈도우: WebView2 인쇄 창 (window.print). 용지 크기는 화면의 @page가 정하고, 여백이 0이라 머리말·꼬리말이 없다.

use tauri::WebviewWindow;

/// 인쇄 창을 연다. width_mm·height_mm: 한 쪽의 크기 (가로로 놓으면 가로가 더 길다)
#[tauri::command]
pub fn print_pages(window: WebviewWindow, width_mm: f64, height_mm: f64) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        mac::print(&window, width_mm, height_mm)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (width_mm, height_mm);
        window.print().map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2_app_kit::{
        NSPaperOrientation, NSPrintInfo, NSPrintJobSavingURL, NSPrintSaveJob, NSPrintSpoolJob, NSPrintingPaginationMode,
        NSWindow,
    };
    use objc2_foundation::{NSSize, NSString, NSURL};
    use objc2_web_kit::WKWebView;
    use tauri::WebviewWindow;

    const PT_PER_MM: f64 = 72.0 / 25.4;

    pub fn print(window: &WebviewWindow, width_mm: f64, height_mm: f64) -> Result<(), String> {
        // 시험용: 인쇄 창 대신 이 경로에 PDF로 저장 (개발 빌드에서만)
        let save_to = if cfg!(debug_assertions) { std::env::var("ISEE_PRINT_PDF").ok() } else { None };
        window
            .with_webview(move |wv| unsafe {
                let web = &*(wv.inner() as *const WKWebView);
                let win = &*(wv.ns_window() as *const NSWindow);
                // 사용자가 마지막에 고른 프린터는 그대로 두고 용지·방향·여백만 바꾼다
                let info = NSPrintInfo::sharedPrintInfo();
                let landscape = width_mm > height_mm;
                info.setOrientation(if landscape { NSPaperOrientation::Landscape } else { NSPaperOrientation::Portrait });
                info.setPaperSize(NSSize::new(width_mm * PT_PER_MM, height_mm * PT_PER_MM));
                info.setTopMargin(0.0);
                info.setBottomMargin(0.0);
                info.setLeftMargin(0.0);
                info.setRightMargin(0.0);
                info.setHorizontalPagination(NSPrintingPaginationMode::Automatic);
                info.setVerticalPagination(NSPrintingPaginationMode::Automatic);
                info.setHorizontallyCentered(false);
                info.setVerticallyCentered(false);
                if let Some(path) = &save_to {
                    info.setJobDisposition(NSPrintSaveJob);
                    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                    info.dictionary().insert(NSPrintJobSavingURL, &*url);
                } else {
                    info.setJobDisposition(NSPrintSpoolJob);
                }
                let op = web.printOperationWithPrintInfo(&info);
                // 웹뷰 인쇄는 창에 붙여(비동기) 돌려야 쪽을 제대로 나눈다 (동기 runOperation은 쪽이 끝없이 생김)
                op.setShowsPrintPanel(save_to.is_none());
                op.setShowsProgressPanel(save_to.is_none());
                op.setCanSpawnSeparateThread(true);
                op.runOperationModalForWindow_delegate_didRunSelector_contextInfo(win, None, None, std::ptr::null_mut());
            })
            .map_err(|e| e.to_string())
    }
}
