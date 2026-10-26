//! OS 파일 관리자의 우클릭 메뉴에 iSee 변환을 넣고 뺀다.
//! - 맥: Finder "빠른 동작" (~/Library/Services 의 Automator 작업)
//! - 윈도우: 탐색기 이미지 우클릭 메뉴 (HKCU 레지스트리, 윈도우 11은 "더 많은 옵션 표시" 안)
//! 둘 다 앱을 화면 없이 `--convert`로 실행한다.

pub struct Action {
    #[cfg_attr(not(windows), allow(dead_code))] // 윈도우 레지스트리 키 이름에만 쓴다
    pub id: &'static str,
    pub label: &'static str,
    pub op: &'static str,
}

pub const ACTIONS: &[Action] = &[
    Action { id: "rotate_cw", label: "시계 방향으로 회전", op: "rotate=cw" },
    Action { id: "rotate_ccw", label: "반시계 방향으로 회전", op: "rotate=ccw" },
    Action { id: "resize_1920", label: "크기 1920px", op: "resize=1920" },
    Action { id: "resize_50", label: "크기 50%", op: "resize=50%" },
    Action { id: "to_jpeg", label: "JPG로 변환", op: "format=jpeg" },
    Action { id: "to_webp", label: "WebP로 변환", op: "format=webp" },
    Action { id: "size_1mb", label: "1MB 이하로", op: "size=1000000" },
    Action { id: "share", label: "공유용으로 저장", op: "share" },
];

fn exe() -> anyhow::Result<std::path::PathBuf> {
    Ok(std::env::current_exe()?.canonicalize()?)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{ACTIONS, exe};
    use std::path::PathBuf;

    const PREFIX: &str = "iSee - ";

    fn services_dir() -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Services")
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    fn info_plist(label: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSIconName</key>
			<string>NSActionTemplate</string>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>{label}</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.image</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#,
            label = xml_escape(label)
        )
    }

    fn arg(name: &str, idx: u32) -> String {
        format!(
            "\t\t\t\t\t<key>{idx}</key>\n\t\t\t\t\t<dict>\n\t\t\t\t\t\t<key>default value</key>\n\t\t\t\t\t\t<integer>0</integer>\n\t\t\t\t\t\t<key>name</key>\n\t\t\t\t\t\t<string>{name}</string>\n\t\t\t\t\t\t<key>required</key>\n\t\t\t\t\t\t<string>0</string>\n\t\t\t\t\t\t<key>type</key>\n\t\t\t\t\t\t<string>0</string>\n\t\t\t\t\t\t<key>uuid</key>\n\t\t\t\t\t\t<string>{idx}</string>\n\t\t\t\t\t</dict>\n"
        )
    }

    /// Automator "셸 스크립트 실행" 하나로 된 빠른 동작. 고른 파일을 인자로 넘긴다.
    fn document(command: &str) -> String {
        let args: String = ["inputMethod", "CheckedForUserDefaultShell", "source", "COMMAND_STRING", "shell"]
            .iter()
            .enumerate()
            .map(|(i, n)| arg(n, i as u32))
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{command}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/bash</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>6A1D1C1E-1F0B-4C5A-9D7E-1A2B3C4D5E61</string>
				<key>Keywords</key>
				<array>
					<string>Shell</string>
					<string>Script</string>
				</array>
				<key>OutputUUID</key>
				<string>6A1D1C1E-1F0B-4C5A-9D7E-1A2B3C4D5E62</string>
				<key>UUID</key>
				<string>6A1D1C1E-1F0B-4C5A-9D7E-1A2B3C4D5E63</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>arguments</key>
				<dict>
{args}				</dict>
				<key>isViewVisible</key>
				<integer>1</integer>
				<key>location</key>
				<string>309.5:305.0</string>
				<key>nibPath</key>
				<string>/System/Library/Automator/Run Shell Script.action/Contents/Resources/Base.lproj/main.nib</string>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleIDsByPath</key>
		<dict/>
		<key>applicationPaths</key>
		<array/>
		<key>inputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject.image</string>
		<key>outputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>presentationMode</key>
		<integer>15</integer>
		<key>processesInput</key>
		<integer>0</integer>
		<key>serviceApplicationBundleID</key>
		<string>com.apple.finder</string>
		<key>serviceApplicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject.image</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<integer>0</integer>
		<key>systemImageName</key>
		<string>NSActionTemplate</string>
		<key>useAutomaticInputType</key>
		<integer>0</integer>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#,
            command = xml_escape(command),
            args = args
        )
    }

    fn shell_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', r"'\''"))
    }

    /// dir 아래에 빠른 동작 작업들을 만든다 (테스트에서는 임시 폴더)
    pub fn write_workflows(dir: &std::path::Path) -> anyhow::Result<Vec<PathBuf>> {
        let exe = exe()?;
        let mut out = Vec::new();
        for a in ACTIONS {
            let label = format!("{PREFIX}{}", a.label);
            let wf = dir.join(format!("{label}.workflow")).join("Contents");
            std::fs::create_dir_all(&wf)?;
            let command = format!("{} --convert {} \"$@\"", shell_quote(&exe.to_string_lossy()), a.op);
            std::fs::write(wf.join("Info.plist"), info_plist(&label))?;
            std::fs::write(wf.join("document.wflow"), document(&command))?;
            out.push(wf.parent().unwrap().to_path_buf());
        }
        Ok(out)
    }

    pub fn install() -> anyhow::Result<usize> {
        let dir = services_dir();
        std::fs::create_dir_all(&dir)?;
        let n = write_workflows(&dir)?.len();
        refresh();
        Ok(n)
    }

    pub fn uninstall() -> anyhow::Result<usize> {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(services_dir()) {
            for e in rd.filter_map(|e| e.ok()) {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(PREFIX) && name.ends_with(".workflow") {
                    std::fs::remove_dir_all(e.path())?;
                    n += 1;
                }
            }
        }
        refresh();
        Ok(n)
    }

    pub fn installed() -> bool {
        services_dir().join(format!("{PREFIX}{}.workflow", ACTIONS[0].label)).exists()
    }

    /// Finder가 새 빠른 동작을 바로 알아보게 서비스 목록을 새로 읽힌다
    fn refresh() {
        let _ = std::process::Command::new("/System/Library/CoreServices/pbs").arg("-flush").status();
    }
}

#[cfg(windows)]
mod imp {
    use super::{ACTIONS, exe};
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const KEY: &str = r"Software\Classes\SystemFileAssociations\image\shell\iSee";

    pub fn install() -> anyhow::Result<usize> {
        let exe = exe()?.to_string_lossy().trim_start_matches(r"\\?\").to_string();
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (root, _) = hkcu.create_subkey(KEY)?;
        root.set_value("MUIVerb", &"iSee 빠른 변환")?;
        root.set_value("Icon", &format!("\"{exe}\",0"))?;
        root.set_value("SubCommands", &"")?;
        for (i, a) in ACTIONS.iter().enumerate() {
            let (k, _) = root.create_subkey(format!(r"shell\{:02}{}", i, a.id))?;
            k.set_value("MUIVerb", &a.label)?;
            let (c, _) = k.create_subkey("command")?;
            c.set_value("", &format!("\"{exe}\" --convert {} \"%1\"", a.op))?;
        }
        Ok(ACTIONS.len())
    }

    pub fn uninstall() -> anyhow::Result<usize> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.delete_subkey_all(KEY) {
            Ok(()) => Ok(ACTIONS.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    pub fn installed() -> bool {
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(KEY).is_ok()
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    pub fn install() -> anyhow::Result<usize> {
        anyhow::bail!("이 OS는 아직 지원하지 않습니다")
    }
    pub fn uninstall() -> anyhow::Result<usize> {
        Ok(0)
    }
    pub fn installed() -> bool {
        false
    }
}

pub use imp::*;
