<script lang="ts">
  import { store } from "../lib/store.svelte";
  import { thumbUrl } from "../lib/api";
  import { boxFor, drawFrame, loadFrame, type Frame } from "../lib/frames";

  let { paths }: { paths: string[] } = $props();

  let text = $state(`© ${new Date().getFullYear()}`);
  let anchor = $state(8); // 오른쪽 아래
  let size = $state(25); // 사진 가로의 %
  let opacity = $state(70);
  let color = $state<"white" | "black">("white");
  let frameW = $state(0);
  let input = $state<HTMLInputElement>();

  // 미리보기는 첫 번째 사진의 썸네일 위에 글자를 얹어서 보여준다
  // svelte-ignore state_referenced_locally
  const firstIndex = store.entries.findIndex((e) => e.path === paths[0]);
  const first = store.entries[firstIndex];

  // 미리보기 칸 크기에 맞춰 디코딩한 사진 (썸네일은 작아서 흐리게 보인다)
  let pw = $state(0);
  let ph = $state(0);
  let frame = $state.raw<Frame | null>(null);
  let canvas = $state<HTMLCanvasElement>();
  $effect(() => {
    if (!first || !pw || !ph) return;
    loadFrame(first.path, first.mtime, boxFor(pw, ph, window.devicePixelRatio || 1)).then((f) => (frame = f));
  });
  $effect(() => {
    if (frame && canvas) drawFrame(canvas, frame);
  });
  /** 미리보기 칸 안에 사진 비율대로 꽉 차는 크기 */
  let fit = $derived.by(() => {
    const aw = frame?.fw ?? 4, ah = frame?.fh ?? 3;
    const k = Math.min((pw - 16) / aw, (ph - 16) / ah);
    return k > 0 ? { w: Math.round(aw * k), h: Math.round(ah * k) } : { w: 0, h: 0 };
  });

  $effect(() => {
    input?.focus();
    input?.select();
  });

  const FONT = '-apple-system, BlinkMacSystemFont, "Apple SD Gothic Neo", "Malgun Gothic", sans-serif';

  // 미리보기 글자 크기: 글자 폭이 사진 폭의 size%가 되도록 대략 맞춘다
  let previewFont = $derived.by(() => {
    if (!frameW || !text) return 12;
    const c = document.createElement("canvas").getContext("2d")!;
    c.font = `600 100px ${FONT}`;
    const w = c.measureText(text).width || 1;
    return (frameW * (size / 100) * 100) / w;
  });

  let pos = $derived.by(() => {
    const col = anchor % 3;
    const row = Math.floor(anchor / 3);
    const h = col === 0 ? "left:3%" : col === 1 ? "left:50%" : "right:3%";
    const v = row === 0 ? "top:3%" : row === 1 ? "top:50%" : "bottom:3%";
    const tx = col === 1 ? "-50%" : "0";
    const ty = row === 1 ? "-50%" : "0";
    return `${h};${v};transform:translate(${tx},${ty})`;
  });

  /** 글자를 투명 PNG로 그린다. 엔진이 사진마다 크기를 맞춰 합성한다. */
  async function renderPng(): Promise<number[]> {
    const px = 160;
    const c = document.createElement("canvas");
    const ctx = c.getContext("2d")!;
    ctx.font = `600 ${px}px ${FONT}`;
    c.width = Math.ceil(ctx.measureText(text).width) + 48;
    c.height = Math.ceil(px * 1.4);
    ctx.font = `600 ${px}px ${FONT}`; // 캔버스 크기를 바꾸면 글꼴 설정이 초기화된다
    ctx.textBaseline = "middle";
    ctx.shadowColor = color === "white" ? "rgba(0,0,0,0.55)" : "rgba(255,255,255,0.55)";
    ctx.shadowBlur = 14;
    ctx.fillStyle = color === "white" ? "#ffffff" : "#000000";
    ctx.fillText(text, 24, c.height / 2);
    // toBlob은 웹뷰에서 응답이 오지 않는 경우가 있어 동기식 toDataURL을 쓴다
    const bin = atob(c.toDataURL("image/png").split(",")[1]);
    return Array.from(bin, (ch) => ch.charCodeAt(0));
  }

  async function apply() {
    if (!text.trim()) return;
    // 대화상자를 닫으면 props도 사라지므로 대상 목록을 먼저 잡아 둔다
    const targets = paths;
    let png: number[];
    try {
      png = await renderPng();
    } catch (e) {
      store.showToast(`워터마크 글자를 그리지 못했습니다: ${e}`, { error: true });
      return;
    }
    store.runConvert(
      {
        watermark: {
          png,
          anchor,
          width_ratio: size / 100,
          margin_ratio: 0.03,
          opacity: opacity / 100,
        },
      },
      "워터마크",
      targets,
    );
    store.dialog = null;
  }
</script>

<h3>워터마크 넣기</h3>
<p class="desc">사진 {paths.length}장에 같은 글자를 넣습니다. 사진 크기가 달라도 비율로 맞춥니다.</p>

<div class="layout">
  <div class="preview" bind:clientWidth={pw} bind:clientHeight={ph}>
    {#if first}
      <div class="frame" bind:clientWidth={frameW} style="width:{fit.w}px;height:{fit.h}px">
        {#if frame?.bmp}
          <canvas bind:this={canvas}></canvas>
        {:else}
          <img src={thumbUrl(first, firstIndex)} alt="" draggable="false" />
        {/if}
        <span
          class="wm"
          style="{pos};font-size:{previewFont}px;opacity:{opacity / 100};color:{color};text-shadow:0 0 6px {color ===
          'white'
            ? 'rgba(0,0,0,.6)'
            : 'rgba(255,255,255,.6)'}">{text}</span
        >
      </div>
    {/if}
  </div>
  <div class="controls">
    <label>글자 <input type="text" bind:this={input} bind:value={text} spellcheck="false" /></label>
    <div class="label">위치</div>
    <div class="grid9">
      {#each Array(9) as _, i (i)}
        <button class:on={anchor === i} onclick={() => (anchor = i)} aria-label="위치 {i + 1}"></button>
      {/each}
    </div>
    <label>크기 {size}% <input type="range" min="5" max="80" bind:value={size} /></label>
    <label>진하기 {opacity}% <input type="range" min="10" max="100" bind:value={opacity} /></label>
    <div class="label">색</div>
    <div class="colors">
      <button class:on={color === "white"} onclick={() => (color = "white")}>흰색</button>
      <button class:on={color === "black"} onclick={() => (color = "black")}>검정</button>
    </div>
  </div>
</div>

<div class="actions">
  <button class="btn" onclick={() => (store.dialog = null)}>취소</button>
  <button class="btn primary" onclick={apply} disabled={!text.trim()}>넣기</button>
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: 1fr 190px;
    gap: 16px;
  }
  .preview {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 420px;
    background: #141518;
    border-radius: 6px;
  }
  .frame {
    position: relative;
    display: inline-block;
    line-height: 0;
  }
  .frame img,
  .frame canvas {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .wm {
    position: absolute;
    white-space: nowrap;
    font-weight: 600;
    line-height: 1.2;
    font-family: -apple-system, BlinkMacSystemFont, "Apple SD Gothic Neo", "Malgun Gothic", sans-serif;
    pointer-events: none;
  }
  .controls {
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: 12px;
    color: var(--text-dim);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  label input[type="range"] {
    accent-color: var(--accent);
  }
  .grid9 {
    display: grid;
    grid-template-columns: repeat(3, 28px);
    gap: 4px;
  }
  .grid9 button {
    height: 20px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--panel);
  }
  .grid9 button.on {
    background: var(--accent);
    border-color: var(--accent);
  }
  .colors {
    display: flex;
    gap: 6px;
  }
  .colors button {
    padding: 3px 10px;
    border-radius: 4px;
    border: 1px solid var(--border);
  }
  .colors button.on {
    border-color: var(--accent);
    color: var(--text);
  }
</style>
