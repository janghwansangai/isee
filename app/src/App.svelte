<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { store } from "./lib/store.svelte";
  import {
    api,
    baseName,
    fmtSize,
    isMac,
    mark,
    parentOf,
    startMarks,
    type OutFormat,
    type Settings,
    type Stats,
    type Turn,
  } from "./lib/api";
  import Toolbar from "./components/Toolbar.svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import Grid from "./components/Grid.svelte";
  import InfoPanel from "./components/InfoPanel.svelte";
  import Viewer from "./components/Viewer.svelte";
  import Dialogs from "./components/Dialogs.svelte";
  import Compare from "./components/Compare.svelte";
  import SimilarView from "./components/SimilarView.svelte";
  import OrganizeView from "./components/OrganizeView.svelte";
  import EditorView from "./components/EditorView.svelte";
  import VideoPlayer from "./components/VideoPlayer.svelte";
  import PeopleView from "./components/PeopleView.svelte";
  import BgPanel from "./components/BgPanel.svelte";
  import { checkForUpdate, scheduleUpdateCheck } from "./lib/update";

  let cols = $state(1);
  let viewer = $state<ReturnType<typeof Viewer>>();
  let compareView = $state<ReturnType<typeof Compare>>();
  let similarView = $state<ReturnType<typeof SimilarView>>();
  let organizeView = $state<ReturnType<typeof OrganizeView>>();
  let editorView = $state<ReturnType<typeof EditorView>>();
  let peopleView = $state<ReturnType<typeof PeopleView>>();
  let stats = $state<Stats | null>(null);
  let bgOpen = $state(false);
  /** 백그라운드에서 뭔가 하고 있는지 (상태 표시줄 문구) */
  let bgLabel = $derived.by(() => {
    const b = stats?.bg;
    if (!b) return "";
    const parts: string[] = [];
    if (b.thumbs) parts.push(`썸네일 ${b.thumbs.toLocaleString()}장`);
    if (b.similar[1] && b.similar[0] < b.similar[1]) parts.push(`비슷한 사진 ${b.similar[1] - b.similar[0]}장`);
    if (b.faces[1] && b.faces[0] < b.faces[1]) parts.push(`얼굴 ${b.faces[1] - b.faces[0]}장`);
    if (b.paused) return parts.length ? `⏸ 멈춤 · ${parts.join(" · ")} 남음` : "⏸ 멈춤";
    return parts.length ? `작업 중 · ${parts.join(" · ")} 남음` : "";
  });

  let imageCount = $derived(store.imageIndices.length);
  let dirCount = $derived(store.entries.filter((e) => e.kind === "dir").length);
  let otherCount = $derived(store.entries.length - imageCount - dirCount);
  let selSize = $derived.by(() => {
    let s = 0;
    for (const i of store.selected) s += store.entries[i]?.size ?? 0;
    return s;
  });

  function openEntry(i: number) {
    const e = store.entries[i];
    if (!e) return;
    if (e.kind === "dir") store.open(e.path);
    else if (e.kind === "image") {
      store.selectOnly(i);
      store.viewer = i;
    } else if (e.kind === "video" && /\.(mp4|m4v|mov|webm)$/i.test(e.name)) {
      // 웹뷰가 재생할 수 있는 형식은 앱 안에서, 나머지는 기본 앱으로
      store.selectOnly(i);
      store.video = e;
    } else invoke("open_default", { path: e.path });
  }

  /** 파일이면 그 폴더를 열고 뷰어로, 폴더면 그 폴더를 연다. */
  async function openPath(p: string) {
    if (await invoke<boolean>("is_dir", { path: p })) return store.open(p);
    const parent = parentOf(p);
    if (!parent) return;
    await store.open(parent, { selectPath: p });
    const i = store.entries.findIndex((e) => e.path === p);
    if (i >= 0 && store.entries[i].kind === "image") store.viewer = i;
  }

  async function pickAndOpen() {
    const p = await openDialog({ directory: true, title: "폴더 열기" });
    if (typeof p === "string") store.open(p);
  }

  async function pickDest(copy: boolean) {
    const p = await openDialog({ directory: true, title: copy ? "복사할 폴더" : "옮길 폴더" });
    if (typeof p === "string") store.transferTargets(p, copy);
  }

  function startRename() {
    const paths = store.targets();
    if (paths.length !== 1) return;
    store.dialog = { type: "rename", path: paths[0], name: baseName(paths[0]) };
  }

  function inTextField() {
    const t = document.activeElement as HTMLElement | null;
    return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
  }

  function changeThumbs(delta: number) {
    const v = Math.min(320, Math.max(88, store.settings.thumb_size + delta));
    store.setSettings({ thumb_size: v });
  }

  function goPlace(icon: string) {
    const p = store.places.find((x) => x.icon === icon);
    if (p) store.open(p.path);
  }

  // 비교·비슷한 사진 화면이 덮고 있을 때 메뉴로 받아도 되는 것 (보이지 않는 그리드 선택을 지우거나 옮기지 않게)
  const OVERLAY_MENU_OK = new Set(["undo", "shell_install", "shell_uninstall", "check_update", "bg_panel"]);

  function onMenu(id: string) {
    // 편집기에서는 실행 취소가 편집 단계를 되돌린다
    if (store.editor) {
      if (id === "undo") editorView?.undo();
      return;
    }
    if (store.video) return;
    if ((store.compare || store.similar || store.organize || (store.people && store.viewer < 0)) && !OVERLAY_MENU_OK.has(id)) return;
    if (id.startsWith("ctx:")) return onCtx(id);
    switch (id) {
      case "open_folder":
        return pickAndOpen();
      case "new_folder":
        store.dialog = { type: "newFolder" };
        return;
      case "reveal": {
        const t = store.targets()[0] ?? store.path;
        if (t) api.reveal(t);
        return;
      }
      case "rename":
        return startRename();
      case "trash":
        return store.trashTargets();
      case "undo":
        if (inTextField()) document.execCommand("undo");
        else store.undo();
        return;
      case "select_all":
        if (inTextField()) (document.activeElement as HTMLInputElement).select();
        else if (store.viewer < 0) store.selectAll();
        return;
      case "thumbs_bigger":
        return changeThumbs(16);
      case "thumbs_smaller":
        return changeThumbs(-16);
      case "go_back":
        return store.back();
      case "go_forward":
        return store.forward();
      case "go_up":
        return store.up();
      case "go_home":
        return goPlace("home");
      case "go_pictures":
        return goPlace("pictures");
      case "go_downloads":
        return goPlace("downloads");
      case "quick_folders":
        store.dialog = { type: "quickFolders" };
        return;
      case "compare":
        return store.openCompare();
      case "trash_rejected":
        return store.trashRejected();
      case "similar":
        return store.openSimilar();
      case "organize":
        return store.openOrganize();
      case "edit":
        return store.openEditor();
      case "people":
        return store.openPeople();
      case "bg_panel":
        bgOpen = true;
        return;
      case "check_update":
        return checkForUpdate(true);
      case "shell_install":
      case "shell_uninstall":
        return toggleShell(id === "shell_install");
    }
  }

  /** Finder 빠른 동작(맥) · 탐색기 우클릭 메뉴(윈도우) 넣기/빼기 */
  async function toggleShell(install: boolean) {
    const where = isMac ? "Finder 우클릭 › 빠른 동작" : "탐색기 우클릭 메뉴";
    try {
      const r = await api.shellIntegration(install);
      store.showToast(
        install
          ? `${where}에 iSee 변환 ${r.count}개를 넣었습니다`
          : r.count
            ? `${where}에서 iSee 변환을 뺐습니다`
            : "넣어 둔 iSee 변환이 없습니다",
      );
    } catch (e) {
      store.showToast(`실패: ${e}`, { error: true });
    }
  }

  // 맥에서는 이 단축키들을 메뉴가 처리하므로 화면에서 또 처리하지 않는다
  const MAC_MENU_KEYS = new Set(["z", "a", "o", "n", "[", "]", "ArrowUp", "=", "-"]);

  function onKey(e: KeyboardEvent) {
    if (store.dialog || inTextField()) return;
    const mod = e.metaKey || e.ctrlKey;
    if (store.video) {
      if (e.key === "Escape" || e.key === "Enter") {
        e.preventDefault();
        store.video = null;
      }
      return;
    }
    if (store.editor) {
      // 편집기: ⌘Z는 메뉴로 오므로 여기서는 다시 실행(⇧⌘Z)·도구 키 등
      if (isMac && e.metaKey && e.code === "KeyZ" && !e.shiftKey) return;
      if (editorView?.onKey(e)) e.preventDefault();
      return;
    }
    if (isMac && e.metaKey && MAC_MENU_KEYS.has(e.key)) return;
    // 비교 화면이 열려 있으면 그쪽 키가 먼저 (숫자키 = 남길 사진, Delete = 나머지 삭제)
    if (store.compare && !(mod && e.key.toLowerCase() === "z")) {
      if (compareView?.onKey(e)) e.preventDefault();
      return;
    }
    if (store.similar && !(mod && e.key.toLowerCase() === "z")) {
      if (similarView?.onKey(e)) e.preventDefault();
      return;
    }
    // 인물 화면 위에 뷰어를 띄웠으면 뷰어가 키를 받는다
    if (store.people && store.viewer < 0 && !(mod && e.key.toLowerCase() === "z")) {
      if (peopleView?.onKey(e)) e.preventDefault();
      return;
    }
    if (store.organize && !(mod && e.key.toLowerCase() === "z")) {
      if (organizeView?.onKey(e)) e.preventDefault();
      return;
    }
    if (!isMac && e.ctrlKey) {
      const map: Record<string, () => void> = {
        z: () => store.undo(),
        a: () => store.viewer < 0 && store.selectAll(),
        o: () => pickAndOpen(),
        "=": () => changeThumbs(16),
        "-": () => changeThumbs(-16),
      };
      const fn = map[e.key.toLowerCase()];
      if (fn && !(store.viewer >= 0 && (e.key === "=" || e.key === "-"))) {
        e.preventDefault();
        fn();
        return;
      }
    }
    // ⌘1 썸네일 · ⌘2 목록 (Finder와 같음). 메뉴 단축키로 걸면 뷰어의 ⌘1(실제 크기)을 가로채므로 여기서 처리한다.
    if (mod && store.viewer < 0 && (e.code === "Digit1" || e.code === "Digit2")) {
      e.preventDefault();
      store.setSettings({ view_mode: e.code === "Digit1" ? "grid" : "list" });
      return;
    }
    if (!isMac && e.altKey) {
      const map: Record<string, () => void> = {
        ArrowLeft: () => store.back(),
        ArrowRight: () => store.forward(),
        ArrowUp: () => store.up(),
      };
      if (map[e.key]) {
        e.preventDefault();
        map[e.key]();
        return;
      }
    }

    // 빠른 삭제: Delete, Backspace (Cmd+Backspace 포함). 휴지통으로 가므로 되돌릴 수 있다.
    if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      store.trashTargets();
      return;
    }
    // R: 시계 방향, Shift+R: 반시계 방향 회전 (제자리, 되돌리기 가능)
    if (e.code === "KeyR" && !mod && !e.altKey) {
      e.preventDefault();
      store.runConvert({ turn: e.shiftKey ? "ccw90" : "cw90", save: { mode: "overwrite" } }, "회전");
      return;
    }
    // P 남기기 · X 버리기 · U 표시 지우기 (한글 입력 상태에서도 되도록 물리 키 위치로)
    const flagKey = { KeyP: 1, KeyX: -1, KeyU: 0 }[e.code];
    if (flagKey !== undefined && !mod && !e.altKey) {
      e.preventDefault();
      markAndAdvance(flagKey, undefined);
      return;
    }
    // 빠른 고르기 모드에서는 숫자 0~5가 별점
    const star = e.code.match(/^(?:Digit|Numpad)([0-5])$/);
    if (store.settings.culling && star && !mod && !e.altKey) {
      e.preventDefault();
      markAndAdvance(undefined, Number(star[1]));
      return;
    }
    // C: 고른 사진 비교
    // E: 보고 있는(또는 고른) 사진 편집
    if (e.code === "KeyE" && !mod && !e.altKey) {
      e.preventDefault();
      store.openEditor();
      return;
    }
    if (e.code === "KeyC" && !mod && !e.altKey && store.viewer < 0) {
      e.preventDefault();
      store.openCompare();
      return;
    }
    // 숫자키 1~9: 빠른 이동, Shift는 복사 (키보드 배열과 무관하게 e.code 사용)
    const digit = e.code.match(/^(?:Digit|Numpad)([1-9])$/);
    if (digit && !mod && !e.altKey && !store.settings.culling) {
      e.preventDefault();
      store.quickMove(Number(digit[1]), e.shiftKey);
      return;
    }

    if (store.viewer >= 0) {
      if (viewer?.onKey(e)) e.preventDefault();
      return;
    }

    const n = store.entries.length;
    if (!n) return;
    const f = store.focus < 0 ? 0 : store.focus;
    let nf = f;
    switch (e.key) {
      case "ArrowRight":
        nf = f + 1;
        break;
      case "ArrowLeft":
        nf = f - 1;
        break;
      case "ArrowDown":
        nf = f + cols;
        break;
      case "ArrowUp":
        if (mod) {
          e.preventDefault();
          store.up();
          return;
        }
        nf = f - cols;
        break;
      case "PageDown":
        nf = f + cols * 4;
        break;
      case "PageUp":
        nf = f - cols * 4;
        break;
      case "Home":
        nf = 0;
        break;
      case "End":
        nf = n - 1;
        break;
      case "Enter":
      case " ":
        e.preventDefault();
        openEntry(f);
        return;
      case "F2":
        e.preventDefault();
        startRename();
        return;
      case "Escape":
        store.selected.clear();
        return;
      default:
        return;
    }
    e.preventDefault();
    nf = Math.min(Math.max(nf, 0), n - 1);
    if (e.shiftKey) store.selectRange(nf);
    else store.selectOnly(nf);
    store.scrollTo = nf;
  }

  /** 표시하고, 빠른 고르기 모드면 다음 사진으로 넘어간다 */
  async function markAndAdvance(flag: number | undefined, rating: number | undefined) {
    const paths = store.imageTargets();
    if (!paths.length) return;
    await store.setMarks(paths, flag, rating);
    if (!store.settings.culling || paths.length > 1) return;
    if (store.viewer >= 0) viewer?.go(1);
    else {
      const imgs = store.imageIndices;
      const next = imgs.find((i) => i > store.focus);
      if (next !== undefined) {
        store.selectOnly(next);
        store.scrollTo = next;
      }
    }
  }

  /** 썸네일을 끌어 왼쪽 폴더·경로·폴더 칸에 놓으면 이동 (Option/Alt를 누르면 복사) */
  function onDragStart(e: PointerEvent) {
    const paths = store.selectedPaths();
    if (!paths.length) return;
    const count = paths.length;
    store.dragging = { count, x: e.clientX, y: e.clientY, copy: e.altKey };
    const move = (ev: PointerEvent) => {
      store.dragging = { count, x: ev.clientX, y: ev.clientY, copy: ev.altKey };
      const el = document.elementFromPoint(ev.clientX, ev.clientY)?.closest("[data-drop-path]");
      const p = el?.getAttribute("data-drop-path") ?? null;
      store.dropOver = p && p !== store.path && !paths.includes(p) ? p : null;
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      const dest = store.dropOver;
      store.dragging = null;
      store.dropOver = null;
      if (dest) store.transferTargets(dest, ev.altKey);
    };
    move(e);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up, { once: true });
  }

  /** 우클릭: 엔진이 OS 메뉴를 띄운다 (고른 항목은 "menu" 이벤트로 돌아옴).
   *  버튼을 누를 때 바로 띄우면 웹뷰가 버튼을 뗀 신호를 받지 못해 다음 클릭 한 번이 무시된다.
   *  그래서 버튼을 뗀 다음에 띄운다 (뗀 신호가 오지 않으면 잠시 뒤에). */
  let ctxPending = false;
  let ctxTimer = 0;
  function onContext() {
    ctxPending = true;
    clearTimeout(ctxTimer);
    ctxTimer = window.setTimeout(openContextMenu, 1000);
  }
  function openContextMenu() {
    if (!ctxPending) return;
    ctxPending = false;
    clearTimeout(ctxTimer);
    const paths = store.targets();
    if (!paths.length) return;
    api.contextMenu(store.imageTargets().length, paths.length);
  }

  async function onCtx(id: string) {
    const [, kind, a, b] = id.split(":");
    const save = store.saveSpec();
    switch (kind) {
      case "open":
        return openEntry(store.viewer >= 0 ? store.viewer : store.focus);
      case "compare":
        return store.openCompare();
      case "organize":
        return store.openOrganize();
      case "edit":
        return store.openEditor();
      case "flag":
        return store.setMarks(store.imageTargets(), Number(a));
      case "rate":
        return store.setMarks(store.imageTargets(), undefined, Number(a));
      case "turn":
        // 회전은 항상 제자리에서 (JPEG·HEIC는 화질 손실 없이), 되돌리기 가능
        return store.runConvert({ turn: a as Turn, save: { mode: "overwrite" } }, "회전");
      case "resize":
        if (a === "custom") return askValue("resize");
        return store.runConvert(
          { resize: a === "pct" ? { kind: "percent", value: +b } : { kind: "long_side", value: +b } },
          "크기 변경",
        );
      case "format":
        return store.runConvert({ format: a as OutFormat }, `${a === "jpeg" ? "JPG" : a.toUpperCase()}로 변환`);
      case "size":
        if (a === "custom") return askValue("size");
        return store.runConvert({ max_bytes: +a }, "용량 줄이기");
      case "quality":
        if (a === "custom") return askValue("quality");
        return store.runConvert({ quality: +a }, `화질 ${a}`);
      case "share":
        // 공유용은 원본을 덮어쓰지 않는다
        return store.runConvert(
          {
            resize: { kind: "long_side", value: 2048 },
            format: "jpeg",
            quality: 85,
            strip_gps: true,
            suffix: "공유",
            save: save.mode === "folder" ? save : { mode: "new_file" },
          },
          "공유용 저장",
        );
      case "watermark":
        store.dialog = { type: "watermark", paths: store.imageTargets() };
        return;
      case "batch_rename":
        store.dialog = { type: "batchRename", paths: store.targets() };
        return;
      case "save":
        if (a === "folder") {
          const p = await openDialog({ directory: true, title: "변환한 사진을 저장할 폴더" });
          if (typeof p === "string") store.setSettings({ convert_save: "folder", convert_folder: p });
        }
        return;
      case "quick": {
        const q = store.settings.quick_folders[+a - 1];
        if (q) store.transferTargets(q, false);
        return;
      }
      case "move_to":
        return pickDest(false);
      case "copy_to":
        return pickDest(true);
      case "rename":
        return startRename();
      case "reveal":
        return api.reveal(store.targets()[0]);
      case "copy_path":
        return navigator.clipboard.writeText(store.targets().join("\n"));
      case "trash":
        return store.trashTargets();
    }
  }

  function askValue(kind: "resize" | "size" | "quality") {
    store.dialog = { type: "convertValue", kind, paths: store.imageTargets() };
  }

  function onSettingsChanged(next: Settings) {
    store.applySettings(next);
  }

  onMount(() => {
    (async () => {
      mark("화면 구성");
      const [s, places, pending] = await Promise.all([
        api.getSettings(),
        api.places(),
        api.takePendingOpen(),
      ]);
      store.settings = s;
      store.places = places;
      mark("설정");
      if (pending) await openPath(pending);
      else {
        const start =
          s.last_folder ??
          places.find((p) => p.icon === "pictures")?.path ??
          places[0]?.path;
        if (start) await store.open(start);
        if (store.error && places[0]) await store.open(places[0].path);
      }
      mark("폴더 열림");
      requestAnimationFrame(() =>
        requestAnimationFrame(async () => {
          mark("그리기");
          store.launchMs = await api.frontendReady(startMarks);
          scheduleUpdateCheck();
        }),
      );
    })();

    const timer = setInterval(async () => {
      stats = await api.stats();
    }, 700);
    const unlisten = [
      listen<{ path: string; at: number }>("open-path", async (e) => {
        store.openRequestAt = e.payload.at;
        await api.takePendingOpen();
        openPath(e.payload.path);
      }),
      listen<string>("menu", (e) => onMenu(e.payload)),
      listen<[number, number]>("convert-progress", (e) => {
        if (store.progress) store.progress = { ...store.progress, done: e.payload[0], total: e.payload[1] };
      }),
      listen<Settings>("settings-changed", (e) => onSettingsChanged(e.payload)),
      getCurrentWebview().onDragDropEvent((e) => {
        if (e.payload.type === "drop" && e.payload.paths[0]) openPath(e.payload.paths[0]);
      }),
    ];
    return () => {
      clearInterval(timer);
      unlisten.forEach((p) => p.then((f) => f()));
    };
  });
</script>

<svelte:window onkeydown={onKey} onpointerup={() => ctxPending && openContextMenu()} />

<div class="app" class:info={store.settings.show_info}>
  <div class="toolbar"><Toolbar /></div>
  <div class="side"><Sidebar /></div>
  <main class="main">
    <Grid bind:cols onOpen={openEntry} onContext={() => onContext()} {onDragStart} />
  </main>
  {#if store.settings.show_info}
    <div class="info"><InfoPanel /></div>
  {/if}
  <footer class="status">
    <span>사진 {imageCount.toLocaleString()}장</span>
    {#if dirCount}<span>폴더 {dirCount}개</span>{/if}
    {#if otherCount}<span>기타 {otherCount}개</span>{/if}
    {#if store.selected.size > 1}<span>{store.selected.size}개 선택 · {fmtSize(selSize)}</span>{/if}
    <span class="spacer"></span>
    <button class="bg-chip" class:busy={!!bgLabel} onclick={() => (bgOpen = !bgOpen)} title="백그라운드 작업 보기·켜기·끄기">
      {bgLabel || "백그라운드 작업"}{stats?.bg && !stats.bg.prefill ? " · 미리 만들기 꺼짐" : ""}
    </button>
    {#if store.settings.show_hud}
      <span class="hud">
        목록 {store.listMs.toFixed(1)}ms · 실행→첫 화면 {store.launchMs.toFixed(0)}ms
        {#if stats} · 썸네일 생성 평균 {stats.avg_ms.toFixed(1)}ms ({stats.generated}장) · 캐시 {stats.mem_hits + stats.disk_hits}{/if}
      </span>
    {/if}
  </footer>
</div>

{#if store.viewer >= 0}
  <div
    oncontextmenu={(e) => {
      e.preventDefault();
      onContext();
    }}
    role="presentation"
  >
    <Viewer bind:this={viewer} />
  </div>
{/if}

{#if store.similar}
  <SimilarView bind:this={similarView} />
{/if}

{#if store.organize}
  <OrganizeView bind:this={organizeView} />
{/if}

{#if store.people}
  <div class="people-layer" class:under={store.viewer >= 0}>
    <PeopleView bind:this={peopleView} />
  </div>
{/if}

{#if store.compare}
  <Compare bind:this={compareView} />
{/if}

{#if store.editor}
  <EditorView bind:this={editorView} />
{/if}

{#if store.video}
  <VideoPlayer />
{/if}

{#if bgOpen}
  <BgPanel bg={stats?.bg ?? null} onclose={() => (bgOpen = false)} />
{/if}

<Dialogs />

{#if store.dragging}
  <div class="ghost" style="left:{store.dragging.x + 14}px;top:{store.dragging.y + 10}px">
    {store.dragging.count}개 {store.dragging.copy ? "복사" : "이동"}
    {#if store.dropOver}→ {baseName(store.dropOver)}{/if}
  </div>
{/if}

{#if store.progress}
  <div class="toast progress" role="status">
    {store.progress.label} 중 · {store.progress.done} / {store.progress.total}
    <span class="bar"><span style="width:{(store.progress.done / Math.max(1, store.progress.total)) * 100}%"></span></span>
  </div>
{:else if store.toast}
  {#key store.toast.id}
    <div class="toast" class:error={store.toast.error} role="status">
      {store.toast.message}
      {#if store.toast.undo}
        <button onclick={() => store.undo()}>되돌리기 <kbd>{isMac ? "⌘Z" : "Ctrl+Z"}</kbd></button>
      {/if}
    </div>
  {/key}
{/if}

<style>
  .app {
    display: grid;
    grid-template-columns: 230px 1fr;
    grid-template-rows: 40px 1fr 26px;
    grid-template-areas:
      "toolbar toolbar"
      "side main"
      "status status";
    height: 100vh;
  }
  .app.info {
    grid-template-columns: 230px 1fr 260px;
    grid-template-areas:
      "toolbar toolbar toolbar"
      "side main info"
      "status status status";
  }
  .toolbar {
    grid-area: toolbar;
  }
  .side {
    grid-area: side;
    min-height: 0;
  }
  .main {
    grid-area: main;
    min-height: 0;
    min-width: 0;
  }
  .info {
    grid-area: info;
    min-height: 0;
  }
  .status {
    grid-area: status;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 12px;
    font-size: 12px;
    color: var(--text-faint);
    background: var(--panel);
    border-top: 1px solid var(--border);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
  }
  .spacer {
    flex: 1;
  }
  .bg-chip {
    padding: 0 6px;
    border-radius: 4px;
    color: var(--text-faint);
  }
  .bg-chip:hover {
    background: var(--hover);
    color: var(--text);
  }
  .busy {
    color: var(--text-dim);
  }
  .hud {
    color: #7fd8a4;
  }
  .ghost {
    position: fixed;
    z-index: 200;
    pointer-events: none;
    padding: 4px 10px;
    border-radius: 12px;
    background: var(--accent);
    color: white;
    font-size: 12px;
    font-weight: 600;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
  }
  .toast {
    position: fixed;
    z-index: 150;
    left: 50%;
    bottom: 40px;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 9px 14px;
    border-radius: 8px;
    background: #2f3037;
    border: 1px solid var(--border);
    box-shadow: 0 8px 30px rgba(0, 0, 0, 0.45);
    max-width: min(640px, calc(100vw - 32px));
    animation: rise 0.12s ease-out;
  }
  .toast.progress {
    gap: 12px;
    font-variant-numeric: tabular-nums;
  }
  .bar {
    width: 140px;
    height: 4px;
    border-radius: 2px;
    background: var(--border);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.1s;
  }
  .toast.error {
    border-color: rgba(255, 100, 100, 0.5);
  }
  .toast button {
    color: var(--accent);
    font-weight: 600;
    white-space: nowrap;
  }
  kbd {
    font-family: inherit;
    font-size: 11px;
    color: var(--text-faint);
    margin-left: 4px;
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translate(-50%, 6px);
    }
  }
  /* 인물 화면에서 사진을 크게 볼 때는 뷰어 아래로 숨긴다 (닫으면 다시 인물 화면) */
  .people-layer.under {
    display: none;
  }
</style>
