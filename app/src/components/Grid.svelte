<script lang="ts">
  import { SvelteSet } from "svelte/reactivity";
  import { store } from "../lib/store.svelte";
  import { api, thumbUrl, fmtDate, fmtSize, type Entry, type Settings } from "../lib/api";
  import Icon from "./Icon.svelte";

  /** 썸네일을 만들 수 없는 파일 (종류 아이콘으로 대신) */
  const broken = new SvelteSet<string>();
  // 다른 폴더로 가면 비운다: 다시 열었을 때 한 번 더 시도 (클라우드가 잠시 멈췄다가 살아난 경우 등)
  $effect(() => {
    void store.path;
    broken.clear();
  });

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
  // 목록 보기: 한 줄 높이와 위 머리글(이름·날짜·크기·종류) 높이
  const ROW = 30;
  const HEAD = 28;

  let el: HTMLDivElement;
  let width = $state(0);
  let height = $state(0);
  let scrollTop = $state(0);

  let list = $derived(store.settings.view_mode === "list");
  // 하위 폴더·다른 폴더를 함께 보면 목록에 폴더 칸을 보여 준다
  let showFolder = $derived(!store.singleFolder);
  let size = $derived(store.settings.thumb_size);
  $effect(() => {
    cols = list ? 1 : Math.max(1, Math.floor((width - PAD * 2 + GAP) / (size + GAP)));
  });
  let cellW = $derived(list ? width : Math.max(40, (width - PAD * 2 - GAP * (cols - 1)) / cols));
  let rowH = $derived(list ? ROW : cellW + CAPTION + GAP);
  let rows = $derived(Math.ceil(store.entries.length / cols));
  // 첫 줄의 위치: 스크롤 영역 기준(top0)과 칸을 그리는 판 기준(cy0). 목록은 머리글이 판 위에 붙어 있다.
  let top0 = $derived(list ? HEAD : PAD);
  let cy0 = $derived(list ? 0 : PAD);
  // 목록 머리글은 스크롤해도 위에 붙어 있어 그 아래부터 보인다
  let hidTop = $derived(list ? HEAD : 0);
  // 화면에 실제로 보이는 줄 (썸네일 우선순위용)
  let viewFirst = $derived(Math.max(0, Math.floor((scrollTop + hidTop - top0) / rowH)));
  let viewLast = $derived(Math.min(rows - 1, Math.floor((scrollTop + height - top0) / rowH)));
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
    const top = top0 + Math.floor(t / cols) * rowH;
    const margin = list ? 0 : GAP;
    if (top < el.scrollTop + hidTop) el.scrollTop = top - hidTop - margin;
    else if (top + rowH > el.scrollTop + height) el.scrollTop = top + rowH - height + margin;
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

  // 목록 머리글: 같은 칸을 다시 누르면 오름·내림 순서를 바꾼다
  function sortBy(by: Settings["sort_by"]) {
    if (store.settings.sort_by === by) store.setSettings({ sort_desc: !store.settings.sort_desc });
    else store.setSettings({ sort_by: by, sort_desc: false });
  }

  function kindLabel(e: Entry): string {
    if (e.kind === "dir") return "폴더";
    const dot = e.name.lastIndexOf(".");
    return dot > 0 ? e.name.slice(dot + 1).toUpperCase() : "";
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest(".lhead")) return;
    const i = indexAt(e);
    if (i < 0) {
      store.selected.clear();
      return;
    }
    // 왼쪽 위 체크 상자는 ⌘(Ctrl)-클릭과 같다: 다른 선택을 유지한 채 이것만 넣고 뺀다
    const onCheck = !!(e.target as HTMLElement).closest("[data-check]");
    const mod = e.metaKey || e.ctrlKey || onCheck;
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
  {#if list}
    {@const by = store.settings.sort_by}
    {@const arrow = store.settings.sort_desc ? "▼" : "▲"}
    <div class="lhead" style="height:{HEAD}px">
      <span class="lcheck"></span>
      <span class="lthumb"></span>
      <button class="lname" class:on={by === "name"} onclick={() => sortBy("name")}>이름 {by === "name" ? arrow : ""}</button>
      {#if showFolder}<span class="lfolder">폴더</span>{/if}
      <button class="ldate" class:on={by === "date"} onclick={() => sortBy("date")}>수정한 날짜 {by === "date" ? arrow : ""}</button>
      <button class="lsize" class:on={by === "size"} onclick={() => sortBy("size")}>크기 {by === "size" ? arrow : ""}</button>
      <button class="lkind" class:on={by === "type"} onclick={() => sortBy("type")}>종류 {by === "type" ? arrow : ""}</button>
    </div>
  {/if}
  <div class="canvas" style="height:{rows * rowH + cy0 + (list ? 0 : PAD)}px">
    {#each visible as i (store.entries[i].path)}
      {@const e = store.entries[i]}
      {@const x = list ? 0 : PAD + (i % cols) * (cellW + GAP)}
      {@const y = cy0 + Math.floor(i / cols) * rowH}
      {@const m = store.marks[e.path]}
      {@const thumbable = (e.kind === "image" || e.kind === "video" || e.kind === "pdf") && !broken.has(`${e.path}|${e.mtime}`)}
      {#if list}
        <div
          class="cell row"
          class:odd={i % 2 === 1}
          class:sel={store.selected.has(i)}
          class:focus={store.focus === i}
          class:drop={e.kind === "dir" && store.dropOver === e.path}
          class:multi={store.selected.size > 1}
          class:rejected={m?.flag === -1}
          style="transform:translateY({y}px);width:{cellW}px;height:{ROW}px"
          data-index={i}
          data-drop-path={e.kind === "dir" ? e.path : undefined}
          role="option"
          aria-selected={store.selected.has(i)}
        >
          <span class="check lcheck" class:on={store.selected.has(i)} data-check title="여러 개 고르기">{store.selected.has(i) ? "✓" : ""}</span>
          <span class="lthumb">
            {#if thumbable}
              <img src={thumbUrl(e, i)} alt="" draggable="false" decoding="async" onerror={() => broken.add(`${e.path}|${e.mtime}`)} />
            {:else}
              <span class="kind {e.kind}"><Icon name={e.kind} size={18} /></span>
            {/if}
          </span>
          <span class="lname" title={e.name}>
            <span class="ltext">{e.name}</span>
            {#if m?.flag}<span class="lflag" class:pick={m.flag === 1}>{m.flag === 1 ? "✓" : "✕"}</span>{/if}
            {#if m?.rating}<span class="lstars">{"★".repeat(m.rating)}</span>{/if}
            {#if e.cloud}<span class="lcloud" title="클라우드에만 있는 파일 (열면 내려받습니다)">☁</span>{/if}
          </span>
          {#if showFolder}<span class="lfolder" title={e.folder}>{e.folder ?? ""}</span>{/if}
          <span class="ldate">{fmtDate(e.mtime)}</span>
          <span class="lsize">{e.kind === "dir" ? "" : fmtSize(e.size)}</span>
          <span class="lkind">{kindLabel(e)}</span>
        </div>
      {:else}
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
          <div class="thumb" class:rejected={m?.flag === -1} class:multi={store.selected.size > 1} style="height:{cellW}px">
            <span class="check" class:on={store.selected.has(i)} data-check title="여러 개 고르기">{store.selected.has(i) ? "✓" : ""}</span>
            {#if m?.flag}
              <span class="flag" class:pick={m.flag === 1}>{m.flag === 1 ? "✓" : "✕"}</span>
            {/if}
            {#if m?.rating}
              <span class="stars">{"★".repeat(m.rating)}</span>
            {/if}
            {#if thumbable}
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
          <div class="name" title={e.folder ? `${e.folder}/${e.name}` : e.name}>{e.name}</div>
        </div>
      {/if}
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
  .check {
    position: absolute;
    left: 6px;
    top: 6px;
    z-index: 2;
    display: none;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 5px;
    border: 1.5px solid rgba(255, 255, 255, 0.85);
    background: rgba(0, 0, 0, 0.35);
    color: white;
    font-size: 13px;
    font-weight: 700;
  }
  .thumb:hover .check,
  .thumb.multi .check,
  .check.on {
    display: inline-flex;
  }
  .check.on {
    background: var(--accent);
    border-color: var(--accent);
  }
  .flag {
    position: absolute;
    right: 5px;
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
  /* 칸을 채운다: 클라우드 미리보기(구글 드라이브 128px 등)처럼 작은 썸네일도 칸 크기로 보이게 */
  .thumb img {
    width: 100%;
    height: 100%;
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

  /* ───── 목록 보기 ───── */
  .lhead {
    position: sticky;
    top: 0;
    z-index: 3;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 12px;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
    font-size: 11px;
    color: var(--text-faint);
  }
  .lhead button {
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    white-space: nowrap;
  }
  .lhead button:hover,
  .lhead button.on {
    color: var(--text);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 12px;
    font-size: 13px;
    color: var(--text-dim);
  }
  .row.odd {
    background: rgba(255, 255, 255, 0.025);
  }
  .row.sel {
    background: var(--accent-soft);
    color: var(--text);
  }
  .row.focus:not(.sel) {
    box-shadow: inset 0 0 0 1px #4a4b55;
  }
  .row.drop {
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .row.rejected .ltext,
  .row.rejected .lthumb img {
    opacity: 0.4;
  }
  .lcheck {
    flex: none;
    width: 16px;
  }
  /* 체크 상자: 자리는 늘 두고(줄이 흔들리지 않게) 올렸을 때·여러 개 골랐을 때·고른 줄만 보인다 */
  .row .check {
    position: static;
    display: inline-flex;
    visibility: hidden;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    font-size: 11px;
  }
  .row:hover .check,
  .row.multi .check,
  .row .check.on {
    visibility: visible;
  }
  .lthumb {
    flex: none;
    width: 22px;
    height: 22px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 3px;
    overflow: hidden;
  }
  .lthumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .lname {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .ltext {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .row.sel .ltext {
    color: var(--text);
  }
  .lflag {
    flex: none;
    color: #d94848;
    font-weight: 700;
    font-size: 12px;
  }
  .lflag.pick {
    color: var(--ok);
  }
  .lstars {
    flex: none;
    color: #f5c451;
    font-size: 11px;
    letter-spacing: 1px;
  }
  .lcloud {
    flex: none;
    color: #9fc4ff;
    font-size: 12px;
  }
  .lfolder {
    flex: none;
    width: 160px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .row .lfolder {
    font-size: 12px;
    color: var(--text-faint);
  }
  .ldate {
    flex: none;
    width: 128px;
    font-variant-numeric: tabular-nums;
  }
  .lsize {
    flex: none;
    width: 72px;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .lhead .lsize {
    text-align: right;
  }
  .lkind {
    flex: none;
    width: 52px;
  }
  .row .ldate,
  .row .lsize,
  .row .lkind {
    font-size: 12px;
    color: var(--text-faint);
  }
  .row.sel .ldate,
  .row.sel .lsize,
  .row.sel .lkind {
    color: var(--text-dim);
  }
</style>
