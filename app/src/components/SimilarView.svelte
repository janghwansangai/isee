<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { SvelteMap, SvelteSet } from "svelte/reactivity";
  import { listen } from "@tauri-apps/api/event";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { store } from "../lib/store.svelte";
  import {
    api,
    baseName,
    fmtSize,
    photoEntry,
    scopeLabel,
    thumbUrl,
    type Photo,
    type SimilarMember,
    type SimilarOptions,
    type SimilarReport,
    type SimilarRule,
  } from "../lib/api";
  import Icon from "./Icon.svelte";

  type Group = SimilarReport["groups"][number];
  type Tab = "groups" | "duplicates" | "blurry" | "dark" | "bright";

  // 화면을 연 폴더 (그사이 다른 폴더로 가도 이 폴더 기준)
  // 연 범위 (지금 폴더, 하위 폴더·함께 보는 폴더 포함)
  const scope = store.scope();
  let report = $state.raw<SimilarReport | null>(null);
  let progress = $state<[number, number]>([0, 0]);
  let tab = $state<Tab>("groups");
  let opts = $state<SimilarOptions>($state.snapshot(store.settings.similar));
  /** 사용자가 바꾼 남김 여부 (경로 → 남김). 없으면 추천을 따른다 */
  const overrides = new SvelteMap<string, boolean>();
  /** 흐림·어두움·밝음 탭에서 고른 사진 */
  const picked = new SvelteSet<string>();
  let working = $state(false);

  const PRESETS: { label: string; rules: SimilarRule[] }[] = [
    { label: "선명한 것", rules: ["sharp", "exposure", "noise", "resolution", "newest"] },
    { label: "노출이 좋은 것", rules: ["exposure", "sharp", "noise", "resolution", "newest"] },
    { label: "노이즈가 적은 것", rules: ["noise", "sharp", "exposure", "resolution", "newest"] },
    { label: "해상도가 높은 것", rules: ["resolution", "sharp", "exposure", "noise", "newest"] },
    { label: "나중에 찍은 것", rules: ["newest", "sharp", "exposure", "noise", "resolution"] },
  ];
  const LEVELS = ["", "거의 같은 것만", "아주 비슷한 것", "비슷한 것", "조금 비슷해도", "대충 비슷해도"];
  let preset = $derived(Math.max(0, PRESETS.findIndex((p) => p.rules[0] === opts.rules[0])));

  const keepOf = (m: SimilarMember) => overrides.get(m.path) ?? m.keep;
  const restOf = (g: Group) => g.members.filter((m) => !keepOf(m));
  let restAll = $derived(report ? report.groups.flatMap(restOf) : []);
  let analyzing = $derived(progress[1] > 0 && progress[0] < progress[1]);

  // ───── 보고서 ─────
  let timer = 0;
  let lastAt = 0;
  async function refresh() {
    try {
      report = await api.similarReport(scope, $state.snapshot(opts));
      lastAt = performance.now();
    } catch (e) {
      store.showToast(`비슷한 사진을 찾지 못했습니다: ${e}`, { error: true });
    }
  }
  function refreshSoon(delay = 100) {
    clearTimeout(timer);
    timer = window.setTimeout(refresh, delay);
  }

  onMount(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    listen<[number, number]>("analysis-progress", (e) => {
      progress = e.payload;
      // 분석 중에는 1.5초마다 다시 묶어 보여 주고, 끝나면 바로
      if (e.payload[0] >= e.payload[1] || performance.now() - lastAt > 1500) refreshSoon(0);
    }).then((u) => (alive ? (unlisten = u) : u()));
    (async () => {
      // 사진이 아주 많으면 걸릴 시간을 알리고 묻는다 (취소하면 분석한 것만으로 보여 준다)
      if (!(await store.confirmAnalysis(scope))) {
        refresh();
        return;
      }
      if (!alive) return;
      const p = await api.analyzeFolder(scope);
      // 분석할 것이 적으면(클라우드 사진은 건너뜀) 끝났다는 알림이 이 응답보다 먼저 온다: 더 나아간 쪽을 쓴다
      if (!(progress[1] === p[1] && progress[0] > p[0])) progress = p;
      refresh();
    })().catch((e) => store.showToast(`분석을 시작하지 못했습니다: ${e}`, { error: true }));
    return () => {
      alive = false;
      unlisten?.();
      clearTimeout(timer);
      // 화면을 닫아도 분석은 뒤에서 계속한다 (백그라운드 작업 창에서 보고 멈출 수 있음)
    };
  });

  // 기준을 바꾸면 저장된 특징값으로 바로 다시 묶는다 (사진을 다시 읽지 않음)
  let first = true;
  $effect(() => {
    void [opts.level, opts.keep, opts.rules.join()];
    if (first) {
      first = false;
      return;
    }
    untrack(() => refreshSoon(60));
  });

  // 지우기·되돌리기·비교 화면에서 정리한 뒤 폴더가 다시 읽히면 보고서도 다시
  $effect(() => {
    void store.entries;
    untrack(() => report && refreshSoon(200));
  });

  // ───── 동작 ─────
  function toggle(m: SimilarMember) {
    const next = !keepOf(m);
    if (next === m.keep) overrides.delete(m.path);
    else overrides.set(m.path, next);
  }

  function compareGroup(g: Group) {
    // 비교 화면은 9장까지: 남길 것 먼저, 나머지는 점수 순
    const ordered = [...g.members.filter(keepOf), ...restOf(g)].slice(0, 9);
    store.openCompare(
      ordered.map((m) => m.path),
      { keep: ordered.filter(keepOf).map((m) => m.path), entries: ordered.map(photoEntry) },
    );
  }

  async function trash(paths: string[], what: string) {
    if (!paths.length || working) return;
    working = true;
    try {
      const r = await api.trash(paths);
      for (const p of r.ok) {
        overrides.delete(p);
        picked.delete(p);
      }
      if (r.failed.length) {
        const f = r.failed[0];
        store.showToast(`${r.ok.length}장 정리, ${r.failed.length}장 실패 (${baseName(f.path)}: ${f.error})`, {
          error: true,
          undo: r.ok.length > 0,
        });
      } else {
        store.showToast(`${what} ${r.ok.length}장을 휴지통으로 보냈습니다`, { undo: true });
      }
      await store.reload();
      refreshSoon(0);
    } finally {
      working = false;
    }
  }

  async function trashAllRest() {
    const n = restAll.length;
    if (!n) return;
    const ok = await ask(
      `비슷한 묶음 ${report?.groups.length ?? 0}개에서 추천하지 않은 ${n}장(${fmtSize(restAll.reduce((a, m) => a + m.size, 0))})을 휴지통으로 보낼까요?\n휴지통에서 되살리거나 ${navigator.userAgent.includes("Mac") ? "⌘" : "Ctrl+"}Z로 되돌릴 수 있습니다.`,
      { title: "비슷한 사진 정리", kind: "warning", okLabel: "휴지통으로", cancelLabel: "취소" },
    );
    if (ok) await trash(restAll.map((m) => m.path), "추천하지 않은 사진");
  }

  /** 지우지 않고 표시만: 남길 것은 남김, 나머지는 버림 (빠른 고르기 모드에서 이어서 정리) */
  async function markGroups(groups: Group[]) {
    const keep = groups.flatMap((g) => g.members.filter(keepOf).map((m) => m.path));
    const rest = groups.flatMap((g) => restOf(g).map((m) => m.path));
    await store.setMarks(keep, 1);
    await store.setMarks(rest, -1);
    store.showToast(`남김 ${keep.length}장 · 버림 ${rest.length}장으로 표시했습니다 (빠른 고르기 모드에서 확인)`, { undo: true });
    refreshSoon(0);
  }

  const flagged = $derived(
    !report ? [] : tab === "blurry" ? report.blurry : tab === "dark" ? report.dark : tab === "bright" ? report.bright : [],
  );
  const copies = $derived(report ? report.duplicates.flatMap((d) => d.slice(1)) : []);

  function setTab(t: Tab) {
    tab = t;
    picked.clear();
  }

  function togglePick(p: Photo) {
    if (picked.has(p.path)) picked.delete(p.path);
    else picked.add(p.path);
  }

  export function close() {
    store.similar = false;
  }

  /** App에서 넘겨주는 키. 처리했으면 true */
  export function onKey(e: KeyboardEvent): boolean {
    if (e.key === "Escape") {
      close();
      return true;
    }
    return false;
  }

  const thumb = (p: Photo) => thumbUrl(p, -1);
  const label = (n: number, unit = "장") => `${n.toLocaleString()}${unit}`;
</script>

<div class="similar" role="dialog" aria-label="비슷한 사진 정리" tabindex="-1" oncontextmenu={(e) => e.preventDefault()}>
  <div class="topbar">
    <span class="title">비슷한 사진 정리</span>
    <span class="dim">{scopeLabel(scope)}</span>
    {#if analyzing}
      <span class="progress" title="뷰어·비교 화면을 여는 동안에는 잠시 멈춥니다">
        분석 {progress[0].toLocaleString()} / {progress[1].toLocaleString()}
        <span class="bar"><span style="width:{(progress[0] / Math.max(1, progress[1])) * 100}%"></span></span>
      </span>
    {:else if report}
      <span class="dim small">{label(report.analyzed)} 분석 완료</span>
      {#if report.cloud}
        <span
          class="cloud-note"
          title="구글 드라이브·iCloud 등에서 아직 내려받지 않은 사진은 읽으면 내려받기가 시작되므로 분석하지 않습니다. Finder에서 폴더를 '오프라인으로 사용 가능'으로 하거나 사진을 내려받으면 다음에 분석합니다."
        >
          ☁ 클라우드에만 있는 {label(report.cloud)}은 내려받지 않아 분석하지 않음
        </span>
      {/if}
    {/if}
    <span class="spacer"></span>
    <button class="icon-btn" title="닫기 (Esc)" onclick={close}><Icon name="close" /></button>
  </div>

  <div class="controls">
    <label title="얼마나 비슷해야 한 묶음으로 볼지">
      묶는 정도
      <input type="range" min="1" max="5" step="1" bind:value={opts.level} />
      <span class="val">{LEVELS[opts.level]}</span>
    </label>
    <label>
      추천 기준
      <select value={preset} onchange={(e) => (opts.rules = [...PRESETS[Number(e.currentTarget.value)].rules])}>
        {#each PRESETS as p, i (i)}<option value={i}>{p.label}</option>{/each}
      </select>
    </label>
    <label>
      묶음마다 남길 장수
      <select bind:value={opts.keep}>
        {#each [1, 2, 3] as k (k)}<option value={k}>{k}장</option>{/each}
      </select>
    </label>
    <span class="spacer"></span>
    {#if tab === "groups" && report?.groups.length}
      <button class="btn" disabled={working} onclick={() => report && markGroups(report.groups)} title="지우지 않고 남김·버림 표시만 합니다">
        남김·버림 표시만
      </button>
      <button class="btn danger" disabled={!restAll.length || working} onclick={trashAllRest}>
        추천 외 {label(restAll.length)} 휴지통
      </button>
    {:else if tab === "duplicates" && copies.length}
      <button class="btn danger" disabled={working} onclick={() => trash(copies.map((p) => p.path), "사본")}>
        사본 {label(copies.length)} 휴지통
      </button>
    {:else if flagged.length}
      <button class="btn" disabled={working} onclick={() => store.setMarks(picked.size ? [...picked] : flagged.map((p) => p.path), -1).then(() => refreshSoon(0))}>
        {picked.size ? `고른 ${label(picked.size)}` : "모두"} 버림 표시
      </button>
      <button
        class="btn danger"
        disabled={!picked.size || working}
        onclick={() => trash([...picked], tab === "blurry" ? "흐린 사진" : tab === "dark" ? "어두운 사진" : "밝은 사진")}
      >
        고른 {label(picked.size)} 휴지통
      </button>
    {/if}
  </div>

  <div class="tabs" role="tablist">
    <button role="tab" aria-selected={tab === "groups"} class:on={tab === "groups"} onclick={() => setTab("groups")}>
      비슷한 묶음 <b>{report?.groups.length ?? 0}</b>
    </button>
    <button role="tab" aria-selected={tab === "duplicates"} class:on={tab === "duplicates"} onclick={() => setTab("duplicates")}>
      똑같은 파일 <b>{report?.duplicates.length ?? 0}</b>
    </button>
    <button role="tab" aria-selected={tab === "blurry"} class:on={tab === "blurry"} onclick={() => setTab("blurry")}>
      흐림 <b>{report?.blurry.length ?? 0}</b>
    </button>
    <button role="tab" aria-selected={tab === "dark"} class:on={tab === "dark"} onclick={() => setTab("dark")}>
      너무 어두움 <b>{report?.dark.length ?? 0}</b>
    </button>
    <button role="tab" aria-selected={tab === "bright"} class:on={tab === "bright"} onclick={() => setTab("bright")}>
      너무 밝음 <b>{report?.bright.length ?? 0}</b>
    </button>
  </div>

  <div class="body">
    {#if !report}
      <p class="empty">사진을 분석하고 있습니다…</p>
    {:else if tab === "groups"}
      {#if !report.groups.length}
        <p class="empty">
          {analyzing ? "분석하는 대로 묶음이 나타납니다…" : "비슷한 사진이 없습니다. 묶는 정도를 올려 보세요."}
        </p>
      {/if}
      {#each report.groups as g (g.members[0].path)}
        {@const rest = restOf(g)}
        <section class="group">
          <header>
            <span class="count">{g.members.length}장</span>
            <span class="names">{baseName(g.members[0].path)} 외</span>
            <span class="spacer"></span>
            <button class="link" onclick={() => compareGroup(g)} title="크게 나란히 비교 (남길 사진이 미리 골라져 있음)">비교</button>
            <button class="link danger" disabled={!rest.length || working} onclick={() => trash(rest.map((m) => m.path), "추천하지 않은 사진")}>
              나머지 {rest.length}장 휴지통
            </button>
          </header>
          <div class="row">
            {#each g.members as m (m.path)}
              {@const keep = keepOf(m)}
              <button
                class="card"
                class:keep
                class:rest={!keep}
                onclick={() => toggle(m)}
                ondblclick={() => compareGroup(g)}
                title="{baseName(m.path)} · {m.width}×{m.height} · {fmtSize(m.size)}
클릭: 남김/정리 바꾸기 · 더블클릭: 비교"
              >
                <span class="img"><img src={thumb(m)} alt="" loading="lazy" decoding="async" draggable="false" /></span>
                {#if m.keep}<span class="star">★ 추천</span>{/if}
                <span class="state">{keep ? "남김" : "정리"}</span>
                <span class="name">{baseName(m.path)}</span>
                <span class="reasons">{m.reasons.join(" · ") || " "}</span>
              </button>
            {/each}
          </div>
        </section>
      {/each}
    {:else if tab === "duplicates"}
      {#if !report.duplicates.length}<p class="empty">내용이 똑같은 파일이 없습니다.</p>{/if}
      {#each report.duplicates as d (d[0].path)}
        <section class="group">
          <header>
            <span class="count">{d.length}개</span>
            <span class="names">{fmtSize(d[0].size)} · 내용이 똑같은 파일</span>
            <span class="spacer"></span>
            <button class="link danger" disabled={working} onclick={() => trash(d.slice(1).map((p) => p.path), "사본")}>
              사본 {d.length - 1}개 휴지통
            </button>
          </header>
          <div class="row">
            {#each d as p, i (p.path)}
              <div class="card" class:keep={i === 0} class:rest={i > 0} title={p.path}>
                <span class="img"><img src={thumb(p)} alt="" loading="lazy" decoding="async" draggable="false" /></span>
                <span class="state">{i === 0 ? "남김" : "사본"}</span>
                <span class="name">{baseName(p.path)}</span>
                <span class="reasons">{p.path.slice(0, p.path.length - baseName(p.path).length)}</span>
              </div>
            {/each}
          </div>
        </section>
      {/each}
    {:else}
      {#if !flagged.length}
        <p class="empty">해당하는 사진이 없습니다.</p>
      {:else}
        <p class="hint">
          {tab === "blurry"
            ? "이 폴더의 보통 사진보다 훨씬 덜 선명한 사진입니다."
            : tab === "dark"
              ? "대부분이 까맣게 묻힌 사진입니다."
              : "대부분이 하얗게 날아간 사진입니다."}
          클릭해서 고른 뒤 정리하세요. 직접 남김 표시한 사진은 빠집니다.
        </p>
      {/if}
      <div class="flat">
        {#each flagged as p (p.path)}
          <button class="card" class:picked={picked.has(p.path)} onclick={() => togglePick(p)} title={baseName(p.path)}>
            <span class="img"><img src={thumb(p)} alt="" loading="lazy" decoding="async" draggable="false" /></span>
            <span class="check">{picked.has(p.path) ? "✓" : ""}</span>
            <span class="name">{baseName(p.path)}</span>
          </button>
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .similar {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .topbar,
  .controls {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 10px 0 16px;
    white-space: nowrap;
  }
  .topbar {
    height: 44px;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }
  .controls {
    min-height: 44px;
    flex-wrap: wrap;
    padding-top: 6px;
    padding-bottom: 6px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
    color: var(--text-dim);
  }
  .controls label {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .controls input[type="range"] {
    width: 120px;
    accent-color: var(--accent);
  }
  .val {
    min-width: 7em;
    color: var(--text);
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
  .cloud-note {
    font-size: 12px;
    color: #9fc4ff;
  }
  .progress {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .bar {
    width: 120px;
    height: 4px;
    border-radius: 2px;
    background: var(--panel-2);
    overflow: hidden;
  }
  .bar > span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
  .btn.danger {
    background: #3a2224;
    border-color: #5a2c30;
    color: #ffb4b4;
  }
  .btn:disabled {
    opacity: 0.4;
  }
  .tabs {
    display: flex;
    gap: 4px;
    padding: 6px 12px 0;
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    padding: 6px 12px;
    border-radius: 6px 6px 0 0;
    color: var(--text-dim);
    font-size: 13px;
    border-bottom: 2px solid transparent;
  }
  .tabs button.on {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .tabs b {
    margin-left: 4px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .body {
    flex: 1;
    overflow: auto;
    padding: 12px 16px 24px;
  }
  .empty,
  .hint {
    color: var(--text-dim);
    font-size: 13px;
  }
  .empty {
    margin-top: 40px;
    text-align: center;
  }
  .group {
    margin-bottom: 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
    overflow: hidden;
  }
  .group header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    font-size: 12px;
    color: var(--text-dim);
    border-bottom: 1px solid var(--border);
  }
  .count {
    color: var(--text);
    font-weight: 600;
  }
  .link {
    padding: 2px 8px;
    border-radius: 4px;
    color: var(--text-dim);
  }
  .link:hover:not(:disabled) {
    background: var(--hover);
    color: var(--text);
  }
  .link.danger {
    color: #ff9c9c;
  }
  .link:disabled {
    opacity: 0.4;
  }
  .row,
  .flat {
    display: flex;
    gap: 10px;
    padding: 10px 12px;
    overflow-x: auto;
  }
  .flat {
    flex-wrap: wrap;
    padding: 4px 0;
  }
  .card {
    position: relative;
    flex: none;
    width: 168px;
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 4px;
    border-radius: 6px;
    border: 2px solid transparent;
    text-align: left;
  }
  .card:hover {
    background: var(--hover);
  }
  .card.keep {
    border-color: var(--ok);
  }
  .card.rest .img {
    opacity: 0.55;
  }
  .card.picked {
    border-color: var(--danger);
  }
  .img {
    width: 156px;
    height: 117px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #111214;
    border-radius: 4px;
    overflow: hidden;
  }
  .img img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .star {
    position: absolute;
    top: 8px;
    left: 8px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(20, 20, 22, 0.8);
    color: #ffd25a;
    font-size: 11px;
    font-weight: 600;
  }
  .state {
    position: absolute;
    top: 8px;
    right: 8px;
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 11px;
    background: rgba(20, 20, 22, 0.8);
    color: var(--text-dim);
  }
  .card.keep .state {
    color: var(--ok);
  }
  .check {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 18px;
    height: 18px;
    border-radius: 4px;
    background: rgba(20, 20, 22, 0.8);
    border: 1px solid var(--border);
    color: #ff9c9c;
    font-size: 12px;
    line-height: 16px;
    text-align: center;
  }
  .name {
    font-size: 12px;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .reasons {
    font-size: 11px;
    color: var(--text-faint);
    line-height: 1.35;
    min-height: 2.7em;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
</style>
