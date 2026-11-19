//! 파일 복사·붙여넣기용 운영체제 클립보드. Finder·탐색기와 파일을 주고받는다.
//! 맥: NSPasteboard에 파일 주소(file URL). Finder는 ⌘V로 복사, ⌥⌘V로 옮긴다.
//! 윈도우: CF_HDROP(파일 목록) + "Preferred DropEffect"(잘라내기면 옮기기). 탐색기와 같은 방식.
//! 모두 메인 스레드에서 부른다 (lib.rs의 on_main).

/// 클립보드에 있는 파일
pub struct Files {
    pub paths: Vec<String>,
    /// 탐색기에서 잘라내기로 담음 (맥 Finder에는 잘라내기가 없어 늘 false)
    pub cut: bool,
}

pub use imp::{clear, has_files, native_edit, read_files, seq, write_files};

#[cfg(target_os = "macos")]
mod imp {
    use super::Files;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{ClassType, MainThreadMarker, sel};
    use objc2_app_kit::{
        NSApplication, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardURLReadingFileURLsOnlyKey, NSPasteboardWriting,
    };
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSURL};

    /// 클립보드가 바뀔 때마다 늘어나는 번호 (담은 뒤 다른 앱이 바꿨는지 알려고)
    pub fn seq() -> i64 {
        NSPasteboard::generalPasteboard().changeCount() as i64
    }

    pub fn write_files(paths: &[String], _cut: bool) -> Result<(), String> {
        let pb = NSPasteboard::generalPasteboard();
        let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
            .iter()
            .map(|p| ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(p))))
            .collect();
        pb.clearContents();
        if pb.writeObjects(&NSArray::from_retained_slice(&urls)) {
            Ok(())
        } else {
            Err("클립보드에 담지 못했습니다".into())
        }
    }

    /// 파일이 들어 있는가. 종류만 보고 내용은 읽지 않는다
    /// (macOS는 앱이 사용자 몰래 클립보드 내용을 읽으면 '붙여넣기 허용' 창을 띄울 수 있다)
    pub fn has_files() -> bool {
        let ty: &NSString = unsafe { NSPasteboardTypeFileURL };
        NSPasteboard::generalPasteboard().types().is_some_and(|t| t.containsObject(ty))
    }

    pub fn read_files() -> Option<Files> {
        let pb = NSPasteboard::generalPasteboard();
        let classes = NSArray::from_slice(&[NSURL::class()]);
        let yes = NSNumber::new_bool(true);
        let yes: &AnyObject = &yes;
        let key: &NSString = unsafe { NSPasteboardURLReadingFileURLsOnlyKey };
        let opts = NSDictionary::from_slices(&[key], &[yes]);
        let objs = unsafe { pb.readObjectsForClasses_options(&classes, Some(&opts)) }?;
        let paths: Vec<String> = objs
            .iter()
            .filter_map(|o| o.downcast::<NSURL>().ok())
            .filter_map(|u| u.path())
            // Finder가 준 경로는 디스크에 저장된 꼴(대개 NFD)이라 앱 안의 꼴(NFC)로 맞춘다
            .map(|p| isee_core::listing::nfc_string(p.to_string()))
            .collect();
        (!paths.is_empty()).then_some(Files { paths, cut: false })
    }

    pub fn clear() {
        NSPasteboard::generalPasteboard().clearContents();
    }

    /// 글자 칸의 복사·잘라내기·붙여넣기: 메뉴가 키를 받았으니 웹뷰에 원래 동작을 보낸다
    pub fn native_edit(action: &str) {
        let sel = match action {
            "copy" => sel!(copy:),
            "cut" => sel!(cut:),
            "paste" => sel!(paste:),
            _ => return,
        };
        let Some(mtm) = MainThreadMarker::new() else { return };
        unsafe {
            NSApplication::sharedApplication(mtm).sendAction_to_from(sel, None, None);
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::Files;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL, POINT};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
        OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows_sys::Win32::UI::Shell::{DROPFILES, DragQueryFileW};

    const CF_HDROP: u32 = 15;
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;
    const DROPEFFECT_LINK: u32 = 4;

    /// 열린 클립보드 (놓으면 닫는다). 다른 프로그램이 잠깐 쥐고 있을 수 있어 몇 번 다시 해 본다.
    struct Open;

    impl Open {
        fn new() -> Option<Open> {
            for _ in 0..20 {
                if unsafe { OpenClipboard(null_mut()) } != 0 {
                    return Some(Open);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            None
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    /// 탐색기가 복사(1)·잘라내기(2)를 적어 두는 형식
    fn drop_effect_format() -> u32 {
        let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    }

    pub fn seq() -> i64 {
        unsafe { GetClipboardSequenceNumber() as i64 }
    }

    unsafe fn global_from(bytes: &[u8]) -> Option<HGLOBAL> {
        unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if h.is_null() {
                return None;
            }
            let p = GlobalLock(h) as *mut u8;
            if p.is_null() {
                GlobalFree(h);
                return None;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            GlobalUnlock(h);
            Some(h)
        }
    }

    pub fn write_files(paths: &[String], cut: bool) -> Result<(), String> {
        // DROPFILES 머리 + 경로들 (UTF-16, 하나마다 0, 끝에 0 하나 더)
        let header = std::mem::size_of::<DROPFILES>();
        let df = DROPFILES { pFiles: header as u32, pt: POINT { x: 0, y: 0 }, fNC: 0, fWide: 1 };
        let mut bytes = vec![0u8; header];
        unsafe { std::ptr::write_unaligned(bytes.as_mut_ptr() as *mut DROPFILES, df) };
        for p in paths {
            for w in p.encode_utf16().chain([0]) {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[0, 0]);
        let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY | DROPEFFECT_LINK };
        let _open = Open::new().ok_or("클립보드를 열지 못했습니다 (다른 프로그램이 쓰는 중)")?;
        unsafe {
            EmptyClipboard();
            let h = global_from(&bytes).ok_or("메모리가 모자랍니다")?;
            if SetClipboardData(CF_HDROP, h).is_null() {
                GlobalFree(h);
                return Err("클립보드에 담지 못했습니다".into());
            }
            if let Some(h) = global_from(&effect.to_le_bytes())
                && SetClipboardData(drop_effect_format(), h).is_null()
            {
                GlobalFree(h);
            }
        }
        Ok(())
    }

    pub fn has_files() -> bool {
        unsafe { IsClipboardFormatAvailable(CF_HDROP) != 0 }
    }

    pub fn read_files() -> Option<Files> {
        unsafe {
            if !has_files() {
                return None;
            }
            let _open = Open::new()?;
            let h = GetClipboardData(CF_HDROP);
            if h.is_null() {
                return None;
            }
            let n = DragQueryFileW(h, u32::MAX, null_mut(), 0);
            let mut paths = Vec::new();
            for i in 0..n {
                let len = DragQueryFileW(h, i, null_mut(), 0);
                let mut buf = vec![0u16; len as usize + 1];
                let got = DragQueryFileW(h, i, buf.as_mut_ptr(), len + 1);
                if got > 0 {
                    paths.push(String::from_utf16_lossy(&buf[..got as usize]));
                }
            }
            let mut cut = false;
            let e = GetClipboardData(drop_effect_format());
            if !e.is_null() {
                let p = GlobalLock(e) as *const u32;
                if !p.is_null() {
                    let v = p.read_unaligned();
                    GlobalUnlock(e);
                    cut = v & DROPEFFECT_MOVE != 0 && v & DROPEFFECT_COPY == 0;
                }
            }
            (!paths.is_empty()).then_some(Files { paths, cut })
        }
    }

    pub fn clear() {
        if let Some(_open) = Open::new() {
            unsafe {
                EmptyClipboard();
            }
        }
    }

    /// 윈도우는 메뉴의 복사·붙여넣기가 Ctrl+C·V 키를 보내 웹뷰가 직접 처리한다
    pub fn native_edit(_action: &str) {}
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::Files;

    pub fn seq() -> i64 {
        0
    }
    pub fn write_files(_paths: &[String], _cut: bool) -> Result<(), String> {
        Err("이 운영체제는 클립보드를 지원하지 않습니다".into())
    }
    pub fn has_files() -> bool {
        false
    }
    pub fn read_files() -> Option<Files> {
        None
    }
    pub fn clear() {}
    pub fn native_edit(_action: &str) {}
}
