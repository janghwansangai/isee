<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";
  import { store } from "../lib/store.svelte";
  import { api, thumbUrl } from "../lib/api";
  import Icon from "./Icon.svelte";

  /** 썸네일을 만들 수 없는 파일 (종류 아이콘으로 대신) */
  const broken = new SvelteSet<string>();

  let {
    cols = $bindable(1),
    onOpen,
    onContext,
    onDragStart,
  }: {
    cols?: number;
    onOpen: (i: number) => void;
    onContext: (i: number, e: MouseEvent) => void;
    onDragStart: (e: PointerEvent) => void;
  } = $props();

  const GAP = 8;
  const CAPTION = 22;
  const PAD = 12;

  let el: HTMLDivElement;
  let width = $state(0);
  let height = $state(0);
  let scrollTop = $state(0);

  let size = $derived(store.settings.thumb_size);
  $effect(() => {
    cols = Math.max(1, Math.floor((width - PAD * 2 + GAP) / (size + GAP)));
  });
  let cellW = $derived(Math.max(40, (width - PAD * 2 - GAP * (cols - 1)) / cols));
  let rowH = $derived(cellW + CAPTION + GAP);
  let rows = $derived(Math.ceil(store.entries.length / cols));
  // 화면에 실제로 보이는 줄 (썸네일 우선순위용)
  let viewFirst = $derived(Math.max(0, Math.floor((scrollTop - PAD) / rowH)));
  let viewLast = $derived(Math.min(rows - 1, Math.floor((scrollTop + height - PAD) / rowH)));
  // DOM으로 그리는 줄: 위아래로 2줄 여유
  let first = $derived(Math.max(0, viewFirst - 2));
  let last = $derived(Math.min(rows - 1, viewLast + 2));
  let visible = $derived.by(() => {
    const out: number[] = [];
    const end = Math.min(store.entries.length, (last + 1) * cols);
    for (let i = first * cols; i < end; i++) out.push(i);
    return out;
  });

  // 보이는 범위를 엔진에 알려 그 칸의 썸네일부터 만들게 한다.
  let raf = 0;
  $effect(() => {
    const start = viewFirst * cols;
    const end = Math.min(store.entries.length - 1, (viewLast + 1) * cols - 1);
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(() => api.setViewport(start, end));
  });

  // 포커스 이동 시 그 칸이 보이게 스크롤
  $effect(() => {
    const t = store.scrollTo;
    if (t < 0 || !el || !height) return;
    const top = PAD + Math.floor(t / cols) * rowH;
    if (top < el.scrollTop) el.scrollTop = top - GAP;
    else if (top + rowH > el.scrollTop + height) el.scrollTop = top + rowH - height + GAP;
    store.scrollTo = -1;
  });

  // 폴더가 바뀌면 맨 위로
  let lastPath = "";
  $effect(() => {
    if (store.path !== lastPath) {
      lastPath = store.path;
      if (el) el.scrollTop = 0;
    }
  });

  function indexAt(e: Event): number {
    const cell = (e.target as HTMLElement).closest<HTMLElement>("[data-index]");
    return cell ? Number(cell.dataset.index) : -1;
  }

  let down: { x: number; y: number; i: number; deferSelect: boolean } | null = null;

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const i = indexAt(e);
    if (i < 0) {
      store.selected.clear();
      return;
    }
    const mod = e.metaKey || e.ctrlKey;
    let deferSelect = false;
    if (mod) {
      if (store.selected.has(i)) store.selected.delete(i);
      else store.selected.add(i);
      store.focus = i;
      store.anchor = i;
    } else if (e.shiftKey) {
      store.selectRange(i);
    } else if (store.selected.has(i)) {
      // 여러 개를 끌 수 있게, 끌지 않고 놓았을 때만 하나만 선택
      deferSelect = true;
      store.focus = i;
    } else {
      store.selectOnly(i);
    }
    down = { x: e.clientX, y: e.clientY, i, deferSelect };
    // 커서가 그리드 밖으로 빠르게 나가도 움직임을 계속 받도록 붙잡는다
    el.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    // 눌림 여부는 pointerdown/up으로 직접 기억한다 (포인터를 붙잡고 있으므로 up을 놓치지 않음)
    if (!down) return;
    if (Math.hypot(e.clientX - down.x, e.clientY - down.y) > 6) {
      down = null;
      onDragStart(e);
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (down?.deferSelect) store.selectOnly(down.i);
    down = null;
    if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
  }

  // 끌기를 위해 포인터를 붙잡으면 dblclick의 target이 그리드 전체가 되므로, 좌표로 칸을 찾는다
  function onDblClick(e: MouseEvent) {
    const cell = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>("[data-index]");
    if (cell) onOpen(Number(cell.dataset.index));
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const i = indexAt(e);
    if (i < 0) return;
    if (!store.selected.has(i)) store.selectOnly(i);
    onContext(i, e);
  }
</script>

<div
  class="grid"
  bind:this={el}
  bind:clientWidth={width}
  bind:clientHeight={height}
  onscroll={() => (scrollTop = el.scrollTop)}
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  ondblclick={onDblClick}
  oncontextmenu={onContextMenu}
  role="listbox"
  tabindex="-1"
  aria-multiselectable="true"
>
  {#if store.entries.length === 0 && !store.loading}
    <div class="empty">
      {store.error ?? "이 폴더에 표시할 파일이 없습니다"}
      {#if !store.error}<div class="hint">보기 › 파일 종류에서 다른 종류를 켤 수 있습니다</div>{/if}
    </div>
  {/if}
  <div class="canvas" style="height:{rows * rowH + PAD * 2}px">
    {#each visible as i (store.entries[i].path)}
      {@const e = store.entries[i]}
      {@const x = PAD + (i % cols) * (cellW + GAP)}
      {@const y = PAD + Math.floor(i / cols) * rowH}
      {@const m = store.marks[e.path]}
      <div
        class="cell"
        class:sel={store.selected.has(i)}
        class:focus={store.focus === i}
        class:drop={e.kind === "dir" && store.dropOver === e.path}
        style="transform:translate({x}px,{y}px);width:{cellW}px"
        data-index={i}
        data-drop-path={e.kind === "dir" ? e.path : undefined}
        role="option"
        aria-selected={store.selected.has(i)}
      >
        <div class="thumb" class:rejected={m?.flag === -1} style="height:{cellW}px">
          {#if m?.flag}
            <span class="flag" class:pick={m.flag === 1}>{m.flag === 1 ? "✓" : "✕"}</span>
          {/if}
          {#if m?.rating}
            <span class="stars">{"★".repeat(m.rating)}</span>
          {/if}
          {#if (e.kind === "image" || e.kind === "video" || e.kind === "pdf") && !broken.has(`${e.path}|${e.mtime}`)}
            <img
              src={thumbUrl(e, i)}
              alt=""
              draggable="false"
              decoding="async"
              onerror={() => broken.add(`${e.path}|${e.mtime}`)}
            />
            {#if e.kind === "video"}<span class="badge play">▶</span>{/if}
            {#if e.kind === "pdf"}<span class="badge">PDF</span>{/if}
          {:else}
            <span class="kind {e.kind}"><Icon name={e.kind} size={Math.round(cellW * 0.42)} /></span>
          {/if}
          {#if e.cloud}
            <span class="cloud" title="클라우드에만 있는 파일 (열면 내려받습니다)">☁</span>
          {/if}
        </div>
        <div class="name" title={e.name}>{e.name}</div>
      </div>
    {/each}
  </div>
</div>

<style>
  .grid {
    position: relative;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    outline: none;
  }
  .canvas {
    position: relative;
  }
  .cell {
    position: absolute;
    top: 0;
    left: 0;
    contain: layout paint;
  }
  .thumb {
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius);
    background: #1c1d21;
    overflow: hidden;
    border: 2px solid transparent;
  }
  .thumb {
    position: relative;
  }
  .thumb.rejected img {
    opacity: 0.35;
  }
  .flag {
    position: absolute;
    left: 5px;
    top: 5px;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 10px;
    background: #d94848;
    color: white;
    font-size: 12px;
    font-weight: 700;
  }
  .flag.pick {
    background: var(--ok);
    color: #08140e;
  }
  .stars {
    position: absolute;
    left: 5px;
    bottom: 4px;
    z-index: 1;
    padding: 0 4px;
    border-radius: 4px;
    background: rgba(0, 0, 0, 0.55);
    color: #f5c451;
    font-size: 11px;
    letter-spacing: 1px;
  }
  .badge {
    position: absolute;
    left: 6px;
    bottom: 6px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(0, 0, 0, 0.6);
    color: #fff;
    font-size: 10px;
    font-weight: 600;
  }
  .cloud {
    position: absolute;
    right: 6px;
    bottom: 6px;
    padding: 0 5px;
    border-radius: 4px;
    background: rgba(0, 0, 0, 0.6);
    color: #9fc4ff;
    font-size: 13px;
  }
  .thumb img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    display: block;
  }
  .cell.sel .thumb {
    background: var(--accent-soft);
    border-color: var(--accent-line);
  }
  .cell.focus:not(.sel) .thumb {
    border-color: #4a4b55;
  }
  .cell.drop .thumb {
    background: var(--accent-soft);
    border-color: var(--accent);
    border-style: dashed;
  }
  .name {
    height: 22px;
    line-height: 22px;
    padding: 0 4px;
    font-size: 12px;
    color: var(--text-dim);
    text-align: center;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cell.sel .name {
    color: var(--text);
  }
  .kind {
    color: #6d7080;
    display: flex;
  }
  .kind.dir {
    color: #6ea0ff;
  }
  .kind.video {
    color: #c48cff;
  }
  .kind.pdf {
    color: #ff7a7a;
  }
  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    pointer-events: none;
  }
  .hint {
    font-size: 12px;
  }
</style>
