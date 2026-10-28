<script lang="ts">
  import { store } from "../lib/store.svelte";
  import { api, fmtDate, fmtSize, thumbUrl, type Info } from "../lib/api";

  let info = $state.raw<Info | null>(null);
  const memo = new Map<string, Info>();
  let timer = 0;

  let entry = $derived(store.entries[store.focus]);
  let multi = $derived(store.selected.size > 1);
  let totalSize = $derived.by(() => {
    let s = 0;
    for (const i of store.selected) s += store.entries[i]?.size ?? 0;
    return s;
  });

  // 포커스가 빠르게 바뀌는 동안(키를 누르고 있을 때)은 정보를 읽지 않는다
  $effect(() => {
    const e = entry;
    clearTimeout(timer);
    if (!e || e.kind === "dir") {
      info = null;
      return;
    }
    const key = `${e.path}|${e.mtime}`;
    const hit = memo.get(key);
    if (hit) {
      info = hit;
      return;
    }
    // 앞 파일의 정보가 남아 보이지 않게
    info = null;
    timer = window.setTimeout(async () => {
      try {
        const r = await api.fileInfo(e.path);
        memo.set(key, r);
        if (store.entries[store.focus]?.path === e.path) info = r;
      } catch {
        info = null;
      }
    }, 90);
  });
</script>

<aside class="panel" aria-label="정보">
  {#if multi}
    <div class="title">{store.selected.size}개 선택</div>
    <dl>
      <dt>총 크기</dt>
      <dd>{fmtSize(totalSize)}</dd>
    </dl>
  {:else if entry}
    {#if entry.kind === "image" || entry.kind === "video" || entry.kind === "pdf"}
      <div class="preview">
        <img src={thumbUrl(entry, store.focus)} alt="" draggable="false" onerror={(e) => ((e.currentTarget as HTMLElement).style.visibility = "hidden")} onload={(e) => ((e.currentTarget as HTMLElement).style.visibility = "")} />
      </div>
    {/if}
    <div class="title" title={entry.name}>{entry.name}</div>
    <dl>
      {#if entry.kind !== "dir"}
        <dt>크기</dt>
        <dd>{fmtSize(entry.size)}</dd>
      {/if}
      {#if info?.width}
        <dt>해상도</dt>
        <dd>{info.width} × {info.height} ({((info.width * (info.height ?? 0)) / 1e6).toFixed(1)}MP)</dd>
      {/if}
      {#if info?.format}
        <dt>형식</dt>
        <dd>{info.format}</dd>
      {/if}
      {#if info?.taken}
        <dt>촬영</dt>
        <dd>{info.taken.replace(/^(\d{4}):(\d{2}):(\d{2})/, "$1-$2-$3")}</dd>
      {/if}
      <dt>수정</dt>
      <dd>{fmtDate(entry.mtime)}</dd>
      {#if info?.camera}
        <dt>카메라</dt>
        <dd>{info.camera}</dd>
      {/if}
      {#if info?.lens}
        <dt>렌즈</dt>
        <dd>{info.lens}</dd>
      {/if}
      {#if info?.exposure || info?.fnumber || info?.iso}
        <dt>노출</dt>
        <dd>
          {[info.exposure, info.fnumber, info.iso ? `ISO ${info.iso}` : null, info.focal]
            .filter(Boolean)
            .join("  ")}
        </dd>
      {/if}
      {#if entry.cloud || info?.cloud}
        <dt>위치</dt>
        <dd>☁ 클라우드에만 있음 (열면 내려받음)</dd>
      {/if}
      {#if info?.gps}
        <dt>위치</dt>
        <!-- 사진을 넘기는 순간 info가 먼저 비워질 수 있어 ?.로 읽는다 -->
        <dd>{info?.gps?.[0].toFixed(5)}, {info?.gps?.[1].toFixed(5)}</dd>
      {/if}
    </dl>
  {:else}
    <div class="none">선택한 항목 없음</div>
  {/if}
</aside>

<style>
  .panel {
    height: 100%;
    overflow-y: auto;
    padding: 12px;
    background: var(--panel);
    border-left: 1px solid var(--border);
    user-select: text;
    -webkit-user-select: text;
  }
  .preview {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 180px;
    margin-bottom: 10px;
    border-radius: var(--radius);
    background: #1a1b1f;
    overflow: hidden;
  }
  .preview img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .title {
    font-weight: 600;
    word-break: break-all;
    margin-bottom: 10px;
  }
  dl {
    display: grid;
    grid-template-columns: 52px 1fr;
    gap: 6px 8px;
    margin: 0;
    font-size: 12px;
  }
  dt {
    color: var(--text-faint);
  }
  dd {
    margin: 0;
    color: var(--text-dim);
    word-break: break-all;
    font-variant-numeric: tabular-nums;
  }
  .none {
    color: var(--text-faint);
    text-align: center;
    margin-top: 40px;
  }
</style>
