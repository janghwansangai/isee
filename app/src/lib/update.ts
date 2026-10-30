// 자동 업데이트: 공개 배포 저장소(isee-releases)의 latest.json을 확인하고, 서명이 맞는 새 버전만 설치한다.
// 실행 직후에는 확인하지 않고(실행 속도 우선) 15초 뒤에, 하루에 한 번만 조용히 확인한다.
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ask } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { store } from "./store.svelte";

const LAST_KEY = "isee.updateCheckedAt";
const DAY = 24 * 60 * 60 * 1000;
let busy = false;
/** 새 버전 설치를 묻거나 내려받는 중 (다시 확인하지 않는다) */
let installing = false;
/** 이번 확인의 결과(최신·실패)를 알려 준다: 메뉴로 눌렀거나, 자동 확인 중에 메뉴로 또 눌렀을 때 */
let report = false;

async function install(u: Update) {
  const notes = u.body?.trim() ? `\n\n${u.body.trim().slice(0, 600)}` : "";
  const ok = await ask(`새 버전 ${u.version}이 있습니다 (지금 ${u.currentVersion}). 내려받아 설치할까요?${notes}`, {
    title: "iSee 업데이트",
    okLabel: "설치하고 다시 시작",
    cancelLabel: "나중에",
  });
  if (!ok) return;
  let total = 0;
  let got = 0;
  store.progress = { done: 0, total: 100, label: "업데이트 내려받기" };
  try {
    await u.downloadAndInstall((e) => {
      if (e.event === "Started") total = e.data.contentLength ?? 0;
      else if (e.event === "Progress") {
        got += e.data.chunkLength;
        if (total) store.progress = { done: Math.round((got / total) * 100), total: 100, label: "업데이트 내려받기" };
      }
    });
  } finally {
    store.progress = null;
  }
  await relaunch();
}

/** 메뉴로 확인할 때 기다리는 시간 (느린 학교망 등) */
const MANUAL_TIMEOUT = 40_000;

/** 확인이 안 된 이유를 쉬운 말로 (학교·회사 인터넷은 GitHub 연결이 막히거나 느린 일이 많다).
 *  연결 실패와 시간 초과는 오류 문구가 같아서("error sending request") 걸린 시간으로 가린다. */
function reason(e: unknown, ms: number): string {
  const s = String(e);
  if (/signature/i.test(s)) return "업데이트 파일의 서명이 맞지 않아 설치하지 않았습니다.";
  if (ms >= MANUAL_TIMEOUT - 1000 || /timed? ?out|timeout/i.test(s)) return `GitHub에서 ${Math.round(ms / 1000)}초 동안 답이 없었습니다.`;
  if (/sending request|dns|resolve|connect|network|unreachable|refused|reset/i.test(s)) return "GitHub에 연결하지 못했습니다.";
  return "인터넷으로 새 버전을 확인하지 못했습니다.";
}

/** 메뉴로 확인했는데 실패: 이유와 함께 내려받기 사이트를 열지 묻는다 */
async function failed(e: unknown, ms: number) {
  const open = await ask(
    `${reason(e, ms)}\n\n` +
      `학교·회사 인터넷에서는 GitHub(github.com, githubusercontent.com) 연결이 막혀 있거나 느릴 수 있습니다. ` +
      `잠시 뒤 다시 하거나, 내려받기 사이트에서 새 버전을 받아 설치해도 됩니다 (설정과 분석 결과는 그대로 남습니다).\n\n` +
      `자세한 내용: ${String(e).slice(0, 300)}`,
    { title: "업데이트를 확인하지 못했습니다", kind: "warning", okLabel: "내려받기 사이트 열기", cancelLabel: "닫기" },
  );
  if (open) api.openDownloadPage().catch((err) => store.showToast(`${err}`, { error: true }));
}

/** manual: 메뉴에서 누른 경우 (최신이어도 알려 주고, 확인하는 동안·실패도 알린다) */
export async function checkForUpdate(manual: boolean) {
  if (installing) {
    if (manual) store.showToast("새 버전 설치를 진행하고 있습니다");
    return;
  }
  if (busy) {
    if (manual) {
      report = true;
      store.showToast("업데이트를 확인하고 있습니다. 잠시 뒤 결과가 나옵니다.", { sticky: true });
    }
    return;
  }
  if (!manual) {
    try {
      const last = Number(localStorage.getItem(LAST_KEY) ?? 0);
      if (Date.now() - last < DAY) return;
    } catch {
      // 저장소를 못 쓰면 그냥 확인한다
    }
  }
  busy = true;
  report = manual;
  // 결과가 나올 때까지 보여 준다 (느린 인터넷에서는 수십 초)
  if (manual) store.showToast("업데이트를 확인하는 중…", { sticky: true });
  const t0 = performance.now();
  let u: Update | null;
  try {
    // 메뉴로 누른 경우는 느린 인터넷(학교망 등)을 위해 더 기다린다
    u = await check({ timeout: manual ? MANUAL_TIMEOUT : 15000 });
    try {
      localStorage.setItem(LAST_KEY, String(Date.now()));
    } catch {
      // 무시
    }
  } catch (e) {
    // 확인은 끝났으니 안내 창이 떠 있는 동안에도 다시 누를 수 있게 한다
    busy = false;
    // 자동 확인이 실패하면(인터넷 없음 등) 조용히 넘어간다
    if (report) {
      store.hideToast();
      await failed(e, performance.now() - t0);
    }
    return;
  }
  busy = false;
  if (!u) {
    if (report) store.showToast("최신 버전입니다");
    return;
  }
  if (report) store.hideToast();
  installing = true;
  try {
    await install(u);
  } catch (e) {
    store.showToast(`업데이트를 설치하지 못했습니다: ${e}`, { error: true });
  } finally {
    installing = false;
  }
}

/** 실행 15초 뒤 한 번 (화면이 다 뜬 다음) */
export function scheduleUpdateCheck() {
  // 개발 서버(npm run dev)에서는 확인하지 않는다
  if (location.port === "1420") return;
  window.setTimeout(() => checkForUpdate(false), 15_000);
}
