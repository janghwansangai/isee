<script lang="ts">
  // 사진 인쇄: 인쇄 방식·용지·여백·글자를 고르고 미리 본 뒤 운영체제 인쇄 창으로 보낸다.
  // 미리보기는 썸네일로 그리고, 인쇄할 때만 원본에서 필요한 크기로 읽어 300dpi로 그린다.
  import { onMount, untrack } from "svelte";
  import { store } from "../lib/store.svelte";
  import { thumbUrl, type Entry } from "../lib/api";
  import {
    MAX_PRINT_PAGES,
    PAPERS,
    drawPage,
    layout,
    loadOptions,
    printPages,
    saveOptions,
    type PrintMode,
    type PrintOptions,
    type PrintPhoto,
  } from "../lib/print";
  import Icon from "./Icon.svelte";

  let { entries, onclose }: { entries: Entry[]; onclose: () => void } = $props();

  let opts = $state<PrintOptions>(loadOptions());
  /** 미리보기용 썸네일 (사진 비율도 여기서 안다) */
  let thumbs = $state.raw<(ImageBitmap | null)[]>([]);
  let photos = $state.raw<PrintPhoto[]>([]);
  let loading = $state(true);
  let pageNo = $state(0);
  let printing = $state(false);

  const MODES: { v: PrintMode; t: string; d: string }[] = [
    { v: "single", t: "한 장에 한 사진", d: "사진마다 용지 한 장" },
    { v: "multi", t: "여러 사진을 한 장에", d: "고른 사진을 비율대로 한 장에 모아서" },
    { v: "repeat", t: "한 사진을 여러 개로", d: "같은 사진을 작게 2·4·8·16개" },
    { v: "poster", t: "크게 나눠 인쇄", d: "한 사진을 여러 장에 나눠 크게 (이어 붙이기)" },
  ];

  onMount(() => {
    let alive = true;
    (async () => {
      const ps: PrintPhoto[] = [];
      const bs: (ImageBitmap | null)[] = [];
      for (const e of entries) {
        let b: ImageBitmap | null = null;
        try {
          const r = await fetch(thumbUrl(e, -1));
          if (r.ok) b = await createImageBitmap(await r.blob());
        } catch {
          b = null;
        }
        if (!alive) {
          b?.close();
          return;
        }
        bs.push(b);
        ps.push({ path: e.path, name: e.name, w: b?.width ?? 4, h: b?.height ?? 3 });
      }
      thumbs = bs;
      photos = ps;
      loading = false;
    })();
    return () => {
      alive = false;
      thumbs.forEach((b) => b?.close());
    };
  });

  let pages = $derived(photos.length ? layout(photos, opts) : []);
  $effect(() => {
    // 설정을 바꾸면 기억하고, 쪽 수가 줄면 마지막 쪽으로
    saveOptions($state.snapshot(opts));
    const n = pages.length;
    untrack(() => {
      if (pageNo >= n) pageNo = Math.max(0, n - 1);
    });
  });

  // ───── 미리보기 ─────
  let boxW = $state(0);
  let boxH = $state(0);
  let canvas = $state<HTMLCanvasElement>();
  let page = $derived(pages[pageNo]);
  let size = $derived.by(() => {
    if (!page || !boxW || !boxH) return null;
    const s = Math.min((boxW - 48) / page.w, (boxH - 48) / page.h);
    return { w: Math.max(1, Math.floor(page.w * s)), h: Math.max(1, Math.floor(page.h * s)) };
  });
  $effect(() => {
    if (!canvas || !page || !size) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(size.w * dpr);
    canvas.height = Math.round(size.h * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.imageSmoothingQuality = "high";
    drawPage(ctx, page, photos, canvas.width / page.w, (i) => thumbs[i]);
  });

  // ───── 인쇄 ─────
  async function print() {
    if (printing || !pages.length) return;
    printing = true;
    store.progress = { done: 0, total: pages.length, label: "인쇄 준비" };
    try {
      await printPages(pages, photos, (done, total) => (store.progress = { done, total, label: "인쇄 준비" }));
    } catch (e) {
      store.showToast(`인쇄하지 못했습니다: ${e}`, { error: true });
    } finally {
      store.progress = null;
      printing = false;
    }
  }

  const paper = $derived(PAPERS[opts.paper]);
  let sheets = $derived(pages.length);
  let many = $derived(opts.mode === "multi" && photos.length > 30);
  /** 쪽이 너무 많으면 인쇄하지 않는다 (메모리) */
  let tooMany = $derived(sheets > MAX_PRINT_PAGES);
</script>

<div class="print" role="dialog" aria-label="인쇄" tabindex="-1" oncontextmenu={(e) => e.preventDefault()}>
  <div class="topbar">
    <span class="title">인쇄</span>
    <span class="dim">사진 {entries.length.toLocaleString()}장</span>
    <span class="spacer"></span>
    <button class="icon-btn" title="닫기 (Esc)" onclick={onclose}><Icon name="close" /></button>
  </div>

  <div class="main">
    <div class="side">
      <section>
        <h3>인쇄 방식</h3>
        <div class="modes">
          {#each MODES as m (m.v)}
            <label class="mode" class:on={opts.mode === m.v}>
              <input type="radio" name="mode" value={m.v} bind:group={opts.mode} />
              <span class="t">{m.t}</span>
              <span class="d">{m.d}</span>
            </label>
          {/each}
        </div>
        {#if opts.mode === "repeat"}
          <div class="seg sub" role="group" aria-label="한 장에 몇 개">
            {#each [2, 4, 8, 16] as const as n (n)}
              <button class:on={opts.copies === n} onclick={() => (opts.copies = n)}>{n}개</button>
            {/each}
          </div>
        {:else if opts.mode === "poster"}
          <div class="seg sub" role="group" aria-label="몇 장으로 나눌지">
            {#each [2, 4, 9, 16] as const as n (n)}
              <button class:on={opts.tiles === n} onclick={() => (opts.tiles = n)}>{n}장</button>
            {/each}
          </div>
          <p class="note">장마다 여백을 잘라 내고 이어 붙이면 큰 사진이 됩니다. 글자를 표시하면 장마다 몇 행 몇 열인지 적힙니다.</p>
        {/if}
      </section>

      <section>
        <h3>용지</h3>
        <select bind:value={opts.paper}>
          {#each Object.entries(PAPERS) as [k, p] (k)}<option value={k}>{p.label}</option>{/each}
        </select>
        <div class="seg" role="group" aria-label="용지 방향">
          <button class:on={opts.orient === "auto"} onclick={() => (opts.orient = "auto")} title="사진이 가장 크게 나오는 방향">자동</button>
          <button class:on={opts.orient === "portrait"} onclick={() => (opts.orient = "portrait")}>세로</button>
          <button class:on={opts.orient === "landscape"} onclick={() => (opts.orient = "landscape")}>가로</button>
        </div>
        <p class="note">인쇄 창에서도 같은 용지({paper.label.split(" ")[0]})를 골라 주세요.</p>
      </section>

      {#if opts.mode !== "multi"}
        <section>
          <h3>사진 맞춤</h3>
          <div class="seg" role="group" aria-label="사진 맞춤">
            <button class:on={opts.fit === "fit"} onclick={() => (opts.fit = "fit")} title="사진 전체가 보이게 (남는 곳은 흰색)">용지에 맞춤</button>
            <button class:on={opts.fit === "fill"} onclick={() => (opts.fit = "fill")} title="빈 곳 없이 꽉 차게 (가장자리가 조금 잘림)">꽉 채우기</button>
          </div>
          <label class="check">
            <input type="checkbox" bind:checked={opts.rotate} />
            사진 방향이 용지와 다르면 돌려서 크게
          </label>
        </section>
      {/if}

      <section>
        <h3>여백</h3>
        <label class="range">
          가장자리
          <input type="range" min="0" max="30" step="1" bind:value={opts.margin} />
          <span class="val">{opts.margin}mm</span>
        </label>
        {#if opts.mode === "multi" || opts.mode === "repeat"}
          <label class="range">
            사진 사이
            <input type="range" min="0" max="15" step="1" bind:value={opts.gap} />
            <span class="val">{opts.gap}mm</span>
          </label>
        {/if}
        <p class="note">대부분의 프린터는 용지 가장자리 3~5mm를 인쇄하지 못합니다. 여백을 0으로 하면 가장자리가 잘릴 수 있습니다.</p>
      </section>

      <section>
        <h3>글자</h3>
        <div class="seg" role="group" aria-label="글자">
          <button class:on={!opts.captions} onclick={() => (opts.captions = false)} title="머리말·꼬리말·안내 없이 사진만">글자 없이 사진만</button>
          <button class:on={opts.captions} onclick={() => (opts.captions = true)}>파일 이름·날짜 표시</button>
        </div>
      </section>

      <div class="go">
        <span class="dim small">
          {#if loading}사진을 읽는 중…{:else}용지 {sheets.toLocaleString()}장{/if}
        </span>
        <button class="btn primary" disabled={loading || printing || !sheets || tooMany} onclick={print}>
          {printing ? "준비 중…" : "인쇄…"}
        </button>
      </div>
      {#if many}<p class="note warn">사진이 많아 한 장에 모으면 아주 작게 나옵니다.</p>{/if}
      {#if tooMany}<p class="note warn">한 번에 {MAX_PRINT_PAGES}쪽까지 인쇄할 수 있습니다. 사진을 나눠서 인쇄하거나 나누는 장 수를 줄여 주세요.</p>{/if}
    </div>

    <div class="preview" bind:clientWidth={boxW} bind:clientHeight={boxH}>
      {#if loading}
        <span class="dim">미리보기를 준비하는 중…</span>
      {:else if size}
        <canvas bind:this={canvas} style="width:{size.w}px;height:{size.h}px"></canvas>
        {#if opts.mode === "poster"}
          <p class="hint">미리보기는 작은 그림이라 흐릴 수 있습니다. 인쇄는 원본으로 선명하게 나옵니다.</p>
        {/if}
        {#if pages.length > 1}
          <div class="nav">
            <button class="icon-btn" disabled={pageNo <= 0} onclick={() => pageNo--} title="앞 쪽"><Icon name="back" /></button>
            <span>{pageNo + 1} / {pages.length}{#if page?.tile}{"  ·  "}{page.tile}{/if}</span>
            <button class="icon-btn" disabled={pageNo >= pages.length - 1} onclick={() => pageNo++} title="다음 쪽"><Icon name="forward" /></button>
          </div>
        {/if}
      {/if}
    </div>
  </div>
</div>

<style>
  .print {
    position: fixed;
    inset: 0;
    z-index: 55;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 44px;
    padding: 0 10px 0 16px;
    white-space: nowrap;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }
  .title {
    font-weight: 600;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .main {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .side {
    width: 320px;
    flex: none;
    overflow-y: auto;
    padding: 6px 16px 20px;
    border-right: 1px solid var(--border);
    background: var(--panel);
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 0;
    border-bottom: 1px solid var(--border);
  }
  h3 {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
  }
  .note {
    margin: 0;
    font-size: 11px;
    line-height: 1.45;
    color: var(--text-faint);
  }
  .note.warn {
    margin-top: 6px;
    color: #f0b45a;
  }
  .modes {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .mode {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 8px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .mode.on {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .mode input {
    grid-row: span 2;
    accent-color: var(--accent);
  }
  .mode .t {
    font-weight: 600;
  }
  .mode .d {
    font-size: 11px;
    color: var(--text-dim);
  }
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }
  .seg.sub {
    margin-top: 2px;
  }
  .seg button {
    flex: 1;
    padding: 5px 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  .seg button + button {
    border-left: 1px solid var(--border);
  }
  .seg button.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .check input {
    accent-color: var(--accent);
  }
  .range {
    display: grid;
    grid-template-columns: 64px 1fr 44px;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .range input {
    padding: 0;
    accent-color: var(--accent);
  }
  .val {
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--text-dim);
  }
  .go {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding-top: 14px;
  }
  .go .btn {
    min-width: 96px;
  }
  .go .btn:disabled {
    opacity: 0.5;
  }
  .preview {
    flex: 1;
    min-width: 0;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #2a2b30;
  }
  canvas {
    display: block;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.45);
  }
  .hint {
    position: absolute;
    top: 8px;
    left: 50%;
    transform: translateX(-50%);
    margin: 0;
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
  }
  .nav {
    position: absolute;
    bottom: 10px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 2px 8px;
    border-radius: 14px;
    background: rgba(20, 20, 24, 0.75);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
</style>
