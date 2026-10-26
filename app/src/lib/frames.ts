// 뷰어와 비교 화면이 함께 쓰는 "화면에 올릴 준비가 끝난 사진" 캐시.
// 엔진(isee://view)이 화면 상자에 맞게 디코딩한 RGBA를 받아 ImageBitmap으로 만들어 둔다.
import { viewUrl } from "./api";

export interface Frame {
  bmp?: ImageBitmap;
  blobUrl?: string;
  /** 받은 픽셀 크기 */
  w: number;
  h: number;
  /** 회전 적용 후 원본 해상도 */
  fw: number;
  fh: number;
  prepMs: number;
  fetchMs: number;
  bitmapMs: number;
  route: string;
}

// 화면 크기 RGBA 한 장 ≈ 20MB. 비교 화면 9칸 + 뷰어 앞뒤 미리 불러오기를 담을 만큼.
const KEEP = 14;
const cache = new Map<string, Frame>();
const inflight = new Map<string, Promise<Frame | null>>();
/** 지금 화면에 그려져 있는 것 (버리지 않음) */
const pinned = new Set<Frame>();

/** 수정 시각(mtime)도 키에 넣는다: 회전·변환으로 같은 이름의 내용이 바뀌면 다시 받는다 */
export function frameKey(path: string, mtime: number, [bw, bh]: [number, number]) {
  return `${path}|${mtime}|${bw}x${bh}`;
}

export function hasFrame(path: string, mtime: number, box: [number, number]) {
  return cache.has(frameKey(path, mtime, box));
}

/** 요청할 상자: 실제 픽셀 수를 128 단위로 올림 (이웃 사진과 캐시 공유).
 *  엔진이 사진 비율에 맞춰 필요한 만큼만 디코딩한다 (세로 사진은 높이 기준). */
export function boxFor(w: number, h: number, dpr: number): [number, number] {
  const up = (v: number) => Math.max(128, Math.ceil((v * dpr) / 128) * 128);
  return [up(w), up(h)];
}

export function pin(f: Frame | null | undefined) {
  if (f) pinned.add(f);
}

export function unpin(f: Frame | null | undefined) {
  if (f) pinned.delete(f);
}

function release(f: Frame) {
  f.bmp?.close();
  if (f.blobUrl) URL.revokeObjectURL(f.blobUrl);
}

/** 오래된 것부터 버리되, 화면에 그려진 것과 방금 받은 것은 남긴다. */
function remember(key: string, f: Frame) {
  cache.set(key, f);
  for (const [k, old] of cache) {
    if (cache.size <= KEEP) break;
    if (pinned.has(old) || old === f) continue;
    cache.delete(k);
    release(old);
  }
}

/** 화면을 닫을 때 메모리를 돌려준다 (그려진 것과 최근 keep장만 남김) */
export function trimFrames(keep = 4) {
  const extra = cache.size - keep;
  let n = 0;
  for (const [k, f] of cache) {
    if (n >= extra) break;
    if (pinned.has(f)) continue;
    cache.delete(k);
    release(f);
    n++;
  }
}

export function loadFrame(path: string, mtime: number, box: [number, number]): Promise<Frame | null> {
  const key = frameKey(path, mtime, box);
  const hit = cache.get(key);
  if (hit) {
    cache.delete(key);
    cache.set(key, hit);
    return Promise.resolve(hit);
  }
  const running = inflight.get(key);
  if (running) return running;
  const p = (async (): Promise<Frame | null> => {
    const t0 = performance.now();
    const r = await fetch(viewUrl(path, box[0], box[1]));
    if (!r.ok) return null;
    const num = (k: string) => Number(r.headers.get(k) ?? 0);
    const route = r.headers.get("x-route") ?? "";
    const base = {
      w: num("x-w"),
      h: num("x-h"),
      fw: num("x-fw") || num("x-w"),
      fh: num("x-fh") || num("x-h"),
      prepMs: num("x-ms"),
      route,
    };
    let f: Frame;
    if (route === "pixels") {
      const buf = await r.arrayBuffer();
      const t1 = performance.now();
      const bmp = await createImageBitmap(new ImageData(new Uint8ClampedArray(buf), base.w, base.h));
      f = { ...base, bmp, fetchMs: t1 - t0, bitmapMs: performance.now() - t1 };
    } else {
      const blob = await r.blob();
      f = { ...base, blobUrl: URL.createObjectURL(blob), fetchMs: performance.now() - t0, bitmapMs: 0 };
    }
    remember(key, f);
    return f;
  })().catch(() => null);
  inflight.set(key, p);
  p.finally(() => inflight.delete(key));
  return p;
}

/** canvas에 그린다 */
export function drawFrame(canvas: HTMLCanvasElement | undefined, f: Frame) {
  if (!f.bmp || !canvas) return;
  canvas.width = f.w;
  canvas.height = f.h;
  canvas.getContext("2d")!.drawImage(f.bmp, 0, 0);
}
