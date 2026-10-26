// 자동 업데이트: 공개 배포 저장소(isee-releases)의 latest.json을 확인하고, 서명이 맞는 새 버전만 설치한다.
// 실행 직후에는 확인하지 않고(실행 속도 우선) 15초 뒤에, 하루에 한 번만 조용히 확인한다.
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ask } from "@tauri-apps/plugin-dialog";
import { store } from "./store.svelte";

const LAST_KEY = "isee.updateCheckedAt";
const DAY = 24 * 60 * 60 * 1000;
let busy = false;

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

/** manual: 메뉴에서 누른 경우 (최신이어도 알려 준다) */
export async function checkForUpdate(manual: boolean) {
  if (busy) return;
  if (!manual) {
    try {
      const last = Number(localStorage.getItem(LAST_KEY) ?? 0);
      if (Date.now() - last < DAY) return;
    } catch {
      // 저장소를 못 쓰면 그냥 확인한다
    }
  }
  busy = true;
  try {
    const u = await check({ timeout: 15000 });
    try {
      localStorage.setItem(LAST_KEY, String(Date.now()));
    } catch {
      // 무시
    }
    if (u) await install(u);
    else if (manual) store.showToast("최신 버전입니다");
  } catch (e) {
    // 자동 확인이 실패하면(인터넷 없음 등) 조용히 넘어간다
    if (manual) store.showToast(`업데이트를 확인하지 못했습니다: ${e}`, { error: true });
  } finally {
    busy = false;
  }
}

/** 실행 15초 뒤 한 번 (화면이 다 뜬 다음) */
export function scheduleUpdateCheck() {
  // 개발 서버(npm run dev)에서는 확인하지 않는다
  if (location.port === "1420") return;
  window.setTimeout(() => checkForUpdate(false), 15_000);
}
