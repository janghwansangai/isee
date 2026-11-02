// 사진 인쇄: 쪽 나누기(배치)와 그리기. 미리보기와 실제 인쇄가 같은 함수로 그려서 보이는 대로 나온다.
// 단위는 mm. 인쇄할 때는 쪽마다 용지 크기 그대로(300dpi) 그린 이미지를 문서에 넣고 운영체제 인쇄 창을 연다.
import { api, viewUrl } from "./api";

export type PrintMode = "single" | "multi" | "repeat" | "poster";
export type Fit = "fit" | "fill";
export type Orient = "auto" | "portrait" | "landscape";

export interface PrintOptions {
  mode: PrintMode;
  paper: keyof typeof PAPERS;
  orient: Orient;
  /** 잘리지 않게(용지에 맞춤) · 꽉 채우기(가장자리가 잘림) */
  fit: Fit;
  /** 용지 가장자리 여백 */
  margin: number;
  /** 사진 사이 간격 (여러 장을 한 쪽에) */
  gap: number;
  /** 사진을 90° 돌려 더 크게 (용지 방향과 사진 방향이 다를 때) */
  rotate: boolean;
  /** 한 사진을 여러 개로: 한 쪽에 몇 개 */
  copies: 2 | 4 | 8 | 16;
  /** 크게 나눠 인쇄: 몇 장으로 */
  tiles: 2 | 4 | 9 | 16;
  /** 파일 이름·인쇄 날짜 표시 (끄면 사진만) */
  captions: boolean;
}

export const PAPERS = {
  a4: { label: "A4 (210×297mm)", w: 210, h: 297 },
  a3: { label: "A3 (297×420mm)", w: 297, h: 420 },
  a5: { label: "A5 (148×210mm)", w: 148, h: 210 },
  b5: { label: "B5 (182×257mm)", w: 182, h: 257 },
  letter: { label: "레터 (216×279mm)", w: 215.9, h: 279.4 },
  p4x6: { label: "사진 4×6 (102×152mm)", w: 101.6, h: 152.4 },
  p5x7: { label: "사진 5×7 (127×178mm)", w: 127, h: 177.8 },
} as const;

export const DEFAULT_OPTIONS: PrintOptions = {
  mode: "single",
  paper: "a4",
  orient: "auto",
  fit: "fit",
  margin: 5,
  gap: 3,
  rotate: true,
  copies: 4,
  tiles: 4,
  captions: false,
};

const KEY = "isee.print";
export function loadOptions(): PrintOptions {
  try {
    const o = { ...DEFAULT_OPTIONS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
    if (!(o.paper in PAPERS)) o.paper = DEFAULT_OPTIONS.paper;
    return o;
  } catch {
    return { ...DEFAULT_OPTIONS };
  }
}
export function saveOptions(o: PrintOptions) {
  try {
    localStorage.setItem(KEY, JSON.stringify(o));
  } catch {
    // 저장하지 못해도 인쇄는 된다
  }
}

/** 인쇄할 사진 (w·h는 돌린 방향 그대로의 비율) */
export interface PrintPhoto {
  path: string;
  name: string;
  w: number;
  h: number;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 쪽 위의 사진 한 칸 */
export interface Cell {
  photo: number;
  /** 사진이 보이는 칸 (이 밖은 잘린다) */
  box: Rect;
  /** 90° 돌려 그림 */
  rot: boolean;
  /** 꽉 채우기 (칸을 덮고 넘치는 부분은 잘림) */
  fill: boolean;
  /** 크게 나눠 인쇄: 사진 전체가 놓이는 자리 (이 쪽 좌표). 칸에는 그중 일부만 보인다 */
  whole?: Rect;
  /** 칸 아래 글자 (파일 이름) */
  label?: string;
}

export interface Page {
  w: number;
  h: number;
  cells: Cell[];
  /** 쪽 아래 글자와 그 줄의 가운데 높이 */
  footer?: string;
  footY?: number;
  /** 크게 나눠 인쇄: 조각 위치 (몇 행 몇 열) */
  tile?: string;
}

/** 칸 아래 파일 이름 줄 높이, 쪽 아래 글자 줄 높이 (mm) */
const LABEL_H = 4.5;
const FOOT_H = 6;

/** 오늘 날짜 "2026.11.02" */
export function today(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}.${p(d.getMonth() + 1)}.${p(d.getDate())}`;
}

const aspect = (p: PrintPhoto) => (p.w > 0 && p.h > 0 ? p.w / p.h : 1);

/** 상자 안에 비율 a(가로/세로)인 사진을 넣었을 때의 크기 (contain) */
function contain(a: number, w: number, h: number): [number, number] {
  return w / h > a ? [h * a, h] : [w, w / a];
}

function inset(w: number, h: number, o: PrintOptions, captions: boolean): Rect {
  const m = Math.max(0, o.margin);
  return { x: m, y: m, w: Math.max(1, w - 2 * m), h: Math.max(1, h - 2 * m - (captions ? FOOT_H : 0)) };
}

/** 쪽 아래 글자 줄의 가운데 (여백 안쪽 맨 아래 줄) */
function footY(h: number, o: PrintOptions): number {
  return h - Math.max(0, o.margin) - FOOT_H / 2;
}

function paperSize(o: PrintOptions, landscape: boolean): [number, number] {
  const p = PAPERS[o.paper];
  const [a, b] = [Math.min(p.w, p.h), Math.max(p.w, p.h)];
  return landscape ? [b, a] : [a, b];
}

/** 방향 후보: 자동이면 세로·가로 둘 다 해 보고 사진이 더 크게 나오는 쪽 */
function orients(o: PrintOptions): boolean[] {
  return o.orient === "auto" ? [false, true] : [o.orient === "landscape"];
}

/** 사진을 칸에 넣을 때 돌릴지 (돌리면 더 크게 보일 때만) */
function shouldRotate(a: number, box: Rect, o: PrintOptions): boolean {
  if (!o.rotate || Math.abs(a - 1) < 0.02) return false;
  const [w0, h0] = contain(a, box.w, box.h);
  const [w1, h1] = contain(1 / a, box.w, box.h);
  return w1 * h1 > w0 * h0 * 1.0001;
}

/** 쪽 배치. 같은 입력이면 미리보기와 인쇄가 같다. */
export function layout(photos: PrintPhoto[], o: PrintOptions, date = today()): Page[] {
  if (!photos.length) return [];
  switch (o.mode) {
    case "single":
      return layoutSingle(photos, o, date);
    case "multi":
      return [layoutMulti(photos, o, date)];
    case "repeat":
      return photos.map((p, i) => layoutRepeat(p, i, o, date));
    case "poster":
      return photos.flatMap((p, i) => layoutPoster(p, i, o, date));
  }
}

/** 한 장에 한 사진. 방향을 자동으로 하면 사진 대부분의 방향을 따른다 (한 번 인쇄에 용지 방향은 하나) */
function layoutSingle(photos: PrintPhoto[], o: PrintOptions, date: string): Page[] {
  const wide = photos.filter((p) => aspect(p) > 1).length;
  const landscape = o.orient === "auto" ? wide > photos.length / 2 : o.orient === "landscape";
  const [W, H] = paperSize(o, landscape);
  const box = inset(W, H, o, o.captions);
  return photos.map((p, i) => ({
    w: W,
    h: H,
    cells: [{ photo: i, box, rot: shouldRotate(aspect(p), box, o), fill: o.fit === "fill" }],
    footer: o.captions ? `${p.name}  ·  인쇄 ${date}` : undefined,
    footY: footY(H, o),
  }));
}

/** 여러 사진을 한 쪽에: 사진 비율을 지키며 줄마다 높이를 맞춰 쪽을 가장 넓게 채우는 줄 수를 고른다 */
function layoutMulti(photos: PrintPhoto[], o: PrintOptions, date: string): Page {
  const as = photos.map(aspect);
  const labelH = o.captions ? LABEL_H : 0;
  let best: { page: Page; area: number } | null = null;
  for (const land of orients(o)) {
    const [W, H] = paperSize(o, land);
    const c = inset(W, H, o, o.captions);
    const j = justify(as, c.w, c.h, o.gap, labelH);
    if (!j || (best && j.area <= best.area)) continue;
    const cells: Cell[] = [];
    const used = j.rows.reduce((s, row, k) => s + j.hs[k] * j.s + labelH, 0) + o.gap * (j.rows.length - 1);
    let y = c.y + (c.h - used) / 2;
    j.rows.forEach((row, k) => {
      const h = j.hs[k] * j.s;
      const rowW = row.reduce((s, i) => s + h * as[i], 0) + o.gap * (row.length - 1);
      let x = c.x + (c.w - rowW) / 2;
      for (const i of row) {
        const w = h * as[i];
        cells.push({ photo: i, box: { x, y, w, h }, rot: false, fill: false, label: o.captions ? photos[i].name : undefined });
        x += w + o.gap;
      }
      y += h + labelH + o.gap;
    });
    best = { page: { w: W, h: H, cells, footer: o.captions ? `인쇄 ${date}` : undefined, footY: footY(H, o) }, area: j.area };
  }
  return best!.page;
}

/** 비율 as인 사진들을 줄로 나눠 너비 W에 맞춘다. 높이 H를 넘으면 모두 줄인다. 사진 면적 합이 가장 큰 줄 수를 고른다 */
function justify(as: number[], W: number, H: number, gap: number, labelH: number) {
  let best: { rows: number[][]; hs: number[]; s: number; area: number } | null = null;
  const maxRows = Math.min(as.length, 24);
  for (let r = 1; r <= maxRows; r++) {
    const rows = partition(as, r);
    const hs = rows.map((row) => (W - gap * (row.length - 1)) / row.reduce((s, i) => s + as[i], 0));
    const avail = H - gap * (r - 1) - labelH * r;
    if (avail <= 0 || hs.some((h) => h <= 0)) continue;
    const s = Math.min(1, avail / hs.reduce((a, b) => a + b, 0));
    let area = 0;
    rows.forEach((row, k) => row.forEach((i) => (area += (hs[k] * s) ** 2 * as[i])));
    if (!best || area > best.area) best = { rows, hs, s, area };
  }
  return best;
}

/** 순서를 지키며 n개를 r줄로: 줄마다 비율 합(= 줄 높이의 역수)이 고르게 (가장 큰 줄 합을 가장 작게) */
function partition(as: number[], r: number): number[][] {
  const n = as.length;
  const pre = [0];
  for (const a of as) pre.push(pre[pre.length - 1] + a);
  const sum = (i: number, j: number) => pre[j] - pre[i];
  // cost[k][j]: 처음 j개를 k줄로 나눴을 때 가장 큰 줄 합의 최솟값
  const cost = Array.from({ length: r + 1 }, () => new Array<number>(n + 1).fill(Infinity));
  const cut = Array.from({ length: r + 1 }, () => new Array<number>(n + 1).fill(0));
  cost[0][0] = 0;
  for (let k = 1; k <= r; k++) {
    for (let j = k; j <= n; j++) {
      for (let i = k - 1; i < j; i++) {
        const c = Math.max(cost[k - 1][i], sum(i, j));
        if (c < cost[k][j]) {
          cost[k][j] = c;
          cut[k][j] = i;
        }
      }
    }
  }
  const rows: number[][] = [];
  let j = n;
  for (let k = r; k >= 1; k--) {
    const i = cut[k][j];
    rows.unshift(Array.from({ length: j - i }, (_, t) => i + t));
    j = i;
  }
  return rows;
}

const GRIDS: Record<PrintOptions["copies"], [number, number][]> = {
  2: [
    [1, 2],
    [2, 1],
  ],
  4: [[2, 2]],
  8: [
    [2, 4],
    [4, 2],
  ],
  16: [[4, 4]],
};

/** 한 사진을 여러 개로: 칸 배열(가로×세로)·용지 방향·사진 돌림 중 사진이 가장 크게 나오는 것 */
function layoutRepeat(p: PrintPhoto, index: number, o: PrintOptions, date: string): Page {
  const a = aspect(p);
  let best: { page: Page; area: number } | null = null;
  for (const land of orients(o)) {
    const [W, H] = paperSize(o, land);
    const c = inset(W, H, o, o.captions);
    for (const [cols, rows] of GRIDS[o.copies]) {
      const cw = (c.w - o.gap * (cols - 1)) / cols;
      const ch = (c.h - o.gap * (rows - 1)) / rows;
      if (cw <= 0 || ch <= 0) continue;
      const probe = { x: 0, y: 0, w: cw, h: ch };
      const rot = shouldRotate(a, probe, o);
      const [w, h] = contain(rot ? 1 / a : a, cw, ch);
      // 크기가 같으면 돌리지 않은 쪽 (보기에 자연스럽다)
      const area = (o.fit === "fill" ? cw * ch * Math.min(w / cw, h / ch) : w * h) * (rot ? 0.99 : 1);
      if (best && area <= best.area) continue;
      const cells: Cell[] = [];
      for (let r = 0; r < rows; r++)
        for (let k = 0; k < cols; k++)
          cells.push({
            photo: index,
            box: { x: c.x + k * (cw + o.gap), y: c.y + r * (ch + o.gap), w: cw, h: ch },
            rot,
            fill: o.fit === "fill",
          });
      best = {
        page: { w: W, h: H, cells, footer: o.captions ? `${p.name}  ·  ${o.copies}개  ·  인쇄 ${date}` : undefined, footY: footY(H, o) },
        area,
      };
    }
  }
  return best!.page;
}

const TILE_GRIDS: Record<PrintOptions["tiles"], [number, number][]> = {
  2: [
    [1, 2],
    [2, 1],
  ],
  4: [[2, 2]],
  9: [[3, 3]],
  16: [[4, 4]],
};

/** 크게 나눠 인쇄: 여러 쪽을 이어 붙인 큰 판에 사진 하나를 놓고 쪽마다 그 일부를 인쇄한다.
 *  쪽마다 여백을 잘라 내고 붙이면 그림이 이어진다. */
function layoutPoster(p: PrintPhoto, index: number, o: PrintOptions, date: string): Page[] {
  const a = aspect(p);
  let best: { pages: Page[]; area: number } | null = null;
  for (const land of orients(o)) {
    const [W, H] = paperSize(o, land);
    const c = inset(W, H, o, o.captions);
    for (const [cols, rows] of TILE_GRIDS[o.tiles]) {
      const big = { x: 0, y: 0, w: c.w * cols, h: c.h * rows };
      const rot = shouldRotate(a, big, o);
      const ra = rot ? 1 / a : a;
      let [w, h] = contain(ra, big.w, big.h);
      const area = w * h * (rot ? 0.99 : 1);
      if (o.fit === "fill") [w, h] = big.w / big.h > ra ? [big.w, big.w / ra] : [big.h * ra, big.h];
      if (best && area <= best.area) continue;
      const whole = { x: (big.w - w) / 2, y: (big.h - h) / 2, w, h };
      const pages: Page[] = [];
      for (let r = 0; r < rows; r++)
        for (let k = 0; k < cols; k++) {
          const n = r * cols + k + 1;
          pages.push({
            w: W,
            h: H,
            cells: [
              {
                photo: index,
                box: c,
                rot,
                fill: false,
                whole: { x: c.x + whole.x - k * c.w, y: c.y + whole.y - r * c.h, w: whole.w, h: whole.h },
              },
            ],
            tile: `${r + 1}행 ${k + 1}열`,
            footer: o.captions ? `${p.name}  ·  ${n}/${cols * rows} (${r + 1}행 ${k + 1}열)  ·  인쇄 ${date}` : undefined,
            footY: footY(H, o),
          });
        }
      best = { pages, area };
    }
  }
  return best!.pages;
}

/** 사진 그림을 그릴 자리 (돌리기 전 크기). fill이면 칸을 덮고, 아니면 칸 안에 들어간다 */
function target(cell: Cell, a: number): Rect {
  if (cell.whole) return cell.whole;
  const b = cell.box;
  const ra = cell.rot ? 1 / a : a;
  let [w, h] = contain(ra, b.w, b.h);
  if (cell.fill) [w, h] = b.w / b.h > ra ? [b.w, b.w / ra] : [b.h * ra, b.h];
  return { x: b.x + (b.w - w) / 2, y: b.y + (b.h - h) / 2, w, h };
}

/** 이 칸을 그리려면 사진이 몇 px(돌리기 전 가로·세로) 필요한지 */
export function neededPx(cell: Cell, p: PrintPhoto, pxPerMm: number): [number, number] {
  const t = target(cell, aspect(p));
  const [w, h] = cell.rot ? [t.h, t.w] : [t.w, t.h];
  return [Math.ceil(w * pxPerMm), Math.ceil(h * pxPerMm)];
}

/** 쪽 하나를 그린다 (흰 바탕). img(i)가 없으면 회색 상자 */
export function drawPage(
  ctx: CanvasRenderingContext2D,
  page: Page,
  photos: PrintPhoto[],
  pxPerMm: number,
  img: (i: number) => CanvasImageSource | null | undefined,
) {
  const k = pxPerMm;
  ctx.save();
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, page.w * k, page.h * k);
  for (const cell of page.cells) {
    const p = photos[cell.photo];
    const b = cell.box;
    const src = img(cell.photo);
    ctx.save();
    ctx.beginPath();
    ctx.rect(b.x * k, b.y * k, b.w * k, b.h * k);
    ctx.clip();
    const t = target(cell, aspect(p));
    if (!src) {
      ctx.fillStyle = "#e4e4e8";
      ctx.fillRect(t.x * k, t.y * k, t.w * k, t.h * k);
    } else if (cell.rot) {
      ctx.translate((t.x + t.w / 2) * k, (t.y + t.h / 2) * k);
      ctx.rotate(Math.PI / 2);
      ctx.drawImage(src, (-t.h / 2) * k, (-t.w / 2) * k, t.h * k, t.w * k);
    } else {
      ctx.drawImage(src, t.x * k, t.y * k, t.w * k, t.h * k);
    }
    ctx.restore();
    if (cell.label) text(ctx, cell.label, (b.x + b.w / 2) * k, (b.y + b.h + 3.2) * k, b.w * k, 2.6 * k);
  }
  if (page.footer && page.footY !== undefined) text(ctx, page.footer, (page.w / 2) * k, page.footY * k, (page.w - 10) * k, 3 * k);
  ctx.restore();
}

function text(ctx: CanvasRenderingContext2D, s: string, cx: number, baseline: number, maxW: number, size: number) {
  ctx.save();
  ctx.fillStyle = "#444";
  ctx.font = `${size}px -apple-system, "Apple SD Gothic Neo", "Malgun Gothic", "Segoe UI", sans-serif`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  let t = s;
  if (ctx.measureText(t).width > maxW) {
    while (t.length > 1 && ctx.measureText(t + "…").width > maxW) t = t.slice(0, -1);
    t += "…";
  }
  ctx.fillText(t, cx, baseline);
  ctx.restore();
}

/** 인쇄용 원본 크기 사진 (필요한 만큼만 디코딩) */
export async function loadBitmap(path: string, w: number, h: number): Promise<ImageBitmap> {
  const r = await fetch(viewUrl(path, Math.max(1, w), Math.max(1, h)));
  if (!r.ok) throw new Error("사진을 읽을 수 없습니다");
  if (r.headers.get("x-route") === "pixels") {
    const pw = Number(r.headers.get("x-w")), ph = Number(r.headers.get("x-h"));
    return createImageBitmap(new ImageData(new Uint8ClampedArray(await r.arrayBuffer()), pw, ph));
  }
  return createImageBitmap(await r.blob(), { imageOrientation: "from-image" });
}

/** 인쇄 해상도 (쪽 긴 변이 너무 크면 낮춘다: 캔버스 한도) */
export function printDpi(page: Page): number {
  const maxPx = 5200;
  return Math.min(300, (maxPx / Math.max(page.w, page.h)) * 25.4);
}

/** 쪽들을 인쇄 해상도로 그려 문서에 넣고 인쇄 창을 연다. progress(끝난 쪽, 전체) */
export async function printPages(pages: Page[], photos: PrintPhoto[], progress: (done: number, total: number) => void) {
  if (!pages.length) return;
  const { w, h } = pages[0];
  const urls: string[] = [];
  const cache = new Map<number, ImageBitmap>();
  // 사진마다 가장 크게 필요한 크기 (같은 사진은 한 번만 읽는다)
  const dpi = printDpi(pages[0]);
  const k = dpi / 25.4;
  const need = new Map<number, [number, number]>();
  for (const pg of pages)
    for (const c of pg.cells) {
      const [nw, nh] = neededPx(c, photos[c.photo], k);
      const o = need.get(c.photo) ?? [0, 0];
      need.set(c.photo, [Math.max(o[0], nw), Math.max(o[1], nh)]);
    }
  const lastUse = new Map<number, number>();
  pages.forEach((pg, i) => pg.cells.forEach((c) => lastUse.set(c.photo, i)));
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(w * k);
  canvas.height = Math.round(h * k);
  const ctx = canvas.getContext("2d")!;
  ctx.imageSmoothingQuality = "high";
  try {
    for (let i = 0; i < pages.length; i++) {
      progress(i, pages.length);
      for (const c of pages[i].cells) {
        if (cache.has(c.photo)) continue;
        const [nw, nh] = need.get(c.photo)!;
        cache.set(c.photo, await loadBitmap(photos[c.photo].path, nw, nh));
      }
      drawPage(ctx, pages[i], photos, k, (n) => cache.get(n));
      const blob = await new Promise<Blob | null>((ok) => canvas.toBlob(ok, "image/jpeg", 0.92));
      if (!blob) throw new Error("인쇄할 쪽을 만들지 못했습니다");
      urls.push(URL.createObjectURL(blob));
      // 뒤 쪽에서 다시 쓰지 않는 사진은 바로 놓는다 (메모리)
      for (const [n, b] of cache)
        if ((lastUse.get(n) ?? -1) <= i) {
          b.close();
          cache.delete(n);
        }
    }
    progress(pages.length, pages.length);
  } catch (e) {
    urls.forEach((u) => URL.revokeObjectURL(u));
    throw e;
  } finally {
    cache.forEach((b) => b.close());
  }
  await showForPrint(urls, w, h);
  await api.printPages(w, h);
}

let lastUrls: string[] = [];

/** 인쇄할 쪽 이미지를 문서에 넣는다 (화면에는 보이지 않고 인쇄에만 나온다).
 *  맥은 인쇄 창이 뜬 뒤에 그리므로 다음 인쇄 때까지 남겨 둔다. */
async function showForPrint(urls: string[], w: number, h: number) {
  document.querySelector(".print-root")?.remove();
  lastUrls.forEach((u) => URL.revokeObjectURL(u));
  lastUrls = urls;
  const root = document.createElement("div");
  root.className = "print-root";
  const style = document.createElement("style");
  style.textContent = `@page { size: ${w}mm ${h}mm; margin: 0; }`;
  root.appendChild(style);
  const imgs = urls.map((u) => {
    const img = new Image();
    img.className = "print-page";
    // 반올림으로 한 쪽이 넘쳐 빈 쪽이 생기지 않게 아주 조금 작게
    img.style.width = `${w}mm`;
    img.style.height = `${h - 0.3}mm`;
    img.src = u;
    root.appendChild(img);
    return img;
  });
  document.body.appendChild(root);
  await Promise.all(imgs.map((i) => i.decode().catch(() => undefined)));
}
