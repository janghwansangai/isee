<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";
  import { store } from "../lib/store.svelte";
  import { api, baseName, fmtSize, isMac, thumbUrl } from "../lib/api";
  import { boxFor, drawFrame, loadFrame, pin, trimFrames, unpin, type Frame } from "../lib/frames";
  import Icon from "./Icon.svelte";

  const GAP = 4;

  // 비교할 사진 (최대 9장). 폴더를 다시 읽어도 경로로 다시 찾는다.
  let items = $derived(
    (store.compare ?? [])
      .map((path) => {
        const index = store.entries.findIndex((e) => e.path === path);
        return { path, index, entry: store.entries[index] ?? store.compareExtra[path] };
      })
      .filter((x) => x.entry),
  );
  let n = $derived(items.length);
  /** 2장일 때만: 나란히 / 겹쳐 보기 / 깜빡이기 */
  let mode = $state<"grid" | "overlay" | "flicker">("grid");
  let kept = new SvelteSet<string>(store.compareKeep);
  let opacity = $state(50);
  let flickB = $state(false);
  let autoFlick = $state(true);

  let stage = $state<HTMLDivElement>();
  let vw = $state(0);
  let vh = $state(0);
  let dpr = $derived(window.devicePixelRatio || 1);
  // 모든 칸이 함께 쓰는 확대 상태: zoom 1 = 칸에 맞춤, (cx, cy) = 칸 가운데에 올 사진 속 위치 (0~1)
  let zoom = $state(1);
  let cx = $state(0.5);
  let cy = $state(0.5);

  /** 경로 → 받은 사진 (칸을 빼도 어긋나지 않게 경로로 찾는다) */
  let frames = $state.raw<Record<string, Frame>>({});
  let canvases = $state<(HTMLCanvasElement | undefined)[]>([]);
  const drawn = new WeakMap<HTMLCanvasElement, Frame>();
  const frameOf = (i: number): Frame | null => frames[items[i]?.path] ?? null;

  let single = $derived(mode !== "grid" && n === 2);
  let aspect = $derived.by(() => {
    const known = Object.values(frames);
    if (!known.length) return 1.5;
    return known.reduce((a, f) => a + f.fw / f.fh, 0) / known.length;
  });

  /** 칸 배치: 사진이 화면을 가장 넓게 쓰는 열 수를 고른다 */
  let grid = $derived.by(() => {
    if (single || n <= 1) return { cols: 1, rows: 1 };
    let best = { cols: 1, rows: n, area: 0 };
    for (let cols = 1; cols <= n; cols++) {
      const rows = Math.ceil(n / cols);
      const pw = (vw - GAP * (cols - 1)) / cols;
      const ph = (vh - GAP * (rows - 1)) / rows;
      const h = Math.min(pw / aspect, ph);
      const area = h * h * aspect * n;
      if (area > best.area) best = { cols, rows, area };
    }
    return best;
  });
  let pw = $derived(single ? vw : (vw - GAP * (grid.cols - 1)) / grid.cols);
  let ph = $derived(single ? vh : (vh - GAP * (grid.rows - 1)) / grid.rows);

  // 칸 크기에 맞춰 디코딩한 사진을 받는다 (미리 받은 것은 즉시)
  let loadToken = 0;
  $effect(() => {
    const list = items;
    const box = boxFor(pw, ph, dpr);
    if (!pw || !ph) return;
    const my = ++loadToken;
    list.forEach((it) => {
      loadFrame(it.path, it.entry.mtime, box).then((f) => {
        if (my === loadToken && f) setFrame(it.path, f);
      });
    });
  });

  function setFrame(path: string, f: Frame) {
    unpin(frames[path]);
    pin(f);
    frames = { ...frames, [path]: f };
  }

  // 받은 사진을 칸의 canvas에 그린다 (그 칸에 마지막으로 그린 것과 다를 때만)
  $effect(() => {
    items.forEach((_, i) => {
      const f = frameOf(i);
      const c = canvases[i];
      if (f && c && drawn.get(c) !== f) {
        drawFrame(c, f);
        drawn.set(c, f);
      }
    });
  });

  /** 칸 안에서의 사진 표시 크기 (원본 1픽셀 = 화면 1픽셀을 기준으로, 작은 사진은 1포인트당 1픽셀까지만) */
  function fitScale(f: Frame) {
    return Math.min(pw / (f.fw / dpr), ph / (f.fh / dpr), dpr);
  }
  function rect(f: Frame | null) {
    if (!f) return null;
    const s = fitScale(f) * zoom;
    const dw = (f.fw / dpr) * s;
    const dh = (f.fh / dpr) * s;
    return { dw, dh, x: pw / 2 - cx * dw, y: ph / 2 - cy * dh };
  }

  // 확대해서 받은 해상도보다 크게 보이면 그 칸만 원본 해상도로 다시 받는다
  let upgrading = new Set<string>();
  $effect(() => {
    items.forEach((it, i) => {
      const f = frameOf(i);
      const r = rect(f);
      if (!f || !r || !it || f.w >= f.fw) return;
      const key = `${it.path}|${it.entry.mtime}`;
      if (r.dw * dpr > f.w * 1.05 && !upgrading.has(key)) {
        upgrading.add(key);
        loadFrame(it.path, it.entry.mtime, [0, 0]).then((full) => {
          if (full) setFrame(it.path, full);
        });
      }
    });
  });

  /** 칸 i 안의 (mx, my) 지점을 기준으로 확대 (모든 칸이 같은 위치·배율) */
  function zoomAt(factor: number, i: number, mx = pw / 2, my = ph / 2) {
    const f = frameOf(i) ?? Object.values(frames)[0] ?? null;
    const r = rect(f);
    if (!f || !r) return;
    const next = Math.min(Math.max(zoom * factor, 1), 64 / fitScale(f));
    const u = (mx - r.x) / r.dw;
    const v = (my - r.y) / r.dh;
    const dw2 = (r.dw * next) / zoom;
    const dh2 = (r.dh * next) / zoom;
    zoom = next;
    if (zoom <= 1.0001) {
      cx = 0.5;
      cy = 0.5;
    } else {
      // 커서 아래의 사진 속 지점(u, v)이 그대로 있도록
      cx = u - (mx - pw / 2) / dw2;
      cy = v - (my - ph / 2) / dh2;
    }
  }

  function fit() {
    zoom = 1;
    cx = 0.5;
    cy = 0.5;
  }

  // 포인터를 붙잡고 있으면 이벤트 target이 비교 화면 전체가 되므로, 좌표로 칸을 찾는다
  function paneAt(e: MouseEvent): { i: number; mx: number; my: number } | null {
    const el = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>("[data-pane]");
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { i: Number(el.dataset.pane), mx: e.clientX - r.left, my: e.clientY - r.top };
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const p = paneAt(e);
    if (!p) return;
    if (e.ctrlKey || e.metaKey) {
      zoomAt(Math.exp(-e.deltaY * 0.01), p.i, p.mx, p.my);
    } else if (zoom > 1.0001) {
      const r = rect(frameOf(p.i));
      if (!r) return;
      cx += e.deltaX / r.dw;
      cy += e.deltaY / r.dh;
    }
  }

  let drag: { x: number; y: number; cx: number; cy: number; i: number; moved: boolean } | null = null;
  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const p = paneAt(e);
    if (!p || (e.target as HTMLElement).closest("button")) return;
    drag = { x: e.clientX, y: e.clientY, cx, cy, i: p.i, moved: false };
    stage?.setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: PointerEvent) {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (Math.hypot(dx, dy) > 4) drag.moved = true;
    if (zoom > 1.0001 && drag.moved) {
      const r = rect(frameOf(drag.i));
      if (!r) return;
      cx = drag.cx - dx / r.dw;
      cy = drag.cy - dy / r.dh;
    }
  }
  function onPointerUp() {
    // 끌지 않고 놓으면 그 칸을 "남김"으로 표시하거나 해제
    if (drag && !drag.moved && !single) toggle(items[drag.i]?.path);
    drag = null;
  }
  function onDblClick(e: MouseEvent) {
    const p = paneAt(e);
    if (!p) return;
    if (zoom > 1.0001) fit();
    else {
      const f = frameOf(p.i);
      if (f) zoomAt(1 / fitScale(f), p.i, p.mx, p.my);
    }
  }

  function toggle(path: string | undefined) {
    if (!path) return;
    if (kept.has(path)) kept.delete(path);
    else kept.add(path);
  }

  function remove(path: string) {
    store.compare = (store.compare ?? []).filter((p) => p !== path);
    kept.delete(path);
    if ((store.compare?.length ?? 0) < 2) close();
  }

  export function close() {
    // 남김으로 표시한 사진을 그리드에서 선택해 둔다
    const keep = [...kept];
    store.compare = null;
    if (keep.length) {
      store.selected.clear();
      store.entries.forEach((e, i) => keep.includes(e.path) && store.selected.add(i));
    }
  }

  /** 남김으로 표시하지 않은 사진을 모두 휴지통으로 (되돌리기 가능) */
  async function deleteOthers() {
    const others = items.map((it) => it.path).filter((p) => !kept.has(p));
    if (!kept.size) {
      store.showToast("남길 사진을 먼저 고르세요 (클릭하거나 숫자키)");
      return;
    }
    if (!others.length) return;
    const keep = [...kept];
    const r = await api.trash(others);
    store.compare = null;
    store.showToast(`${r.ok.length}장을 휴지통으로 보내고 ${keep.length}장을 남겼습니다`, { undo: true, error: r.failed.length > 0 });
    await store.reload();
    store.selected.clear();
    store.entries.forEach((e, i) => keep.includes(e.path) && store.selected.add(i));
  }

  /** App에서 넘겨주는 키. 처리했으면 true */
  export function onKey(e: KeyboardEvent): boolean {
    const mod = e.metaKey || e.ctrlKey;
    const digit = e.code.match(/^(?:Digit|Numpad)([1-9])$/);
    if (digit && !mod) {
      toggle(items[Number(digit[1]) - 1]?.path);
      return true;
    }
    switch (e.key) {
      case "Escape":
      case "Enter":
        close();
        return true;
      case "Delete":
      case "Backspace":
        deleteOthers();
        return true;
      case "+":
      case "=":
        zoomAt(1.25, 0);
        return true;
      case "-":
        zoomAt(0.8, 0);
        return true;
      case "0":
        fit();
        return true;
      case " ":
        if (mode === "flicker") {
          autoFlick = false;
          flickB = !flickB;
          return true;
        }
        return false;
    }
    if (e.code === "KeyO" && n === 2 && !mod) {
      mode = mode === "overlay" ? "grid" : "overlay";
      return true;
    }
    return false;
  }

  // 깜빡이기: 0.5초마다 두 장을 번갈아
  $effect(() => {
    if (mode !== "flicker" || !autoFlick) return;
    const t = setInterval(() => (flickB = !flickB), 500);
    return () => clearInterval(t);
  });

  $effect(() => {
    api.setViewerActive(true);
    return () => {
      api.setViewerActive(false);
      Object.values(frames).forEach(unpin);
      trimFrames(4);
    };
  });

  let keptCount = $derived(items.filter((it) => kept.has(it.path)).length);
</script>

<div class="compare" oncontextmenu={(e) => e.preventDefault()} role="presentation">
  <div class="topbar">
    <span class="title">비교 {n}장</span>
    <span class="dim">남길 사진 {keptCount}장</span>
    {#if n === 2}
      <div class="modes" role="group" aria-label="보기 방식">
        <button class:on={mode === "grid"} onclick={() => (mode = "grid")}>나란히</button>
        <button class:on={mode === "overlay"} onclick={() => (mode = "overlay")}>겹쳐 보기</button>
        <button class:on={mode === "flicker"} onclick={() => { mode = "flicker"; autoFlick = true; }}>깜빡이기</button>
      </div>
      {#if mode === "overlay"}
        <label class="opacity">위 사진 <input type="range" min="0" max="100" bind:value={opacity} /> {opacity}%</label>
      {/if}
    {/if}
    <span class="spacer"></span>
    <span class="hint">클릭·숫자키 남김 · 핀치/{isMac ? "⌘" : "Ctrl"}+휠 함께 확대 · 더블클릭 100%</span>
    <button class="btn danger" disabled={!keptCount || keptCount === n} onclick={deleteOthers}>
      선택 외 삭제{keptCount && keptCount < n ? ` (${n - keptCount}장)` : ""}
    </button>
    <button class="icon-btn" title="닫기 (Esc)" onclick={close}><Icon name="close" /></button>
  </div>

  <div
    class="stage"
    bind:this={stage}
    bind:clientWidth={vw}
    bind:clientHeight={vh}
    style="grid-template-columns:repeat({single ? 1 : grid.cols}, 1fr);grid-template-rows:repeat({single ? 1 : grid.rows}, 1fr);gap:{GAP}px"
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    ondblclick={onDblClick}
    role="presentation"
  >
    {#if single}
      <!-- 두 장을 같은 자리에 겹쳐서 -->
      <div class="pane" data-pane="0">
        {#each items as it, i (it.path)}
          {@const r = rect(frameOf(i))}
          <canvas
            bind:this={canvases[i]}
            class:hidden={!r}
            style={r
              ? `width:${r.dw}px;height:${r.dh}px;transform:translate(${r.x}px,${r.y}px);opacity:${
                  i === 0 ? 1 : mode === "overlay" ? opacity / 100 : flickB ? 1 : 0
                }`
              : ""}
          ></canvas>
        {/each}
        <div class="badge">
          {mode === "flicker" ? (flickB ? "2" : "1") : "1 + 2"} · {baseName(items[mode === "flicker" && flickB ? 1 : 0]?.path ?? "")}
        </div>
      </div>
    {:else}
      {#each items as it, i (it.path)}
        {@const f = frameOf(i)}
        {@const r = rect(f)}
        <div class="pane" class:kept={kept.has(it.path)} data-pane={i}>
          {#if !r}
            <img class="placeholder" src={thumbUrl(it.entry, it.index)} alt="" draggable="false" />
          {/if}
          <canvas
            bind:this={canvases[i]}
            class:hidden={!r}
            style={r ? `width:${r.dw}px;height:${r.dh}px;transform:translate(${r.x}px,${r.y}px)` : ""}
          ></canvas>
          <div class="head">
            <span class="num">{i + 1}</span>
            <span class="name" title={it.path}>{it.entry.name}</span>
            {#if f}<span class="meta">{f.fw}×{f.fh}</span>{/if}
            <span class="meta">{fmtSize(it.entry.size)}</span>
            {#if store.marks[it.path]?.rating}<span class="stars">{"★".repeat(store.marks[it.path].rating)}</span>{/if}
            <span class="spacer"></span>
            <button class="keep" class:on={kept.has(it.path)} onclick={() => toggle(it.path)}>
              {kept.has(it.path) ? "✓ 남김" : "남기기"}
            </button>
            <button class="icon-btn small" title="비교에서 빼기" onclick={() => remove(it.path)}>
              <Icon name="close" size={13} />
            </button>
          </div>
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .compare {
    position: fixed;
    inset: 0;
    z-index: 55;
    display: flex;
    flex-direction: column;
    background: #0d0d0f;
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 44px;
    padding: 0 10px 0 16px;
    border-bottom: 1px solid #1d1e22;
    white-space: nowrap;
  }
  .title {
    font-weight: 600;
  }
  .dim {
    color: var(--text-dim);
  }
  .spacer {
    flex: 1;
  }
  .hint {
    font-size: 12px;
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .modes {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }
  .modes button {
    padding: 3px 10px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .modes button.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .opacity {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .opacity input {
    width: 100px;
    accent-color: var(--accent);
  }
  .btn.danger {
    background: #3a2224;
    border-color: #5a2c30;
    color: #ffb4b4;
  }
  .btn.danger:disabled {
    opacity: 0.4;
  }
  .stage {
    flex: 1;
    display: grid;
    min-height: 0;
    padding: 0;
    user-select: none;
  }
  .pane {
    position: relative;
    overflow: hidden;
    background: #121215;
    border: 2px solid transparent;
    min-width: 0;
    min-height: 0;
  }
  .pane.kept {
    border-color: var(--ok);
  }
  canvas {
    position: absolute;
    left: 0;
    top: 0;
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
  }
  .head {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 6px 5px 8px;
    background: linear-gradient(rgba(0, 0, 0, 0.65), transparent);
    font-size: 12px;
  }
  .num {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.15);
    font-weight: 600;
  }
  .pane.kept .num {
    background: var(--ok);
    color: #08140e;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .stars {
    color: #f5c451;
  }
  .keep {
    padding: 2px 8px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.25);
    font-size: 12px;
    white-space: nowrap;
  }
  .keep.on {
    background: var(--ok);
    border-color: var(--ok);
    color: #08140e;
    font-weight: 600;
  }
  .icon-btn.small {
    width: 22px;
    height: 22px;
  }
  .badge {
    position: absolute;
    left: 10px;
    top: 8px;
    padding: 3px 8px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.6);
    font-size: 12px;
  }
</style>
