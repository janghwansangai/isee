<script lang="ts">
  import { store } from "../lib/store.svelte";
  import { api, thumbUrl, fmtSize } from "../lib/api";
  import { boxFor, drawFrame, hasFrame, loadFrame as load, pin, trimFrames, unpin, type Frame } from "../lib/frames";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Icon from "./Icon.svelte";

  let stage: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let vw = $state(0);
  let vh = $state(0);
  let frame = $state.raw<Frame | null>(null);
  let framePath = $state("");
  let failed = $state(false);
  let zoom = $state(1); // 1 = 화면에 맞춤
  let panX = $state(0);
  let panY = $state(0);
  let hud = $state({ total: 0, cached: false });
  let fullscreen = $state(false);

  let entry = $derived(store.entries[store.viewer]);
  let imgs = $derived(store.imageIndices);
  let pos = $derived(imgs.indexOf(store.viewer));
  let dpr = $derived(window.devicePixelRatio || 1);

  function box(): [number, number] {
    return boxFor(vw, vh, dpr);
  }

  /** 화면에 그린 사진은 캐시에서 버리지 않게 붙잡아 둔다 */
  function setFrame(f: Frame | null) {
    unpin(frame);
    pin(f);
    frame = f;
  }

  let token = 0;
  async function show() {
    const e = entry;
    if (!e || !vw) return;
    const my = ++token;
    const t0 = performance.now();
    const b = box();
    const cached = hasFrame(e.path, e.mtime, b);
    failed = false;
    const f = await load(e.path, e.mtime, b);
    if (my !== token) return; // 그사이 다른 사진으로 넘어감
    if (!f) {
      failed = true;
      return;
    }
    zoom = 1;
    panX = 0;
    panY = 0;
    setFrame(f);
    framePath = e.path;
    draw(f);
    requestAnimationFrame(() => {
      if (my !== token) return;
      if (store.openRequestAt) {
        store.lastOpenMs = Date.now() - store.openRequestAt;
        store.openRequestAt = 0;
        api.perfLog("열기 요청→사진 표시", store.lastOpenMs);
      }
      hud = { total: performance.now() - t0, cached };
      prefetch(b);
    });
  }

  function draw(f: Frame) {
    drawFrame(canvas, f);
  }

  /** 다음 2장과 이전 1장을 미리 준비 */
  function prefetch(b: [number, number]) {
    for (const d of [1, 2, -1]) {
      const i = imgs[pos + d];
      if (i !== undefined) load(store.entries[i].path, store.entries[i].mtime, b);
    }
  }

  // 사진이 바뀌면 바로, 창 크기만 바뀌면 크기 조절이 멈춘 뒤 한 번 다시 그린다
  let shownKey = "";
  let resizeTimer = 0;
  $effect(() => {
    const key = entry ? `${entry.path}|${entry.mtime}` : "";
    void vw;
    void vh;
    clearTimeout(resizeTimer);
    if (key !== shownKey) {
      shownKey = key;
      show();
    } else {
      resizeTimer = window.setTimeout(show, 150);
    }
  });

  // 확대해서 준비된 해상도보다 크게 보이면 원본 해상도로 다시 받는다
  let upgrading = "";
  $effect(() => {
    const f = frame;
    if (!f || !f.bmp || f.w >= f.fw) return;
    const shownPx = dispW * dpr;
    const mtime = entry?.mtime ?? 0;
    if (shownPx > f.w * 1.05 && upgrading !== `${framePath}|${mtime}`) {
      upgrading = `${framePath}|${mtime}`;
      const path = framePath;
      load(path, mtime, [0, 0]).then((full) => {
        if (full && path === framePath && entry?.path === path) {
          setFrame(full);
          draw(full);
        }
      });
    }
  });

  // 화면에 맞춘 배율 (원본 1픽셀 = 화면 1픽셀을 1로 볼 때). 작은 사진은 1포인트당 1픽셀까지만 키운다.
  let fitScale = $derived(
    frame ? Math.min(vw / (frame.fw / dpr), vh / (frame.fh / dpr), dpr) : 1,
  );
  let scale = $derived(fitScale * zoom);
  let dispW = $derived(frame ? (frame.fw / dpr) * scale : 0);
  let dispH = $derived(frame ? (frame.fh / dpr) * scale : 0);
  let percent = $derived(Math.round(scale * 100));

  function zoomAt(factor: number, cx = vw / 2, cy = vh / 2) {
    const next = Math.min(Math.max(zoom * factor, 0.1), 64 / Math.max(fitScale, 0.01));
    const k = next / zoom;
    // 커서 아래 지점이 그대로 있도록 이동량 보정 (화면 중심 기준 좌표)
    const ox = cx - vw / 2;
    const oy = cy - vh / 2;
    panX = ox - (ox - panX) * k;
    panY = oy - (oy - panY) * k;
    zoom = next;
    if (zoom <= 1) {
      panX = 0;
      panY = 0;
    }
  }

  function fit() {
    zoom = 1;
    panX = 0;
    panY = 0;
  }

  function actualSize(cx = vw / 2, cy = vh / 2) {
    zoomAt(1 / fitScale / zoom, cx, cy);
  }

  export function go(delta: number) {
    if (!imgs.length) return;
    const np = Math.min(Math.max(pos + delta, 0), imgs.length - 1);
    if (np === pos) return;
    store.viewer = imgs[np];
    store.selectOnly(store.viewer);
    store.scrollTo = store.viewer;
  }

  export function goTo(p: number) {
    if (!imgs.length) return;
    store.viewer = imgs[Math.min(Math.max(p, 0), imgs.length - 1)];
    store.selectOnly(store.viewer);
    store.scrollTo = store.viewer;
  }

  export async function close() {
    if (fullscreen) await toggleFullscreen();
    store.viewer = -1;
  }

  async function toggleFullscreen() {
    const w = getCurrentWindow();
    fullscreen = !(await w.isFullscreen());
    await w.setFullscreen(fullscreen);
  }

  /** 뷰어가 열려 있을 때의 키 (App에서 넘겨줌). 처리했으면 true. */
  export function onKey(e: KeyboardEvent): boolean {
    const mod = e.metaKey || e.ctrlKey;
    // 한글 입력 상태에서는 e.key가 자모("ㄹ")로 오므로 글자 키는 물리 키 위치(e.code)로 본다
    if (e.code === "KeyF" && !mod) {
      toggleFullscreen();
      return true;
    }
    switch (e.key) {
      case "ArrowRight":
      case "ArrowDown":
      case "PageDown":
        go(1);
        return true;
      case "ArrowLeft":
      case "ArrowUp":
      case "PageUp":
        go(-1);
        return true;
      case " ":
        go(e.shiftKey ? -1 : 1);
        return true;
      case "Home":
        goTo(0);
        return true;
      case "End":
        goTo(imgs.length - 1);
        return true;
      case "Escape":
      case "Enter":
        close();
        return true;
      case "F11":
        toggleFullscreen();
        return true;
      case "+":
      case "=":
        zoomAt(1.25);
        return true;
      case "-":
        zoomAt(0.8);
        return true;
      case "0":
        if (!mod && !e.shiftKey) {
          fit();
          return true;
        }
        if (mod) {
          fit();
          return true;
        }
        return false;
      case "1":
        if (mod) {
          actualSize();
          return true;
        }
        return false;
    }
    return false;
  }

  let wheelAcc = 0;
  let wheelAt = 0;
  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const r = stage.getBoundingClientRect();
    if (e.ctrlKey || e.metaKey) {
      // 트랙패드 핀치 또는 Ctrl+휠: 커서 위치 기준 확대
      zoomAt(Math.exp(-e.deltaY * 0.01), e.clientX - r.left, e.clientY - r.top);
    } else if (zoom > 1.001) {
      panX -= e.deltaX;
      panY -= e.deltaY;
    } else {
      // 휠: 다음·이전 사진. 트랙패드의 연속 스크롤은 모아서 한 장씩 넘긴다.
      const now = performance.now();
      if (now - wheelAt > 250) wheelAcc = 0;
      wheelAt = now;
      wheelAcc += Math.abs(e.deltaY) > Math.abs(e.deltaX) ? e.deltaY : e.deltaX;
      if (Math.abs(wheelAcc) >= 60) {
        go(Math.sign(wheelAcc));
        wheelAcc = 0;
        wheelAt = now + 150; // 관성 스크롤로 여러 장 넘어가지 않게
      }
    }
  }

  let drag: { x: number; y: number; px: number; py: number } | null = null;
  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0 || zoom <= 1.001) return;
    drag = { x: e.clientX, y: e.clientY, px: panX, py: panY };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: PointerEvent) {
    if (!drag) return;
    panX = drag.px + e.clientX - drag.x;
    panY = drag.py + e.clientY - drag.y;
  }
  function onDblClick(e: MouseEvent) {
    const r = stage.getBoundingClientRect();
    if (zoom > 1.001) fit();
    else actualSize(e.clientX - r.left, e.clientY - r.top);
  }

  $effect(() => {
    api.setViewerActive(true);
    return () => {
      api.setViewerActive(false);
      unpin(frame);
      trimFrames(4);
    };
  });

  let showingFrame = $derived(!!frame && framePath === entry?.path);
</script>

<div class="viewer" class:full={fullscreen}>
  <div
    class="stage"
    bind:this={stage}
    bind:clientWidth={vw}
    bind:clientHeight={vh}
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={() => (drag = null)}
    ondblclick={onDblClick}
    class:grab={zoom > 1.001}
    role="img"
    aria-label={entry?.name}
  >
    {#if entry && !showingFrame}
      <!-- 디코딩이 끝날 때까지 캐시된 썸네일을 크게 보여준다 (즉시 표시) -->
      <img class="placeholder" src={thumbUrl(entry, store.viewer)} alt="" draggable="false" />
    {/if}
    <canvas
      bind:this={canvas}
      class:hidden={!showingFrame || !frame?.bmp}
      style="width:{dispW}px;height:{dispH}px;transform:translate(-50%,-50%) translate({panX}px,{panY}px)"
    ></canvas>
    {#if showingFrame && frame?.blobUrl}
      <img
        class="gif"
        src={frame.blobUrl}
        alt=""
        draggable="false"
        style="width:{dispW}px;height:{dispH}px;transform:translate(-50%,-50%) translate({panX}px,{panY}px)"
      />
    {/if}
    {#if failed}
      <div class="failed">이 사진을 열 수 없습니다</div>
    {/if}
  </div>

  <div class="topbar">
    <span class="name">{entry?.name}</span>
    <span class="dim">{pos + 1} / {imgs.length}</span>
    {#if frame && showingFrame}
      <span class="dim">{frame.fw}×{frame.fh}</span>
    {/if}
    {#if entry}<span class="dim">{fmtSize(entry.size)}</span>{/if}
    <span class="dim">{percent}%</span>
    <span class="spacer"></span>
    <button class="icon-btn" title="휴지통으로 이동 (Delete)" onclick={() => store.trashTargets()}>
      <Icon name="trash" />
    </button>
    <button class="icon-btn" title="닫기 (Esc)" onclick={close}><Icon name="close" /></button>
  </div>

  <button class="nav prev" onclick={() => go(-1)} disabled={pos <= 0} aria-label="이전 사진">
    <Icon name="back" size={28} />
  </button>
  <button class="nav next" onclick={() => go(1)} disabled={pos >= imgs.length - 1} aria-label="다음 사진">
    <Icon name="forward" size={28} />
  </button>

  {#if store.settings.culling && entry}
    {@const m = store.marks[entry.path]}
    {@const c = store.cullCounts()}
    <div class="cullbar">
      <span class="state" class:pick={m?.flag === 1} class:reject={m?.flag === -1}>
        {m?.flag === 1 ? "✓ 남김" : m?.flag === -1 ? "✕ 버림" : "미정"}
      </span>
      <span class="stars">{m?.rating ? "★".repeat(m.rating) : "☆ 별점 없음"}</span>
      <span class="keys">P 남기기 · X 버리기 · U 지우기 · 1–5 별점</span>
      <span class="counts">남김 {c.picked} · 버림 {c.rejected} · 미정 {c.unflagged}</span>
    </div>
  {/if}

  {#if store.settings.show_hud && frame}
    <div class="hud">
      {#if hud.cached}
        <b>미리 불러옴</b> · 화면 표시까지 {hud.total.toFixed(1)}ms
      {:else}
        엔진 {frame.prepMs.toFixed(0)}ms · 전송 {Math.max(0, frame.fetchMs - frame.prepMs).toFixed(0)}ms · 비트맵 {frame.bitmapMs.toFixed(0)}ms ·
        <b>화면 표시까지 {hud.total.toFixed(0)}ms</b>
      {/if}
      · {frame.w}×{frame.h} ({frame.route})
      {#if store.lastOpenMs}· 열기 요청→표시 {store.lastOpenMs}ms{/if}
    </div>
  {/if}
</div>

<style>
  .viewer {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: #0d0d0f;
  }
  .stage {
    position: absolute;
    inset: 0;
    overflow: hidden;
  }
  .stage.grab {
    cursor: grab;
  }
  canvas,
  .gif {
    position: absolute;
    left: 50%;
    top: 50%;
    image-rendering: auto;
  }
  canvas.hidden {
    visibility: hidden;
  }
  .placeholder {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    filter: blur(0.5px);
  }
  .topbar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 8px 12px 8px 16px;
    background: linear-gradient(rgba(0, 0, 0, 0.55), transparent);
    opacity: 0;
    transition: opacity 0.15s;
  }
  .viewer:hover .topbar {
    opacity: 1;
  }
  .name {
    font-weight: 600;
  }
  .dim {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .spacer {
    flex: 1;
  }
  .nav {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 48px;
    height: 96px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: rgba(255, 255, 255, 0.7);
    border-radius: 8px;
    opacity: 0;
    transition: opacity 0.15s;
  }
  .viewer:hover .nav:not(:disabled) {
    opacity: 1;
  }
  .nav:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .prev {
    left: 8px;
  }
  .next {
    right: 8px;
  }
  .cullbar {
    position: absolute;
    left: 50%;
    bottom: 14px;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 7px 14px;
    border-radius: 10px;
    background: rgba(20, 20, 24, 0.85);
    border: 1px solid #2a2b31;
    font-size: 12px;
    white-space: nowrap;
  }
  .cullbar .state {
    font-weight: 700;
    color: var(--text-dim);
  }
  .cullbar .state.pick {
    color: var(--ok);
  }
  .cullbar .state.reject {
    color: #ff7a7a;
  }
  .cullbar .stars {
    color: #f5c451;
  }
  .cullbar .keys,
  .cullbar .counts {
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }
  .hud {
    position: absolute;
    left: 12px;
    bottom: 12px;
    padding: 5px 10px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.65);
    font-size: 12px;
    color: #cfd0d6;
    font-variant-numeric: tabular-nums;
  }
  .hud b {
    color: #7fd8a4;
    font-weight: 600;
  }
  .failed {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-dim);
  }
</style>
