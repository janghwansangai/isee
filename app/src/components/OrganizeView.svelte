<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { SvelteMap, SvelteSet } from "svelte/reactivity";
  import { listen } from "@tauri-apps/api/event";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { store } from "../lib/store.svelte";
  import {
    api,
    baseName,
    fmtSize,
    isMac,
    thumbUrl,
    type OrganizeItem,
    type OrganizeOptions,
    type OrganizePlan,
    type PlaceLevel,
  } from "../lib/api";
  import Icon from "./Icon.svelte";

  type Basis = "date" | "place" | "both" | "event";

  // 화면을 연 폴더와 고른 항목 (그사이 다른 폴더로 가도 이 기준)
  const folder = store.organize!.folder;
  const picked = store.organize!.paths;
  let onlyPicked = $state(picked.length > 0);
  let recursive = $state(false);
  let opts = $state<OrganizeOptions>($state.snapshot(store.settings.organize));
  let copy = $state(store.settings.organize_copy);
  let root = $state(folder);

  let scan = $state<{ photos: number; videos: number; ms: number } | null>(null);
  let plan = $state.raw<OrganizePlan | null>(null);
  /** 읽기·정리 진행 (끝난 수, 전체) */
  let progress = $state<[number, number] | null>(null);
  let running = $state(false);
  /** 계획 폴더 → 바꾼 이름 */
  const renames = new SvelteMap<string, string>();
  /** 빼기로 한 폴더 */
  const skip = new SvelteSet<string>();

  const PRESETS: Record<Basis, { pattern: string; label: string }[]> = {
    date: [
      { pattern: "{년}/{년}-{월}", label: "년 / 년-월" },
      { pattern: "{년}/{월}-{일}", label: "년 / 월-일" },
      { pattern: "{년}-{월}", label: "년-월" },
      { pattern: "{년}-{월}-{일}", label: "년-월-일" },
      { pattern: "{년}/{년}-{월}/{년}-{월}-{일}", label: "년 / 년-월 / 년-월-일" },
    ],
    place: [
      { pattern: "{장소}", label: "장소" },
      { pattern: "{장소}/{년}-{월}", label: "장소 / 년-월" },
      { pattern: "{년}/{장소}", label: "년 / 장소" },
    ],
    both: [
      { pattern: "{년}-{월}-{일} {장소}", label: "년-월-일 장소" },
      { pattern: "{년}/{년}-{월}-{일} {장소}", label: "년 / 년-월-일 장소" },
      { pattern: "{년}-{월} {장소}", label: "년-월 장소" },
    ],
    event: [
      { pattern: "{기간} {장소}", label: "기간 장소" },
      { pattern: "{년}/{기간} {장소}", label: "년 / 기간 장소" },
    ],
  };
  const BASES: { v: Basis; t: string; hint: string }[] = [
    { v: "date", t: "날짜", hint: "촬영 날짜별 폴더" },
    { v: "place", t: "장소", hint: "GPS 위치의 지명별 폴더" },
    { v: "both", t: "날짜+장소", hint: "날짜와 지명을 함께" },
    { v: "event", t: "이벤트", hint: "촬영 간격으로 여행·행사를 나눔" },
  ];
  const LEVELS: { v: PlaceLevel; t: string; ex: string }[] = [
    { v: "country", t: "나라", ex: "대한민국 · 일본" },
    { v: "province", t: "시·도", ex: "제주 · 일본 오사카" },
    { v: "city", t: "시·군·구", ex: "제주 서귀포시" },
    { v: "town", t: "동네", ex: "제주 서귀포시 중문동" },
  ];
  const PLACE_EX: Record<PlaceLevel, string> = {
    country: "대한민국",
    province: "제주",
    city: "제주 서귀포시",
    town: "제주 서귀포시 중문동",
  };

  let basis = $state<Basis>(basisOf(opts.pattern));
  function basisOf(p: string): Basis {
    if (p.includes("{기간}")) return "event";
    const place = p.includes("{장소}");
    const date = /\{(년|월|일)\}/.test(p);
    return place && date ? (p.split("/").some((s) => s.includes("{장소}") && /\{(년|월|일)\}/.test(s)) ? "both" : "place") : place ? "place" : "date";
  }
  function setBasis(b: Basis) {
    basis = b;
    if (!PRESETS[b].some((p) => p.pattern === opts.pattern)) opts.pattern = PRESETS[b][0].pattern;
  }
  /** 규칙 예시: 2024-05-12 제주에서 찍은 사진 */
  function example(p: string) {
    return p
      .replaceAll("{기간}", "2024-05-12~14")
      .replaceAll("{년}", "2024")
      .replaceAll("{월}", "05")
      .replaceAll("{일}", "12")
      .replaceAll("{장소}", PLACE_EX[opts.level]);
  }
  let usesPlace = $derived(opts.pattern.includes("{장소}"));
  let usesEvent = $derived(opts.pattern.includes("{기간}"));

  // ───── 읽기 → 계획 ─────
  let planTimer = 0;
  async function refreshPlan() {
    try {
      plan = await api.organizePlan($state.snapshot(opts));
    } catch (e) {
      store.showToast(`정리 계획을 만들지 못했습니다: ${e}`, { error: true });
    }
  }

  async function rescan() {
    plan = null;
    scan = null;
    progress = [0, 0];
    try {
      scan = await api.organizeScan(onlyPicked ? picked : [], folder, recursive);
      await refreshPlan();
    } catch (e) {
      store.showToast(`사진 정보를 읽지 못했습니다: ${e}`, { error: true });
    } finally {
      progress = null;
    }
  }

  onMount(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    listen<[number, number]>("organize-progress", (e) => {
      if (progress) progress = e.payload;
    }).then((u) => (alive ? (unlisten = u) : u()));
    return () => {
      alive = false;
      unlisten?.();
      clearTimeout(planTimer);
    };
  });

  // 대상(고른 것만 / 하위 폴더 포함)이 바뀌면 다시 읽는다
  $effect(() => {
    void [onlyPicked, recursive];
    untrack(() => rescan());
  });

  // 규칙을 바꾸면 읽어 둔 정보로 바로 다시 계산 (파일을 다시 읽지 않음)
  let firstOpts = true;
  $effect(() => {
    void [opts.pattern, opts.level, opts.guess_place, opts.event_hours];
    if (firstOpts) {
      firstOpts = false;
      return;
    }
    untrack(() => {
      if (!scan) return;
      renames.clear();
      skip.clear();
      clearTimeout(planTimer);
      planTimer = window.setTimeout(refreshPlan, 60);
    });
  });

  // ───── 실행 ─────
  let included = $derived(plan ? plan.folders.filter((f) => !skip.has(f.rel)) : []);
  let fileCount = $derived(included.reduce((a, f) => a + f.items.length, 0));
  let byteCount = $derived(included.reduce((a, f) => a + f.bytes, 0));

  async function pickRoot() {
    const p = await openDialog({ directory: true, title: "정리할 위치", defaultPath: root });
    if (typeof p === "string") root = p;
  }

  async function run() {
    if (!plan || !fileCount || running) return;
    running = true;
    progress = [0, fileCount];
    try {
      const r = await api.organizeApply(
        root,
        $state.snapshot(opts),
        Object.fromEntries([...renames].filter(([a, b]) => b.trim() && a !== b)),
        [...skip],
        copy,
      );
      store.setSettings({ organize_copy: copy });
      const verb = copy ? "복사" : "정리";
      let msg = `${r.files.toLocaleString()}개를 ${verb}했습니다 · 새 폴더 ${r.folders}개 · ${(r.ms / 1000).toFixed(1)}초`;
      if (r.unchanged) msg += ` · 이미 제자리 ${r.unchanged}장`;
      if (r.failed.length) msg += ` · ${r.failed.length}개 실패 (${baseName(r.failed[0].path)}: ${r.failed[0].error})`;
      store.showToast(msg, { undo: r.files > 0, error: r.failed.length > 0 });
      store.organize = null;
      await store.reload();
    } catch (e) {
      store.showToast(`정리하지 못했습니다: ${e}`, { error: true });
    } finally {
      running = false;
      progress = null;
    }
  }

  export function close() {
    if (!running) store.organize = null;
  }

  /** App에서 넘겨주는 키. 처리했으면 true */
  export function onKey(e: KeyboardEvent): boolean {
    if (e.key === "Escape") {
      close();
      return true;
    }
    return false;
  }

  const isVideo = (p: string) => /\.(mp4|mov|m4v|avi|mkv|wmv|webm|mts|m2ts|3gp|flv)$/i.test(p);
  const sample = (items: OrganizeItem[]) => items.slice(0, 6);
  const names = (items: OrganizeItem[], pred: (i: OrganizeItem) => boolean) =>
    items
      .filter(pred)
      .slice(0, 12)
      .map((i) => baseName(i.path))
      .join("\n");
  let all = $derived(plan ? plan.folders.flatMap((f) => f.items) : []);
</script>

<div class="organize" role="dialog" aria-label="날짜·장소별 정리" tabindex="-1" oncontextmenu={(e) => e.preventDefault()}>
  <div class="topbar">
    <span class="title">날짜·장소별 정리</span>
    <span class="dim">{baseName(folder)}</span>
    {#if progress && !running}
      <span class="progress">
        사진 정보 읽는 중 {progress[0].toLocaleString()} / {progress[1].toLocaleString()}
        <span class="bar"><span style="width:{(progress[0] / Math.max(1, progress[1])) * 100}%"></span></span>
      </span>
    {:else if scan}
      <span class="dim small">
        사진 {scan.photos.toLocaleString()}장{scan.videos ? ` · 동영상 ${scan.videos.toLocaleString()}개` : ""} · {(scan.ms / 1000).toFixed(1)}초
      </span>
    {/if}
    <span class="spacer"></span>
    <button class="icon-btn" title="닫기 (Esc)" onclick={close}><Icon name="close" /></button>
  </div>

  <div class="main">
    <aside class="side">
      <section>
        <h3>대상</h3>
        {#if picked.length}
          <label class="check"><input type="checkbox" bind:checked={onlyPicked} /> 고른 {picked.length}개만</label>
        {/if}
        {#if !onlyPicked}
          <label class="check"><input type="checkbox" bind:checked={recursive} /> 하위 폴더의 사진도 포함</label>
        {/if}
        <p class="note">사진과 동영상을 정리합니다. RAW+JPEG, 라이브 포토(HEIC+MOV), .xmp·.aae는 함께 옮깁니다.</p>
      </section>

      <section>
        <h3>기준</h3>
        <div class="seg" role="group" aria-label="기준">
          {#each BASES as b (b.v)}
            <button class:on={basis === b.v} title={b.hint} onclick={() => setBasis(b.v)}>{b.t}</button>
          {/each}
        </div>
      </section>

      <section>
        <h3>폴더 이름 규칙</h3>
        <div class="presets">
          {#each PRESETS[basis] as p (p.pattern)}
            <label class="preset" class:on={opts.pattern === p.pattern}>
              <input type="radio" name="pattern" value={p.pattern} bind:group={opts.pattern} />
              <span class="ex">{example(p.pattern)}</span>
              <span class="lbl">{p.label}</span>
            </label>
          {/each}
        </div>
        <label class="custom">
          직접 입력
          <input type="text" bind:value={opts.pattern} spellcheck="false" />
        </label>
        <p class="note">{"{년} {월} {일} {장소} {기간}"} · "/"로 하위 폴더</p>
      </section>

      {#if usesPlace}
        <section>
          <h3>장소 단위</h3>
          <div class="seg" role="group" aria-label="장소 단위">
            {#each LEVELS as l (l.v)}
              <button class:on={opts.level === l.v} title={l.ex} onclick={() => (opts.level = l.v)}>{l.t}</button>
            {/each}
          </div>
          <label class="check">
            <input type="checkbox" bind:checked={opts.guess_place} />
            위치 없는 사진은 30분 안에 찍은 앞뒤 사진의 위치로
          </label>
          <p class="note">지명은 앱에 들어 있는 데이터로 찾습니다. 위치가 인터넷으로 나가지 않습니다.</p>
        </section>
      {/if}

      {#if usesEvent}
        <section>
          <h3>이벤트 나누기</h3>
          <label class="row">
            촬영 간격이
            <select bind:value={opts.event_hours}>
              {#each [2, 4, 6, 12, 24, 48] as h (h)}<option value={h}>{h}시간</option>{/each}
            </select>
            넘으면 새 폴더
          </label>
        </section>
      {/if}

      <section>
        <h3>방법</h3>
        <div class="seg" role="group" aria-label="이동 또는 복사">
          <button class:on={!copy} onclick={() => (copy = false)}>이동</button>
          <button class:on={copy} onclick={() => (copy = true)}>복사</button>
        </div>
        <div class="root">
          <span class="path" title={root}>{root === folder ? `이 폴더 안 (${baseName(folder)})` : root}</span>
          <button class="link" onclick={pickRoot}>위치 바꾸기…</button>
          {#if root !== folder}<button class="link" onclick={() => (root = folder)}>원래대로</button>{/if}
        </div>
      </section>
    </aside>

    <div class="preview">
      {#if !plan}
        <p class="empty">{progress ? "사진의 촬영 날짜와 위치를 읽고 있습니다…" : "정리할 사진이 없습니다."}</p>
      {:else}
        <div class="summary">
          <b>폴더 {included.length}개</b> · {fileCount.toLocaleString()}장 · {fmtSize(byteCount)}
          {#if plan.companions}<span class="dim"> · 짝 파일 {plan.companions}개 함께</span>{/if}
        </div>
        <div class="flags">
          {#if plan.by_mtime}
            <span class="flag warn" title={names(all, (i) => i.date_source === "modified")}>
              날짜를 파일 수정 시각으로 정한 사진 {plan.by_mtime}장 (실제 촬영일과 다를 수 있음)
            </span>
          {/if}
          {#if plan.by_name}
            <span class="flag" title={names(all, (i) => i.date_source === "filename")}>파일 이름에서 날짜를 찾은 사진 {plan.by_name}장</span>
          {/if}
          {#if usesPlace}
            <span class="flag">위치 있는 사진 {plan.with_gps}장</span>
            {#if plan.guessed}
              <span class="flag" title={names(all, (i) => i.place_guessed)}>위치를 앞뒤 사진으로 추정 {plan.guessed}장</span>
            {/if}
            {#if plan.no_place}
              <span class="flag warn" title={names(all, (i) => !i.place)}>위치 없음 {plan.no_place}장</span>
            {/if}
          {/if}
        </div>
        <div class="folders">
          {#each plan.folders as f (f.rel)}
            {@const off = skip.has(f.rel)}
            <div class="folder" class:off>
              <input
                type="checkbox"
                checked={!off}
                title="이 폴더 포함"
                onchange={(e) => (e.currentTarget.checked ? skip.delete(f.rel) : skip.add(f.rel))}
              />
              <Icon name="dir" size={16} />
              <input
                class="name"
                type="text"
                value={renames.get(f.rel) ?? f.rel}
                spellcheck="false"
                title="폴더 이름을 고칠 수 있습니다 (/로 하위 폴더)"
                onchange={(e) => renames.set(f.rel, e.currentTarget.value)}
              />
              <span class="count">{f.items.length}장</span>
              <span class="size">{fmtSize(f.bytes)}</span>
              <span class="thumbs">
                {#each sample(f.items) as it (it.path)}
                  {#if isVideo(it.path)}
                    <span class="thumb video" title={baseName(it.path)}>▶</span>
                  {:else}
                    <img class="thumb" src={thumbUrl(it, -1)} alt="" loading="lazy" decoding="async" title={baseName(it.path)} />
                  {/if}
                {/each}
                {#if f.items.length > 6}<span class="more">+{f.items.length - 6}</span>{/if}
              </span>
            </div>
          {/each}
        </div>
      {/if}
      <div class="actions">
        {#if running && progress}
          <span class="progress">
            {copy ? "복사" : "정리"} 중 {progress[0].toLocaleString()} / {progress[1].toLocaleString()}
            <span class="bar"><span style="width:{(progress[0] / Math.max(1, progress[1])) * 100}%"></span></span>
          </span>
        {:else}
          <span class="dim small">실행한 정리는 {isMac ? "⌘Z" : "Ctrl+Z"} 한 번으로 모두 되돌립니다</span>
        {/if}
        <span class="spacer"></span>
        <button class="btn" disabled={running} onclick={close}>취소</button>
        <button class="btn primary" disabled={!fileCount || running} onclick={run}>
          {fileCount.toLocaleString()}장 {copy ? "복사하기" : "정리하기"}
        </button>
      </div>
    </div>
  </div>
</div>

<style>
  .organize {
    position: fixed;
    inset: 0;
    z-index: 50;
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
    width: 330px;
    flex: none;
    overflow-y: auto;
    padding: 6px 16px 20px;
    border-right: 1px solid var(--border);
    background: var(--panel);
  }
  section {
    padding: 10px 0;
    border-bottom: 1px solid var(--border);
  }
  section:last-child {
    border-bottom: none;
  }
  h3 {
    margin: 0 0 8px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
  }
  .note {
    margin: 6px 0 0;
    font-size: 11px;
    color: var(--text-faint);
    line-height: 1.45;
  }
  .check,
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 6px 0;
    font-size: 13px;
  }
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
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
  .presets {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .preset {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 8px;
    padding: 6px 8px;
    border-radius: 6px;
    border: 1px solid transparent;
  }
  .preset:hover {
    background: var(--hover);
  }
  .preset.on {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .preset input {
    grid-row: span 2;
    accent-color: var(--accent);
  }
  .ex {
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }
  .lbl {
    font-size: 11px;
    color: var(--text-faint);
  }
  .custom {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .custom input {
    flex: 1;
    min-width: 0;
  }
  input[type="text"] {
    padding: 4px 6px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--panel-2);
  }
  .root {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 8px;
    font-size: 12px;
  }
  .path {
    flex: 1 1 100%;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .link {
    padding: 2px 6px;
    border-radius: 4px;
    color: var(--accent);
  }
  .link:hover {
    background: var(--hover);
  }
  .preview {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .summary {
    padding: 12px 16px 4px;
    font-size: 13px;
  }
  .flags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 16px 10px;
    border-bottom: 1px solid var(--border);
  }
  .flag {
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 11px;
    background: var(--panel-2);
    color: var(--text-dim);
  }
  .flag.warn {
    background: #3a3020;
    color: #ffd27a;
  }
  .folders {
    flex: 1;
    overflow-y: auto;
    padding: 6px 8px;
  }
  .folder {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    border-radius: 6px;
    color: var(--text-dim);
  }
  .folder:hover {
    background: var(--hover);
  }
  .folder.off {
    opacity: 0.45;
  }
  .folder .name {
    flex: 0 1 300px;
    min-width: 120px;
    color: var(--text);
    background: transparent;
    border-color: transparent;
  }
  .folder .name:hover,
  .folder .name:focus {
    border-color: var(--border);
    background: var(--panel-2);
  }
  .count,
  .size {
    width: 64px;
    flex: none;
    font-size: 12px;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .thumbs {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-left: 8px;
    min-width: 0;
    overflow: hidden;
  }
  .thumb {
    width: 44px;
    height: 33px;
    flex: none;
    object-fit: cover;
    border-radius: 3px;
    background: #111214;
  }
  .thumb.video {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    color: var(--text-faint);
  }
  .more {
    font-size: 11px;
    color: var(--text-faint);
  }
  .empty {
    margin-top: 60px;
    text-align: center;
    color: var(--text-dim);
    font-size: 13px;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-top: 1px solid var(--border);
    background: var(--panel);
  }
  .btn:disabled {
    opacity: 0.4;
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
</style>
