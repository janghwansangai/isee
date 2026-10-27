<script lang="ts">
  import { store } from "../lib/store.svelte";
  import { isMac } from "../lib/api";
  import Icon from "./Icon.svelte";

  /** 경로를 클릭할 수 있는 조각으로: [{name, path}] */
  let crumbs = $derived.by(() => {
    const p = store.path;
    if (!p) return [];
    const win = /^[A-Za-z]:\\/.test(p);
    const sep = win ? "\\" : "/";
    const parts = p.split(sep).filter(Boolean);
    const out: { name: string; path: string }[] = [];
    let acc = win ? "" : "";
    parts.forEach((part, i) => {
      acc = win ? (i === 0 ? part + "\\" : acc.replace(/\\?$/, "\\") + part) : acc + "/" + part;
      out.push({ name: part, path: acc });
    });
    if (!win) out.unshift({ name: "/", path: "/" });
    // 너무 길면 앞부분을 줄인다
    return out.length > 6 ? [out[0], { name: "…", path: "" }, ...out.slice(-4)] : out;
  });

  let scopeText = $derived.by(() => {
    const parts: string[] = [];
    if (store.settings.include_subfolders) parts.push("하위 폴더 포함");
    if (store.extra.length) parts.push(`+${store.extra.length}곳`);
    return parts.length ? parts.join(" · ") : "이 폴더만";
  });

  const sorts = [
    { v: "name", t: "이름" },
    { v: "date", t: "날짜" },
    { v: "size", t: "크기" },
    { v: "type", t: "종류" },
    { v: "rating", t: "별점" },
  ] as const;

  let selImages = $derived.by(() => {
    let n = 0;
    for (const i of store.selected) if (store.entries[i]?.kind === "image") n++;
    return n;
  });
  let counts = $derived(store.settings.culling ? store.cullCounts() : null);
  const filters = [
    { v: "all", t: "전체" },
    { v: "picked", t: "남김만" },
    { v: "unflagged", t: "미정만" },
    { v: "rejected", t: "버림만" },
    { v: "rated", t: "별점 있는 것" },
  ] as const;

  let kindsLabel = $derived.by(() => {
    const k = store.settings.kinds;
    const on = [
      k.image && "이미지",
      k.video && "동영상",
      k.pdf && "PDF",
      k.doc && "문서",
      k.other && "기타",
    ].filter(Boolean);
    return on.length ? on.join("·") : "없음";
  });
</script>

<header class="toolbar" data-tauri-drag-region>
  <div class="nav">
    <button class="icon-btn" disabled={!store.canBack} onclick={() => store.back()} title="뒤로">
      <Icon name="back" />
    </button>
    <button class="icon-btn" disabled={!store.canForward} onclick={() => store.forward()} title="앞으로">
      <Icon name="forward" />
    </button>
    <button class="icon-btn" disabled={!store.parent} onclick={() => store.up()} title="상위 폴더">
      <Icon name="up" />
    </button>
  </div>

  <div class="crumbs" data-tauri-drag-region>
    {#each crumbs as c, i (i)}
      {#if i > 0}<span class="sep">›</span>{/if}
      {#if c.path}
        <button
          class="crumb"
          class:last={i === crumbs.length - 1}
          class:drop={store.dropOver === c.path}
          data-drop-path={c.path}
          onclick={() => store.open(c.path)}>{c.name}</button
        >
      {:else}
        <span class="sep">{c.name}</span>
      {/if}
    {/each}
  </div>
  {#if store.path}
    <button
      class="pill scope"
      class:on={!store.singleFolder}
      onclick={() => (store.scopeOpen = !store.scopeOpen)}
      title="보는 범위: 하위 폴더 사진도 보기, 다른 폴더 함께 보기"
    >
      {scopeText} ▾
    </button>
  {/if}

  <div class="right">
    {#if selImages >= 2}
      <button class="pill" onclick={() => store.openCompare()} title="고른 사진을 크게 나란히 비교 (C)">
        비교 {Math.min(selImages, 9)}장
      </button>
    {/if}
    <button class="pill" onclick={() => store.openPeople()} title="얼굴별로 묶어 보기 ({isMac ? "⌘⇧P" : "Ctrl+Shift+P"})">인물</button>
    <button class="pill" onclick={() => store.openSimilar()} title="비슷한 사진을 묶고 가장 잘 나온 사진을 추천 ({isMac ? "⌘⇧S" : "Ctrl+Shift+S"})">
      비슷한 사진
    </button>
    <button
      class="pill"
      class:on={store.settings.culling}
      onclick={() => store.setSettings({ culling: !store.settings.culling })}
      title="빠른 고르기: 사진마다 P 남김 · X 버림 · 숫자 별점을 매기면 다음 사진으로 넘어갑니다. 버린 사진은 한 번에 휴지통으로."
    >
      빠른 고르기
    </button>
    {#if counts}
      <select
        value={store.settings.cull_filter}
        onchange={(e) => store.setSettings({ cull_filter: e.currentTarget.value as never })}
        title="보기 필터"
      >
        {#each filters as f (f.v)}<option value={f.v}>{f.t}</option>{/each}
      </select>
      <span class="counts">
        <b class="pick">✓ {counts.picked}</b> <b class="reject">✕ {counts.rejected}</b> 미정 {counts.unflagged}
      </span>
      <button class="pill danger" disabled={!counts.rejected} onclick={() => store.trashRejected()}>
        버린 {counts.rejected}장 휴지통
      </button>
    {/if}
    <span class="kinds" title="보기 › 파일 종류에서 바꿀 수 있습니다">{kindsLabel}</span>
    <label class="sort" title="정렬">
      <Icon name="sort" size={14} />
      <select
        value={store.settings.sort_by}
        onchange={(e) => store.setSettings({ sort_by: e.currentTarget.value as never })}
      >
        {#each sorts as s (s.v)}<option value={s.v}>{s.t}</option>{/each}
      </select>
    </label>
    <div class="seg" role="group" aria-label="보기 방식">
      <button
        class="icon-btn"
        class:on={store.settings.view_mode !== "list"}
        onclick={() => store.setSettings({ view_mode: "grid" })}
        title="썸네일로 보기 ({isMac ? "⌘1" : "Ctrl+1"})"
      >
        <Icon name="grid" size={16} />
      </button>
      <button
        class="icon-btn"
        class:on={store.settings.view_mode === "list"}
        onclick={() => store.setSettings({ view_mode: "list" })}
        title="목록으로 보기: 파일 이름·날짜·크기 ({isMac ? "⌘2" : "Ctrl+2"})"
      >
        <Icon name="list" size={16} />
      </button>
    </div>
    <input
      class="size"
      class:hidden={store.settings.view_mode === "list"}
      type="range"
      min="88"
      max="320"
      step="8"
      value={store.settings.thumb_size}
      oninput={(e) => (store.settings.thumb_size = Number(e.currentTarget.value))}
      onchange={(e) => store.setSettings({ thumb_size: Number(e.currentTarget.value) })}
      title="썸네일 크기"
    />
    <button
      class="icon-btn"
      class:on={store.settings.show_info}
      onclick={() => store.setSettings({ show_info: !store.settings.show_info })}
      title="정보 패널"
    >
      <Icon name="info" />
    </button>
  </div>
</header>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 40px;
    padding: 0 10px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }
  .nav {
    display: flex;
    gap: 2px;
  }
  .crumbs {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .crumb {
    padding: 3px 6px;
    border-radius: 4px;
    color: var(--text-dim);
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .crumb:hover {
    background: var(--hover);
    color: var(--text);
  }
  .crumb.last {
    color: var(--text);
    font-weight: 600;
  }
  .crumb.drop {
    background: var(--accent-soft);
    outline: 1px dashed var(--accent);
  }
  .sep {
    color: var(--text-faint);
  }
  .right {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .pill {
    padding: 3px 10px;
    border-radius: 12px;
    border: 1px solid var(--border);
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
  }
  .pill:hover:not(:disabled) {
    color: var(--text);
    background: var(--hover);
  }
  .pill.on {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--text);
  }
  .pill.danger {
    border-color: #5a2c30;
    color: #ffb4b4;
  }
  .pill:disabled {
    opacity: 0.4;
  }
  .counts {
    font-size: 12px;
    color: var(--text-faint);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .counts .pick {
    color: var(--ok);
    font-weight: 600;
  }
  .counts .reject {
    color: #ff7a7a;
    font-weight: 600;
  }
  .kinds {
    font-size: 12px;
    color: var(--text-faint);
  }
  .sort {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-dim);
  }
  .sort select {
    padding: 2px 4px;
  }
  .size {
    width: 96px;
    accent-color: var(--accent);
  }
  /* 목록 보기에서는 썸네일 크기가 의미 없으므로 자리만 지킨다 (툴바가 흔들리지 않게) */
  .size.hidden {
    visibility: hidden;
  }
  .scope {
    flex: none;
    white-space: nowrap;
  }
  .seg {
    display: flex;
    gap: 0;
  }
  .icon-btn.on {
    color: var(--accent);
  }
</style>
