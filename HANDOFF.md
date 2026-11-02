# HANDOFF — iSee
> 기준: 2026-11-02 · v0.1.11 공개 직후 (커밋은 git log 맨 위) · 상태: 작업 중
> 다음 AI에게: 이 파일만 읽고 '다음 할 일' 1번부터 시작. 파일은 '수정 지도'에 있는 것부터 필요할 때만 연다.

## 한 줄 요약
교사(다있쌤 로디)가 학생 사진을 빠르게 보고·고르고·정리·인쇄하는 무료 윈도우·맥 사진 뷰어 (Rust + Tauri 2 + Svelte 5). 사용자와는 항상 한국어로.

## 빠른 시작
- 시험: `cargo test --workspace` · `npm --prefix app run check` · `cargo clippy -p isee-app` (경고 32개는 예전부터 있음, 새로 늘리지 말 것)
- 개발 실행: `npm --prefix app run tauri dev` (확인 필요: 지금까지는 아래 시험용 앱 방식으로 확인)
- 배포: README "새 버전 내보내기" (공개 뒤 확인: latest.json 버전·내려받기 200·dmg 서명 certificate root `H"96db2e10faef00c49a901789a273f6cc27fb977d"`, 그다음 docs/PLAN.md 상태 줄과 site/index.html 기본 버전을 올려 커밋·사이트 배포) — Cargo.toml `[workspace.package] version` + `app/src-tauri/tauri.conf.json` version 올림 → 커밋 → `git tag vX.Y.Z` → main·태그 push → GitHub Actions가 릴리스 초안(주인만 봄) 생성 → `scripts/publish.sh vX.Y.Z`로 공개 저장소 `janghwansangai/isee-releases`에 올림
- 소개 사이트: `site/` → 저장소 루트에서 `npx -y wrangler@4.155.0 deploy` → https://isee-app.janhan97.workers.dev

## 현재 상태
- ✅ 완료: v0.1.11 공개(2026-11-02: 보안 점검 반영 — zip 9.2.2, 액션 커밋 고정, site/_headers, 인쇄 한도, 새 폴더 이름 검사). 그 전: 사진 인쇄(0.1.10), 업데이트 확인 안내(0.1.9), 클라우드 사진 내려받아 분석·다운로드 제거(0.1.8), 오류 신고(0.1.7). 사이트 기본 버전 0.1.11.
- ❌ 막힘: 사용자 윈도우 PC에서 "업데이트 확인"이 한참 뒤 실패(맥은 0.13초). 학교망 GitHub 차단·프록시 의심, 실패 창의 "자세한 내용" 문구를 아직 못 받음.

## 다음 할 일 (우선순위순)
1. 윈도우 업데이트 실패 원인: 사용자에게 0.1.9+ 실패 창 문구와 브라우저로 GitHub 내려받기가 되는지 받기. GitHub 차단이면 윈도우 setup.exe(12.6MB < Workers 25MiB)·latest.json을 Cloudflare 사이트에도 올리고 두 번째 updater endpoint로 추가 (맥 tar.gz 35MB는 한도 초과)
2. 윈도우 실기 확인 안 된 것: 인쇄(Edge 인쇄 창), 업데이트, 휴지통 되돌리기, 탐색기 우클릭 메뉴
3. 사용자에게 제안해 둔 것: 공개 저장소의 GitHub secret scanning·push protection·Dependabot 켜기 (사용자 허락 필요)

## 수정 지도 (이걸 바꾸려면 → 여기)
- 앱 명령·상태·이벤트 → `app/src-tauri/src/lib.rs` (거의 모든 Tauri 명령), 메뉴 → `menu.rs`, 사진 주소 isee:// → `protocol.rs`, 인쇄 창 → `print.rs`
- 무거운 작업(목록·디코딩·분석·얼굴·클라우드) → `crates/isee-core/src/*.rs` (`cloud.rs`, `people.rs`, `analysis.rs`, `fsops.rs` …)
- 화면 상태·공통 동작 → `app/src/lib/store.svelte.ts`, 명령 호출 타입 → `app/src/lib/api.ts`, 키·메뉴 연결 → `app/src/App.svelte`
- 인쇄 배치·그리기 → `app/src/lib/print.ts` + `components/PrintView.svelte`
- 업데이트 확인 → `app/src/lib/update.ts`, 사용 설명서 → `components/HelpDialog.svelte`
- 배포 → `.github/workflows/release.yml`, `scripts/publish.sh` · 사이트 → `site/index.html`, `site/_headers`, `wrangler.jsonc`
- 결정·측정 기록 → `docs/PLAN.md`(상태 머리말을 버전마다 갱신), `docs/BENCH.md`

## 주의사항 (코드만 봐서는 모르는 것)
- 사진 보기 속도를 늦추는 기능은 사용자 허락을 먼저 받는다.
- 앱 화면 확인: 사용자의 iSee가 켜져 있으면 같은 식별자 앱은 바로 꺼진다(single-instance). `cd app && npx tauri build --debug --no-bundle --config '{"identifier":"com.isee.viewer.cloudtest","productName":"iSeeTest"}'` → target/debug/isee-app을 임시 iSeeTest.app(Info.plist에 같은 식별자, `codesign -s -`)에 넣고 `HOME=<가짜 홈>`으로 실행 → computer-use(배경 조작)로 확인. 드롭다운은 배경에서 못 바꿈.
- 맥 WKWebView 인쇄는 반드시 `runOperationModalForWindow`(비동기). 동기 `runOperation`은 쪽이 끝없이 생겨 PDF가 1분에 1.3GB가 됐다. 개발 빌드는 `ISEE_PRINT_PDF=<경로>`면 인쇄 창 없이 PDF 저장 — 크기 감시와 함께 쓸 것.
- 클라우드 전용 파일은 읽으면 내려받기가 시작된다. 새 기능은 `listing::is_online_only`를 먼저 확인. 시험은 사용자 개인 구글 드라이브의 작은 폴더로 하고 끝나면 다운로드 제거.
- 맥 한글 경로는 NFC로 통일(`listing::nfc`). 서명 인증서가 바뀌면 사용자의 폴더 접근 허용이 풀린다 — 같은 인증서 유지.
- 액션은 커밋 SHA로 고정됨: 올릴 때 SHA를 직접 바꿀 것. 저장소는 2026-10-30부터 공개(무료 빌드 시간 때문). 서명 키는 GitHub 비밀값과 `~/.tauri`에만.
- `protocol.rs`의 CORS `*`는 일부러 유지(이 앱 웹뷰에서만 열림, 좁히면 윈도우 이미지 로딩을 시험 없이 깨뜨릴 위험).
- `wrangler pages project create`를 저장소 루트에서 실행하면 앱 소스가 공개 배포된 적 있음 — 항상 `wrangler.jsonc`(assets `./site`, preview_urls false)로만 배포.

## 읽지 않아도 되는 것
- `target/`(38GB), `app/node_modules/`, `app/dist/`, `crates/isee-core/data/places.bin`(2.3MB 지명 데이터), `Cargo.lock`, `app/package-lock.json`

## 비밀값 위치 (값은 쓰지 않음)
- 업데이트 서명 키 — GitHub 비밀값 `TAURI_SIGNING_PRIVATE_KEY`·`_PASSWORD`, 로컬 `~/.tauri/isee-updater.key`
- 맥 서명 인증서 — GitHub 비밀값 `MACOS_CERT_P12`·`MACOS_CERT_PASSWORD`, 로컬 `~/.tauri/isee-macos-codesign.*`
- Cloudflare — 사용자 계정 wrangler OAuth 로그인(이 컴퓨터), 코드에 키 없음
