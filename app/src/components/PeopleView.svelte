<script lang="ts">
  // 인물 (8단계, 선택 모듈): 얼굴을 사람별로 묶어 보여 주고, 이름 붙이기·합치기·빼기·인물별 폴더 정리.
  import { onMount, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { listen } from "@tauri-apps/api/event";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { store } from "../lib/store.svelte";
  import { api, baseName, faceUrl, fmtSize, isMac, type PeopleReport, type PersonGroup, type Settings, scopeLabel } from "../lib/api";
  import Icon from "./Icon.svelte";
  import CloudBar from "./CloudBar.svelte";

  // 연 범위 (지금 폴더, 함께 보는 다른 폴더). 하위 폴더 포함은 이 화면에서도 바꿀 수 있다.
  const base = store.people!;
  const folder = base.path;
  let installed = $state<boolean | null>(null);
  let download = $state(0);
  let installing = $state<[number, number] | null>(null);
  let recursive = $state(base.recursive);
  let scope = $derived({ ...base, recursive });
  let progress = $state<[number, number]>([0, 0]);
  let report = $state.raw<PeopleReport | null>(null);
  /** 고른 묶음 (사람 번호 또는 "u:대표 얼굴") */
  let selKey = $state<string | null>(null);
  const picked = new SvelteSet<number>();
  let nameInput = $state("");
  let working = $state(false);

  /** 목록의 한 줄: 사람 묶음, 또는 맨 끝의 "미분류" (묶지 못한 얼굴) */
  type ListGroup = PersonGroup & { unsorted?: true };
  const UNSORTED = "x:unsorted";
  const keyOf = (g: ListGroup) => (g.unsorted ? UNSORTED : g.person != null ? `p:${g.person}` : `u:${g.cover}`);
  let groups = $derived.by((): ListGroup[] => {
    const r = report;
    if (!r) return [];
    if (!r.unsorted.length) return r.groups;
    const photos = new Set(r.unsorted.map((f) => f.path)).size;
    return [...r.groups, { person: null, name: null, faces: r.unsorted, cover: r.unsorted[0].id, photos, auto: 0, unsorted: true }];
  });
  let unnamedNo = $derived.by(() => {
    const m = new Map<string, number>();
    let n = 0;
    for (const g of report?.groups ?? []) if (g.person == null) m.set(keyOf(g), ++n);
    return m;
  });
  const title = (g: ListGroup) => (g.unsorted ? "미분류" : (g.name ?? `이름 없는 인물 ${unnamedNo.get(keyOf(g)) ?? ""}`));
  let sel = $derived(groups.find((g) => keyOf(g) === selKey) ?? null);
  let named = $derived((report?.groups ?? []).filter((g) => g.person != null));
  let analyzing = $derived(progress[1] > 0 && progress[0] < progress[1]);

  // ───── 보이는 부분만 그리기 ─────
  // 얼굴이 수천 개여도 화면에 보이는 줄만 만들어, 썸네일도 보이는 것만 요청한다
  const FACE_W = 120;
  const FACE_H = 142;
  const GAP = 10;
  const ROW_H = FACE_H + GAP;
  /** 왼쪽 인물 목록 한 줄 */
  const ITEM_H = 56;
  const OVERSCAN = 2;
  let faceScroll = $state<HTMLElement>();
  let faceY = $state(0);
  let faceH = $state(0);
  let faceW = $state(0);
  let listY = $state(0);
  let listH = $state(0);
  let cols = $derived(Math.max(1, Math.floor((faceW + GAP) / (FACE_W + GAP))));
  let shownFaces = $derived.by(() => {
    const faces = sel?.faces ?? [];
    const first = Math.max(0, Math.floor(faceY / ROW_H) - OVERSCAN) * cols;
    const last = Math.min(faces.length, (Math.ceil((faceY + faceH) / ROW_H) + OVERSCAN) * cols);
    return faces.slice(first, last).map((f, k) => ({ f, i: first + k }));
  });
  let shownGroups = $derived.by(() => {
    const first = Math.max(0, Math.floor(listY / ITEM_H) - OVERSCAN * 2);
    const last = Math.min(groups.length, Math.ceil((listY + listH) / ITEM_H) + OVERSCAN * 2);
    return groups.slice(first, last).map((g, k) => ({ g, i: first + k }));
  });

  // 다른 묶음을 고르면 얼굴 목록을 맨 위로
  $effect(() => {
    void selKey;
    untrack(() => {
      if (faceScroll) faceScroll.scrollTop = 0;
      faceY = 0;
    });
  });

  // 고른 묶음이 바뀌면 이름 칸을 맞춘다
  $effect(() => {
    const g = sel;
    untrack(() => {
      nameInput = g?.name ?? "";
      picked.clear();
    });
  });

  // ───── 보고서 ─────
  let timer = 0;
  let lastAt = 0;
  // 묶기는 한 번에 하나만: 계산하는 동안 다시 부르면 끝난 뒤 한 번만 더 한다
  // (얼굴이 많으면 묶기가 몇 초 걸리는데, 그사이 요청이 쌓이면 메모리가 늘어 멈출 수 있다)
  let chain: Promise<void> = Promise.resolve();
  let queued = 0;
  function refresh(): Promise<void> {
    if (queued > 1) return chain;
    queued++;
    chain = chain.then(load).finally(() => queued--);
    return chain;
  }
  async function load() {
    try {
      const r = await api.facesReport(scope);
      report = r;
      lastAt = performance.now();
      const keys = [...r.groups.map(keyOf), ...(r.unsorted.length ? [UNSORTED] : [])];
      if (!selKey || !keys.includes(selKey)) selKey = keys[0] ?? null;
    } catch (e) {
      store.showToast(`인물을 불러오지 못했습니다: ${e}`, { error: true });
    }
  }
  function refreshSoon(ms = 100) {
    clearTimeout(timer);
    timer = window.setTimeout(refresh, ms);
  }

  /** 클라우드에만 있는 사진 (분석 시작으로는 할 수 없고 "내려받아 분석"으로) */
  let cloudCount = $state(0);
  /** 남은 사진 수와 예상 시간 (사진 한 장 약 0.1초, 작업 3개 동시) */
  let remaining = $derived(report ? Math.max(0, report.total - report.analyzed - cloudCount) : 0);
  let estimate = $derived.by(() => {
    const sec = Math.ceil((remaining * 0.1) / 3);
    return sec < 60 ? `약 ${Math.max(1, sec)}초` : `약 ${Math.ceil(sec / 60)}분`;
  });

  async function stop() {
    await api.facesStop();
    progress = [0, 0];
    refreshSoon(0);
  }

  async function start() {
    // 사진이 아주 많거나 클라우드 드라이브 폴더면 먼저 알린다
    if (!(await store.confirmAnalysis(scope, "faces"))) return;
    try {
      progress = await api.facesAnalyze(scope);
      await refresh();
    } catch (e) {
      store.showToast(`${e}`, { error: true });
    }
  }

  onMount(() => {
    let alive = true;
    const un: Promise<() => void>[] = [
      listen<[number, number]>("faces-progress", (e) => {
        progress = e.payload;
        if (e.payload[0] >= e.payload[1] || performance.now() - lastAt > 2000) refreshSoon(0);
      }),
      listen<[number, number]>("faces-install", (e) => {
        if (installing) installing = e.payload;
      }),
    ];
    api.facesStatus().then((s) => {
      if (!alive) return;
      installed = s.installed;
      download = s.download;
      // 분석은 저절로 시작하지 않는다: 이미 분석한 결과만 보여 주고, 남은 사진은 "분석 시작"으로
      if (s.installed) refresh();
      if (s.analyzing[1] && s.analyzing[0] < s.analyzing[1]) progress = s.analyzing;
    });
    // 화면을 닫아도 분석은 이어서 한다 (멈추려면 "분석 멈춤" 또는 백그라운드 작업에서)
    return () => {
      alive = false;
      un.forEach((p) => p.then((f) => f()));
      clearTimeout(timer);
      // 아직 만들지 못한 얼굴 썸네일은 더 만들지 않는다
      api.facesCropsCancel().catch(() => undefined);
    };
  });

  let firstScope = true;
  $effect(() => {
    void recursive;
    if (firstScope) {
      firstScope = false;
      return;
    }
    untrack(() => installed && refresh());
  });

  async function install() {
    installing = [0, download];
    try {
      await api.facesInstall();
      installed = true;
      store.showToast("얼굴 분류 기능을 설치했습니다");
      await refresh();
    } catch (e) {
      store.showToast(`설치하지 못했습니다: ${e}`, { error: true });
    } finally {
      installing = null;
    }
  }

  // ───── 이름·합치기·빼기 ─────
  async function act(fn: () => Promise<unknown>) {
    if (working) return;
    working = true;
    try {
      await fn();
      await refresh();
    } catch (e) {
      store.showToast(`${e}`, { error: true });
    } finally {
      working = false;
    }
  }

  /** 이름 붙이기: 이름 없는 묶음은 고른 얼굴이 있으면 그 얼굴만(없으면 전부), 미분류는 고른 얼굴만.
   *  이름 붙은 사람이면 이름만 바꾼다. 같은 이름의 사람이 이미 있으면 그 사람으로 합친다. */
  function saveName() {
    const g = sel;
    const name = nameInput.trim();
    if (!g || !name || (g.unsorted && !picked.size)) return;
    const same = named.find((n) => n.name === name && n.person !== g.person);
    const some = g.person == null && picked.size > 0;
    act(async () => {
      const ids = some ? [...picked] : g.faces.map((f) => f.id);
      const id = await api.facesName(ids, same?.person ?? g.person, same ? null : name);
      // 미분류·일부만 붙였으면 남은 얼굴을 계속 보여 준다
      if (!some) selKey = `p:${id}`;
      else nameInput = "";
    });
  }

  function mergeInto(person: number) {
    const g = sel;
    if (!g || (g.unsorted && !picked.size)) return;
    act(async () => {
      const ids = picked.size ? [...picked] : g.faces.map((f) => f.id);
      await api.facesName(ids, person, null);
      if (!picked.size) selKey = `p:${person}`;
    });
  }

  // ───── 묶는 방식 (정확도·옆모습) ─────
  async function setOption(patch: Partial<Settings>) {
    await store.setSettings(patch);
    await refresh();
  }

  /** 옆모습을 빼려면 예전에(0.1.6 전) 분석한 얼굴은 방향을 알도록 다시 분석해야 한다 */
  async function redoTurn() {
    try {
      const n = await api.facesRedoTurn(scope);
      if (!n) {
        store.showToast("다시 분석할 사진이 없습니다 (클라우드에만 있는 사진은 내려받아야 합니다)");
        return;
      }
      await refresh();
      await start();
    } catch (e) {
      store.showToast(`${e}`, { error: true });
    }
  }

  /** 고른 얼굴(없으면 자동으로 들어온 것 전부) "이 사람 아님" */
  function rejectPicked() {
    const g = sel;
    if (!g || g.person == null || !picked.size) return;
    act(() => api.facesReject([...picked], g.person!));
  }

  /** 이름 없는 묶음에서 고른 얼굴 빼기: 남은 얼굴과 다시 묶지 않는다 (진짜 그 사람 묶음으로는 갈 수 있다) */
  async function splitPicked() {
    const g = sel;
    if (!g || g.person != null || !picked.size) return;
    const others = g.faces.filter((f) => !picked.has(f.id)).map((f) => f.id);
    await act(() => api.facesApart([...picked], others));
    // 대표 얼굴을 뺐으면 묶음 번호가 바뀌므로, 남은 얼굴이 있는 묶음을 계속 보여 준다
    const left = report?.groups.find((x) => x.faces.some((f) => f.id === others[0]));
    if (left) selKey = keyOf(left);
  }

  /** 자동으로 들어온 얼굴을 모두 "맞음"으로 확인 */
  function confirmAuto() {
    const g = sel;
    if (!g || g.person == null) return;
    act(() => api.facesName(g.faces.filter((f) => f.person == null).map((f) => f.id), g.person, null));
  }

  async function deletePerson() {
    const g = sel;
    if (!g || g.person == null) return;
    const ok = await ask(`"${g.name}" 이름을 지울까요? 얼굴은 다시 "이름 없는 인물"로 묶입니다.`, {
      title: "인물 지우기",
      kind: "warning",
      okLabel: "지우기",
      cancelLabel: "취소",
    });
    if (ok) act(() => api.facesDeletePerson(g.person!));
  }

  async function clearAll() {
    const ok = await ask("이 컴퓨터에 저장된 모든 얼굴 데이터와 인물 이름을 지울까요? 되돌릴 수 없습니다.", {
      title: "얼굴 데이터 지우기",
      kind: "warning",
      okLabel: "모두 지우기",
      cancelLabel: "취소",
    });
    if (!ok) return;
    await act(() => api.facesClear());
    selKey = null;
  }

  // ───── 인물별 폴더 ─────
  async function organize(mode: "copy" | "main") {
    if (!named.length) {
      store.showToast("먼저 인물에 이름을 붙여 주세요 (이름이 폴더 이름이 됩니다)");
      return;
    }
    const msg =
      mode === "copy"
        ? `이름 붙은 인물 ${named.length}명의 사진을 "${baseName(folder)}" 안 인물별 폴더로 복사합니다. 여러 사람이 나온 사진은 사람 수만큼 복사됩니다.`
        : `사진을 가장 크게 나온 인물의 폴더로 옮깁니다. 비슷한 크기로 여러 명이 나온 사진은 "단체 사진" 폴더로 갑니다.`;
    const ok = await ask(`${msg}\n${isMac ? "⌘Z" : "Ctrl+Z"}로 한 번에 되돌릴 수 있습니다.`, {
      title: mode === "copy" ? "인물별 폴더로 복사" : "주인공 기준 이동",
      okLabel: mode === "copy" ? "복사" : "이동",
      cancelLabel: "취소",
    });
    if (!ok) return;
    working = true;
    try {
      const r = await api.facesOrganize(scope, folder, mode, []);
      let m = `${r.files}개를 ${mode === "copy" ? "복사" : "옮겼"}했습니다 · 새 폴더 ${r.folders}개`;
      if (r.failed.length) m += ` · ${r.failed.length}개 실패`;
      store.showToast(m, { undo: r.files > 0, error: r.failed.length > 0 });
      await store.reload();
      await refresh();
    } catch (e) {
      store.showToast(`정리하지 못했습니다: ${e}`, { error: true });
    } finally {
      working = false;
    }
  }

  /** 지금 폴더에 있는 사진이면 뷰어로 크게 */
  function openPhoto(path: string) {
    const i = store.entries.findIndex((e) => e.path === path);
    if (i >= 0) {
      store.selectOnly(i);
      store.viewer = i;
    }
  }

  function togglePick(id: number) {
    if (picked.has(id)) picked.delete(id);
    else picked.add(id);
  }

  export function close() {
    store.people = null;
  }

  export function onKey(e: KeyboardEvent): boolean {
    if (e.key === "Escape") {
      if (picked.size) picked.clear();
      else close();
      return true;
    }
    return false;
  }
</script>

<div class="people" role="dialog" aria-label="인물" tabindex="-1" oncontextmenu={(e) => e.preventDefault()}>
  <div class="topbar">
    <span class="title">인물</span>
    <span class="dim">{scopeLabel({ ...base, recursive: false })}</span>
    {#if installed}
      <label class="check"><input type="checkbox" bind:checked={recursive} /> 하위 폴더 포함</label>
      <label class="opt" title="엄격할수록 다른 사람이 섞이는 일이 줄고, 대신 미분류로 남는 얼굴이 늘어납니다. 다시 분석하지 않고 바로 다시 묶습니다.">
        정확도
        <select value={store.settings.face_accuracy} onchange={(e) => setOption({ face_accuracy: e.currentTarget.value as Settings["face_accuracy"] })}>
          <option value="low">낮음 (많이 묶기)</option>
          <option value="normal">보통</option>
          <option value="high">높음 (확실한 것만)</option>
        </select>
      </label>
      <label class="opt" title="옆모습은 다른 사람과 섞이기 쉽습니다. 뺀 얼굴은 미분류로 갑니다. 이름을 직접 붙인 얼굴은 그대로 둡니다.">
        옆모습
        <select value={store.settings.face_profile} onchange={(e) => setOption({ face_profile: e.currentTarget.value as Settings["face_profile"] })}>
          <option value="all">모두 묶기</option>
          <option value="half">많이 돌린 얼굴 빼기</option>
          <option value="front">정면에 가까운 얼굴만</option>
        </select>
      </label>
      {#if analyzing}
        <span class="progress" title="사진을 보는 동안에는 잠시 멈춥니다">
          얼굴 찾는 중 {progress[0].toLocaleString()} / {progress[1].toLocaleString()}
          <span class="bar"><span style="width:{(progress[0] / Math.max(1, progress[1])) * 100}%"></span></span>
        </span>
      {:else if report}
        <span class="dim small">
          {report.analyzed >= report.total
            ? `사진 ${report.total.toLocaleString()}장 분석 완료`
            : `분석 ${report.analyzed.toLocaleString()} / ${report.total.toLocaleString()}장`}
        </span>
      {/if}
    {/if}
    <span class="spacer"></span>
    {#if installed}<button class="btn subtle" onclick={clearAll}>얼굴 데이터 지우기</button>{/if}
    <button class="icon-btn" title="닫기 (Esc)" onclick={close}><Icon name="close" /></button>
  </div>

  {#if installed === false}
    <div class="install">
      <h2>얼굴별 분류 기능 설치</h2>
      <ul>
        <li>사진 속 얼굴을 찾아 같은 사람끼리 자동으로 묶습니다. 이름을 붙이면 다음부터 그 사람을 알아봅니다.</li>
        <li>인물별로 모아 보거나, 인물별 폴더로 복사·이동할 수 있습니다.</li>
        <li>
          얼굴 인식 모델 2개({fmtSize(download)})를 OpenCV Zoo(GitHub)에서 내려받습니다.
          <span class="dim">YuNet(얼굴 찾기, MIT) · SFace(얼굴 인식, Apache-2.0)</span>
        </li>
        <li>사진과 얼굴 데이터는 인터넷으로 나가지 않고 이 컴퓨터에만 저장됩니다. 언제든 모두 지울 수 있습니다.</li>
        <li>분석은 백그라운드에서 하고, 사진을 보는 동안에는 멈춰서 보기 속도에 영향이 없습니다.</li>
      </ul>
      {#if installing}
        <div class="progress big">
          내려받는 중 {fmtSize(installing[0])} / {fmtSize(installing[1])}
          <span class="bar"><span style="width:{(installing[0] / Math.max(1, installing[1])) * 100}%"></span></span>
        </div>
      {:else}
        <button class="btn primary" onclick={install}>설치 ({fmtSize(download)})</button>
      {/if}
    </div>
  {:else if installed}
    {#if report && (remaining > 0 || analyzing)}
      <div class="banner">
        {#if analyzing}
          <span>얼굴을 분석하고 있습니다 · {progress[0].toLocaleString()} / {progress[1].toLocaleString()}장. 사진 보기는 그대로 빠르고, 화면을 닫아도 계속합니다.</span>
          <span class="spacer"></span>
          <button class="btn" onclick={stop}>분석 멈춤</button>
        {:else}
          <span>
            아직 분석하지 않은 사진 <b>{remaining.toLocaleString()}장</b> ({estimate} 걸립니다).
            분석하는 동안 사진 보기는 그대로 빠르며, 뷰어를 여는 동안에는 잠시 멈춥니다.
          </span>
          <span class="spacer"></span>
          <button class="btn primary" onclick={start}>분석 시작</button>
        {/if}
      </div>
    {/if}
    <CloudBar {scope} faces banner refresh={report?.analyzed ?? 0} bind:count={cloudCount} />
    {#if report && store.settings.face_profile !== "all" && report.no_turn > 0 && !analyzing}
      <div class="banner">
        <span>
          예전에 분석한 얼굴 <b>{report.no_turn.toLocaleString()}개</b>는 얼굴 방향 정보가 없어 옆모습을 가리지 못했습니다.
          다시 분석하면 가릴 수 있습니다 (붙인 이름은 그대로 남습니다).
        </span>
        <span class="spacer"></span>
        <button class="btn primary" onclick={redoTurn}>방향 다시 분석</button>
      </div>
    {/if}
    <div class="main">
      <aside class="list" bind:clientHeight={listH} onscroll={(e) => (listY = e.currentTarget.scrollTop)}>
        {#if report && !groups.length}
          <p class="empty">{analyzing ? "얼굴을 찾는 대로 나타납니다…" : "묶을 얼굴이 없습니다."}</p>
        {/if}
        <div class="persons" style="height:{groups.length * ITEM_H}px">
          {#each shownGroups as { g, i } (keyOf(g))}
            <button
              class="person"
              class:unsorted={g.unsorted}
              style="top:{i * ITEM_H}px"
              class:on={keyOf(g) === selKey}
              onclick={() => (selKey = keyOf(g))}
            >
              <img src={faceUrl(g.cover)} alt="" />
              <span class="info">
                <span class="name" class:unnamed={!g.name}>{title(g)}</span>
                <span class="count">
                  {g.unsorted ? `얼굴 ${g.faces.length}개 · 골라서 이름 붙이기` : `사진 ${g.photos}장${g.auto ? ` · 확인 필요 ${g.auto}` : ""}`}
                </span>
              </span>
            </button>
          {/each}
        </div>
        {#if report?.small}
          <p class="note">너무 작은 얼굴 {report.small}개는 묶지 않았습니다.</p>
        {/if}
        {#if report?.fast}
          <p class="note">얼굴이 아주 많아 빠른 방식으로 묶었습니다 (메모리를 아끼려고 큰 묶음끼리만 합침). 폴더를 나누어 보면 더 정확합니다.</p>
        {/if}
      </aside>

      <section class="detail">
        {#if sel}
          <div class="head">
            <input
              class="name-input"
              type="text"
              placeholder="이름 (예: 엄마, 민준)"
              bind:value={nameInput}
              onkeydown={(e) => e.key === "Enter" && !e.isComposing && saveName()}
            />
            <button
              class="btn primary"
              disabled={!nameInput.trim() || nameInput.trim() === sel.name || working || (sel.unsorted && !picked.size)}
              onclick={saveName}
            >
              {sel.person != null ? "이름 바꾸기" : picked.size ? `고른 ${picked.size}개에 이름 붙이기` : "이름 붙이기"}
            </button>
            {#if named.some((n) => n.person !== sel!.person)}
              <select
                disabled={working || (sel.unsorted && !picked.size)}
                onchange={(e) => {
                  const v = Number(e.currentTarget.value);
                  e.currentTarget.value = "";
                  if (v) mergeInto(v);
                }}
              >
                <option value="">{picked.size ? `고른 ${picked.size}개를 다른 인물로…` : "다른 인물과 합치기…"}</option>
                {#each named.filter((n) => n.person !== sel!.person) as n (n.person)}
                  <option value={n.person}>{n.name}</option>
                {/each}
              </select>
            {/if}
            <span class="spacer"></span>
            {#if sel.person != null}
              {#if sel.auto}<button class="btn" disabled={working} onclick={confirmAuto}>자동 {sel.auto}개 모두 맞음</button>{/if}
              <button class="btn danger" disabled={!picked.size || working} onclick={rejectPicked}>
                고른 {picked.size}개 "이 사람 아님"
              </button>
              <button class="btn subtle" disabled={working} onclick={deletePerson}>이름 지우기</button>
            {:else if !sel.unsorted}
              <button
                class="btn danger"
                disabled={!picked.size || picked.size >= sel.faces.length || working}
                onclick={splitPicked}
                title="고른 얼굴을 이 묶음에서 빼고, 다시 같은 사람으로 묶지 않습니다"
              >
                고른 {picked.size}개 빼기
              </button>
            {/if}
          </div>
          <p class="hint">
            얼굴을 클릭해 고르고, 두 번 클릭하면 사진을 크게 봅니다.
            {#if sel.person != null}노란 테두리는 자동으로 넣은 얼굴입니다 (맞으면 "모두 맞음", 틀리면 골라서 "이 사람 아님").
            {:else if sel.unsorted}같은 사람을 찾지 못했거나{report?.turned ? `, 옆모습이라 뺀(${report.turned}개)` : ""} 얼굴입니다. 같은 사람 얼굴을 골라 이름을 붙이거나 다른 인물로 보내세요.
            {:else}같은 사람으로 묶었지만 아직 이름이 없습니다. 다른 사람 얼굴이 섞여 있으면 골라서 "빼기"를 누르세요. 맞는 얼굴만 골라 이름을 붙여도 됩니다.{/if}
          </p>
          <div
            class="scroller"
            bind:this={faceScroll}
            bind:clientHeight={faceH}
            bind:clientWidth={faceW}
            onscroll={(e) => (faceY = e.currentTarget.scrollTop)}
          >
            <div class="faces" style="height:{Math.ceil(sel.faces.length / cols) * ROW_H}px">
              {#each shownFaces as { f, i } (f.id)}
                <button
                  class="face"
                  style="left:{(i % cols) * (FACE_W + GAP)}px; top:{Math.floor(i / cols) * ROW_H}px"
                  class:picked={picked.has(f.id)}
                  class:auto={sel.person != null && f.person == null}
                  title={baseName(f.path)}
                  onclick={() => togglePick(f.id)}
                  ondblclick={() => openPhoto(f.path)}
                >
                  <img src={faceUrl(f.id)} alt="" />
                  <span class="fname">{baseName(f.path)}</span>
                </button>
              {/each}
            </div>
          </div>
        {/if}
      </section>
    </div>
    <div class="actions">
      <span class="dim small">인물별 폴더로 정리 (이름 붙은 인물만, {isMac ? "⌘Z" : "Ctrl+Z"}로 되돌림)</span>
      <span class="spacer"></span>
      <button class="btn" disabled={working || !named.length} onclick={() => organize("copy")}>인물별 폴더로 복사</button>
      <button class="btn" disabled={working || !named.length} onclick={() => organize("main")}>주인공 기준 이동</button>
    </div>
  {/if}
</div>

<style>
  .people {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .topbar,
  .actions {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 10px 0 16px;
    white-space: nowrap;
    background: var(--panel);
  }
  .topbar {
    height: 44px;
    border-bottom: 1px solid var(--border);
  }
  .actions {
    height: 48px;
    border-top: 1px solid var(--border);
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
  .opt {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .opt select {
    padding: 2px 4px;
    font-size: 12px;
  }
  .person.unsorted .name {
    color: #f0c674;
    font-weight: 600;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .progress {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .progress.big {
    margin-top: 16px;
    font-size: 13px;
  }
  .bar {
    width: 140px;
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
  .btn:disabled {
    opacity: 0.4;
  }
  .btn.subtle {
    background: none;
    color: var(--text-dim);
  }
  .btn.danger {
    background: #3a2224;
    border-color: #5a2c30;
    color: #ffb4b4;
  }
  .install {
    max-width: 560px;
    margin: 60px auto;
    padding: 24px 28px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--panel);
  }
  .install h2 {
    margin: 0 0 12px;
    font-size: 18px;
  }
  .install ul {
    margin: 0 0 18px;
    padding-left: 18px;
    line-height: 1.7;
    font-size: 13px;
  }
  .main {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .list {
    width: 250px;
    flex: none;
    overflow-y: auto;
    padding: 8px;
    border-right: 1px solid var(--border);
    background: var(--panel);
  }
  .persons {
    position: relative;
  }
  .person {
    position: absolute;
    left: 0;
    right: 0;
    height: 56px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: 8px;
    text-align: left;
  }
  .person:hover {
    background: var(--hover);
  }
  .person.on {
    background: var(--accent-soft);
  }
  .person img {
    width: 44px;
    height: 44px;
    flex: none;
    border-radius: 50%;
    object-fit: cover;
    background: #111214;
  }
  .info {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name.unnamed {
    font-weight: 400;
    color: var(--text-dim);
  }
  .count {
    font-size: 11px;
    color: var(--text-faint);
  }
  .empty,
  .note,
  .hint {
    font-size: 12px;
    color: var(--text-faint);
    padding: 0 8px;
    line-height: 1.5;
  }
  .detail {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    padding: 12px 0 0 16px;
  }
  .detail > .head,
  .detail > .hint {
    padding-right: 16px;
  }
  .scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding-bottom: 12px;
  }
  .head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }
  .name-input {
    width: 220px;
    padding: 5px 8px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--panel-2);
    font-size: 14px;
  }
  .hint {
    padding: 0;
    margin: 8px 0 12px;
  }
  .faces {
    position: relative;
  }
  .face {
    position: absolute;
    width: 120px;
    height: 142px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px;
    border-radius: 8px;
    border: 2px solid transparent;
  }
  .face:hover {
    background: var(--hover);
  }
  .face.auto {
    border-color: #8a7330;
  }
  .face.picked {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .face img {
    width: 108px;
    height: 108px;
    border-radius: 6px;
    object-fit: cover;
    background: #111214;
  }
  .fname {
    font-size: 11px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .banner {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--accent-soft);
    font-size: 13px;
  }
</style>
