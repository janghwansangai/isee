<script lang="ts">
  // 클라우드에만 있는 사진: 내려받아 분석하기(묻고 시작), 내려받는 동안 진행·남은 시간, 끝나면 "다운로드 제거".
  // 비슷한 사진·인물 화면이 함께 쓴다. 내려받기는 앱 전체에 하나라 어느 화면에서든 같은 진행을 보여 준다.
  import { untrack } from "svelte";
  import { store } from "../lib/store.svelte";
  import { api, fmtSize, type CloudEstimate, type Scope } from "../lib/api";

  let {
    scope,
    faces = false,
    banner = false,
    refresh = 0,
    count = $bindable(0),
  }: {
    scope: Scope;
    /** 얼굴 분석 기준으로 셈 (아니면 비슷한 사진 기준) */
    faces?: boolean;
    /** 화면 위 띠로 보여 줌 (아니면 제목줄 안에) */
    banner?: boolean;
    /** 바뀌면 다시 센다 (분석한 수 등) */
    refresh?: number;
    /** 클라우드에만 있어 아직 분석하지 않은 수 (화면이 남은 사진 안내에서 뺀다) */
    count?: number;
  } = $props();

  let est = $state.raw<CloudEstimate | null>(null);
  let working = $state(false);
  let p = $derived(store.cloud ?? est?.progress ?? null);
  let fetching = $derived(!!p && p.done < p.total);

  let timer = 0;
  async function load() {
    try {
      est = await api.cloudEstimate(scope, faces);
      count = est.count;
      if (!store.cloud) store.cloud = est.progress;
    } catch {
      est = null;
    }
  }
  function loadSoon(ms = 800) {
    clearTimeout(timer);
    timer = window.setTimeout(load, ms);
  }
  $effect(() => {
    void [refresh, scope.path, scope.recursive, scope.extra.join()];
    untrack(() => loadSoon(est ? 800 : 0));
    return () => clearTimeout(timer);
  });
  // 내려받기가 끝나면 다시 센다 (남은 사진·내려받은 사진)
  let wasFetching = false;
  $effect(() => {
    const f = fetching;
    if (wasFetching && !f) untrack(() => loadSoon(300));
    wasFetching = f;
  });

  async function startFetch() {
    working = true;
    try {
      await store.fetchCloud(scope, faces);
    } finally {
      working = false;
    }
  }
  async function evict() {
    if (!est) return;
    working = true;
    try {
      if (await store.evictCloud(est.downloaded[0], est.downloaded[1], est.provider)) await load();
    } finally {
      working = false;
    }
  }

  /** "약 12분 남음 · 오후 3:40쯤 끝" */
  let eta = $derived.by(() => {
    if (!p || !fetching) return "";
    if (p.paused) return "멈춤";
    if (p.eta_secs == null) return "남은 시간 계산 중…";
    const at = new Date(Date.now() + p.eta_secs * 1000).toLocaleTimeString("ko-KR", { hour: "numeric", minute: "2-digit" });
    const min = Math.round(p.eta_secs / 60);
    const left = p.eta_secs < 60 ? "1분 안에 끝" : min < 60 ? `약 ${min}분 남음` : `약 ${Math.floor(min / 60)}시간 ${min % 60}분 남음`;
    return `${left} · ${at}쯤 끝`;
  });

  let show = $derived(fetching || (!!est && (est.count > 0 || est.downloaded[0] > 0)));
  const label = (n: number) => `${n.toLocaleString()}장`;
</script>

{#if show}
  <div class="cloudbar" class:banner>
    {#if fetching && p}
      <span class="note" title="백그라운드에서 2장씩 내려받습니다. 화면을 닫거나 다른 사진을 봐도 계속하고, 다 되면 알려 드립니다.">
        ☁ 클라우드 사진 내려받는 중 {p.done.toLocaleString()} / {p.total.toLocaleString()}장
        ({fmtSize(p.bytes_done)} / {fmtSize(p.bytes_total)}) · {eta}
      </span>
      <span class="bar"><span style="width:{(p.bytes_done / Math.max(1, p.bytes_total)) * 100}%"></span></span>
      <button class="btn small" disabled={working} onclick={() => api.cloudStop()} title="남은 사진은 내려받지 않습니다. 받은 사진은 분석합니다.">중단</button>
    {:else if est}
      {#if est.count > 0}
        <span
          class="note"
          title="구글 드라이브·iCloud 등에서 아직 내려받지 않은 사진은 읽으면 내려받기가 시작되므로 저절로 분석하지 않습니다. '내려받아 분석'을 누르면 내려받은 뒤 분석합니다."
        >
          ☁ 클라우드에만 있는 {label(est.count)}({fmtSize(est.bytes)})은 아직 분석하지 않음
        </span>
        <button class="btn small primary" disabled={working} onclick={startFetch}>내려받아 분석…</button>
      {/if}
      {#if est.downloaded[0] > 0}
        <span class="note" title="iSee가 분석하려고 이 컴퓨터로 내려받은 사진입니다. 다운로드를 제거해도 클라우드의 사진과 분석 결과는 그대로입니다.">
          {est.count > 0 ? " · " : "☁ "}분석하려고 내려받은 사진 {label(est.downloaded[0])}({fmtSize(est.downloaded[1])})
        </span>
        {#if est.can_evict}
          <button class="btn small" disabled={working} onclick={evict}>다운로드 제거…</button>
        {:else}
          <span class="note dim" title="iSee가 대신 지울 수 없는 컴퓨터입니다.">· 파일 탐색기에서 폴더를 오른쪽 클릭 → "공간 확보"로 지울 수 있습니다</span>
        {/if}
      {/if}
    {/if}
  </div>
{/if}

<style>
  .cloudbar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .cloudbar.banner {
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }
  .note {
    font-size: 12px;
    color: #9fc4ff;
    font-variant-numeric: tabular-nums;
  }
  .dim {
    color: var(--text-faint);
  }
  .btn.small {
    padding: 2px 10px;
    font-size: 12px;
  }
  .bar {
    width: 100px;
    height: 4px;
    border-radius: 2px;
    background: var(--panel-2);
    overflow: hidden;
  }
  .bar > span {
    display: block;
    height: 100%;
    background: #9fc4ff;
  }
</style>
