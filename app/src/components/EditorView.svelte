<script lang="ts">
  // 간단 편집기 (6단계): 자르기·회전·반전, 밝기·대비·채도, 모자이크·흐리게, 펜·형광펜·선·화살표·사각형·원·텍스트.
  // 모든 편집은 "개체 목록"으로 남아 저장 전까지 다시 고치거나 되돌릴 수 있다.
  // 화면에는 화면 크기 사진으로 그리고, 저장할 때 같은 그리기 함수로 원본 해상도에 다시 그려 엔진에 넘긴다.
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { store } from "../lib/store.svelte";
  import { api, baseName, fmtSize, isMac, viewUrl, type ConvertSpec, type OutFormat } from "../lib/api";
  import { boxFor, loadFrame, type Frame } from "../lib/frames";
  import Icon from "./Icon.svelte";

  type Tool = "select" | "crop" | "pen" | "marker" | "line" | "arrow" | "rect" | "ellipse" | "text" | "mosaic" | "blur";
  type Rect = { x: number; y: number; w: number; h: number };
  type Op =
    | { k: "pen"; pts: number[]; color: string; lw: number; alpha: number }
    | { k: "line" | "arrow"; x1: number; y1: number; x2: number; y2: number; color: string; lw: number; alpha: number }
    | { k: "rect" | "ellipse"; r: Rect; color: string; lw: number; fill: boolean }
    | { k: "text"; x: number; y: number; text: string; size: number; color: string; outline: boolean; m: number[] }
    | { k: "mosaic" | "blur"; r: Rect; block: number };
  type Adjust = { bright: number; contrast: number; saturate: number };
  /** 기준 사진(회전 전) → 결과 좌표 변환 [a, b, c, d, e, f] (90° 단위 회전·반전만) */
  type Mat = [number, number, number, number, number, number];
  interface Snap {
    ops: Op[];
    crop: Rect | null;
    mat: Mat;
    adj: Adjust;
  }

  const path = store.editor!.path;
  const entry = store.editor!.entry;

  // ───── 상태 ─────
  let frame = $state.raw<Frame | null>(null);
  /** 원본 해상도 (회전 정보 적용 후) */
  let bw = $state(0);
  let bh = $state(0);
  let ops = $state.raw<Op[]>([]);
  let crop = $state.raw<Rect | null>(null);
  let mat = $state.raw<Mat>([1, 0, 0, 1, 0, 0]);
  let adj = $state<Adjust>({ bright: 100, contrast: 100, saturate: 100 });
  let tool = $state<Tool>("pen");
  let color = $state("#ff3b30");
  let weight = $state(2); // 1 가늘게 · 2 보통 · 3 굵게
  let fill = $state(false);
  let outline = $state(true);
  let ratio = $state<number | null>(null); // 자르기 비율 (null = 자유)
  let selected = $state(-1);
  let undoStack: Snap[] = [];
  let redoStack: Snap[] = [];
  let canUndo = $state(false);
  let canRedo = $state(false);
  let dirty = $state(false);
  let saving = $state<string | null>(null);

  // 저장 설정
  let format = $state<OutFormat | "same">("same");
  let resize = $state("none");
  let quality = $state(90);
  let saveOpen = $state(false);

  const COLORS = ["#ff3b30", "#ff9500", "#ffcc00", "#34c759", "#0a84ff", "#af52de", "#ffffff", "#000000"];
  const TOOLS: { t: Tool; label: string; key: string; icon: string }[] = [
    { t: "select", label: "선택·이동", key: "V", icon: "M5 3l13 8-6 1.5L9 19z" },
    { t: "crop", label: "자르기", key: "C", icon: "M6 2v16h16 M2 6h16v16" },
    { t: "pen", label: "펜", key: "P", icon: "M4 20l4-1 11-11-3-3L5 16z M14 6l3 3" },
    { t: "marker", label: "형광펜", key: "H", icon: "M5 19h8 M8 16l9-9 2 2-9 9H8z" },
    { t: "line", label: "선·밑줄", key: "L", icon: "M4 18L20 6" },
    { t: "arrow", label: "화살표", key: "A", icon: "M4 20L19 5 M10 5h9v9" },
    { t: "rect", label: "사각형", key: "R", icon: "M4 6h16v12H4z" },
    { t: "ellipse", label: "원", key: "O", icon: "M12 5a8 7 0 1 0 0 14a8 7 0 1 0 0-14z" },
    { t: "text", label: "텍스트", key: "T", icon: "M5 5h14 M12 5v14 M9 19h6" },
    { t: "mosaic", label: "모자이크", key: "M", icon: "M4 4h5v5H4z M9 9h5v5H9z M14 4h5v5h-5z M4 14h5v5H4z M14 14h5v5h-5z" },
    { t: "blur", label: "흐리게", key: "B", icon: "M12 3c3 4 6 7 6 11a6 6 0 0 1-12 0c0-4 3-7 6-11z" },
  ];
  const RATIOS: { label: string; v: number | null | "orig" }[] = [
    { label: "자유", v: null },
    { label: "원본", v: "orig" },
    { label: "1:1", v: 1 },
    { label: "4:3", v: 4 / 3 },
    { label: "3:4", v: 3 / 4 },
    { label: "16:9", v: 16 / 9 },
    { label: "9:16", v: 9 / 16 },
  ];

  // 결과(회전 후) 크기
  let fw = $derived(mat[0] === 0 ? bh : bw);
  let fh = $derived(mat[0] === 0 ? bw : bh);
  let long = $derived(Math.max(fw, fh) || 1);
  let lw = $derived(Math.max(1, Math.round(long * [0, 0.002, 0.004, 0.008][weight])));
  let textSize = $derived(Math.max(8, Math.round(long * [0, 0.025, 0.04, 0.065][weight])));
  let block = $derived(Math.max(4, Math.round(long * [0, 0.008, 0.015, 0.03][weight])));
  /** 지금 화면에 보이는 영역 (자르기 중에는 전체) */
  let view = $derived<Rect>(tool === "crop" || !crop ? { x: 0, y: 0, w: fw, h: fh } : crop);

  // ───── 불러오기 ─────
  let stage = $state<HTMLDivElement>();
  let sw = $state(0);
  let sh = $state(0);
  let canvas = $state<HTMLCanvasElement>();
  const dpr = window.devicePixelRatio || 1;
  const hasFilter = "filter" in CanvasRenderingContext2D.prototype;

  onMount(() => {
    api.setViewerActive(true);
    const box = boxFor(window.innerWidth, window.innerHeight, dpr);
    loadFrame(path, entry.mtime, box).then((f) => {
      if (!f?.bmp) {
        store.showToast("이 형식은 편집할 수 없습니다", { error: true });
        store.editor = null;
        return;
      }
      frame = f;
      bw = f.fw;
      bh = f.fh;
    });
    return () => api.setViewerActive(store.viewer >= 0 || !!store.compare);
  });

  // ───── 그리기 ─────
  /** 화면 1px당 결과 픽셀 수의 역 (결과 1px → 캔버스 k px) */
  let k = $derived(view.w && sw ? Math.min((sw * dpr) / view.w, (sh * dpr) / view.h) : 1);

  function applyMat(m: Mat, x: number, y: number): [number, number] {
    return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
  }
  function mul(a: Mat, b: Mat): Mat {
    // a ∘ b (b 먼저)
    return [
      a[0] * b[0] + a[2] * b[1],
      a[1] * b[0] + a[3] * b[1],
      a[0] * b[2] + a[2] * b[3],
      a[1] * b[2] + a[3] * b[3],
      a[0] * b[4] + a[2] * b[5] + a[4],
      a[1] * b[4] + a[3] * b[5] + a[5],
    ];
  }
  function norm(r: Rect): Rect {
    return { x: Math.min(r.x, r.x + r.w), y: Math.min(r.y, r.y + r.h), w: Math.abs(r.w), h: Math.abs(r.h) };
  }

  const textBoxes = new Map<Op, Rect>();

  function drawOp(ctx: CanvasRenderingContext2D, op: Op, scale: number) {
    ctx.save();
    switch (op.k) {
      case "pen":
      case "line":
      case "arrow": {
        ctx.strokeStyle = op.color;
        ctx.lineWidth = op.lw;
        ctx.globalAlpha = op.alpha;
        ctx.lineCap = op.alpha < 1 ? "butt" : "round";
        ctx.lineJoin = "round";
        ctx.beginPath();
        if (op.k === "pen") {
          ctx.moveTo(op.pts[0], op.pts[1]);
          for (let i = 2; i < op.pts.length; i += 2) ctx.lineTo(op.pts[i], op.pts[i + 1]);
          if (op.pts.length === 2) ctx.lineTo(op.pts[0] + 0.1, op.pts[1]);
        } else {
          ctx.moveTo(op.x1, op.y1);
          ctx.lineTo(op.x2, op.y2);
        }
        ctx.stroke();
        if (op.k === "arrow") {
          const a = Math.atan2(op.y2 - op.y1, op.x2 - op.x1);
          const head = Math.max(op.lw * 5, 12 / scale);
          ctx.fillStyle = op.color;
          ctx.beginPath();
          ctx.moveTo(op.x2 + Math.cos(a) * op.lw, op.y2 + Math.sin(a) * op.lw);
          ctx.lineTo(op.x2 - head * Math.cos(a - 0.45), op.y2 - head * Math.sin(a - 0.45));
          ctx.lineTo(op.x2 - head * Math.cos(a + 0.45), op.y2 - head * Math.sin(a + 0.45));
          ctx.closePath();
          ctx.fill();
        }
        break;
      }
      case "rect":
      case "ellipse": {
        const r = op.r;
        ctx.beginPath();
        if (op.k === "rect") ctx.rect(r.x, r.y, r.w, r.h);
        else ctx.ellipse(r.x + r.w / 2, r.y + r.h / 2, Math.max(0.5, r.w / 2), Math.max(0.5, r.h / 2), 0, 0, Math.PI * 2);
        if (op.fill) {
          ctx.fillStyle = op.color;
          ctx.fill();
        } else {
          ctx.strokeStyle = op.color;
          ctx.lineWidth = op.lw;
          ctx.stroke();
        }
        break;
      }
      case "text": {
        ctx.translate(op.x, op.y);
        ctx.transform(op.m[0], op.m[1], op.m[2], op.m[3], 0, 0);
        ctx.font = `600 ${op.size}px -apple-system, "Apple SD Gothic Neo", "Malgun Gothic", sans-serif`;
        ctx.textBaseline = "top";
        const lines = op.text.split("\n");
        const lh = op.size * 1.25;
        let width = 0;
        lines.forEach((ln, i) => {
          width = Math.max(width, ctx.measureText(ln).width);
          if (op.outline) {
            ctx.strokeStyle = op.color === "#000000" ? "#ffffff" : "#000000";
            ctx.lineWidth = Math.max(2, op.size / 8);
            ctx.lineJoin = "round";
            ctx.strokeText(ln, 0, i * lh);
          }
          ctx.fillStyle = op.color;
          ctx.fillText(ln, 0, i * lh);
        });
        // 선택용 상자 (결과 좌표)
        const corners = [
          [0, 0],
          [width, 0],
          [0, lines.length * lh],
          [width, lines.length * lh],
        ].map(([x, y]) => [op.x + op.m[0] * x + op.m[2] * y, op.y + op.m[1] * x + op.m[3] * y]);
        const xs = corners.map((c) => c[0]);
        const ys = corners.map((c) => c[1]);
        textBoxes.set(op, { x: Math.min(...xs), y: Math.min(...ys), w: Math.max(...xs) - Math.min(...xs), h: Math.max(...ys) - Math.min(...ys) });
        break;
      }
      case "mosaic":
      case "blur": {
        // 지금까지 그린 결과에서 그 영역을 가져와 모자이크(작게 → 각지게 크게) 또는 흐리게
        const t = ctx.getTransform();
        const sx = Math.round(t.a * op.r.x + t.e);
        const sy = Math.round(t.d * op.r.y + t.f);
        const sw2 = Math.round(t.a * op.r.w);
        const sh2 = Math.round(t.d * op.r.h);
        const cx0 = Math.max(0, sx), cy0 = Math.max(0, sy);
        const cx1 = Math.min(ctx.canvas.width, sx + sw2), cy1 = Math.min(ctx.canvas.height, sy + sh2);
        if (cx1 - cx0 < 1 || cy1 - cy0 < 1) break;
        const bx = Math.max(1, op.block * t.a);
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        if (op.k === "mosaic") {
          const tw = Math.max(1, Math.round(sw2 / bx));
          const th = Math.max(1, Math.round(sh2 / bx));
          const tmp = new OffscreenCanvas(tw, th);
          const tc = tmp.getContext("2d")!;
          tc.imageSmoothingQuality = "high";
          tc.drawImage(ctx.canvas, sx, sy, sw2, sh2, 0, 0, tw, th);
          ctx.beginPath();
          ctx.rect(cx0, cy0, cx1 - cx0, cy1 - cy0);
          ctx.clip();
          ctx.imageSmoothingEnabled = false;
          ctx.drawImage(tmp, 0, 0, tw, th, sx, sy, sw2, sh2);
        } else {
          const pad = Math.ceil(bx * 2);
          const px0 = Math.max(0, cx0 - pad), py0 = Math.max(0, cy0 - pad);
          const pw = Math.min(ctx.canvas.width, cx1 + pad) - px0, ph = Math.min(ctx.canvas.height, cy1 + pad) - py0;
          const tmp = new OffscreenCanvas(pw, ph);
          tmp.getContext("2d")!.drawImage(ctx.canvas, px0, py0, pw, ph, 0, 0, pw, ph);
          ctx.beginPath();
          ctx.rect(cx0, cy0, cx1 - cx0, cy1 - cy0);
          ctx.clip();
          if (hasFilter) {
            ctx.filter = `blur(${bx * 0.8}px)`;
            ctx.drawImage(tmp, px0, py0);
          } else {
            // 필터가 없으면 크게 줄였다가 부드럽게 키운다
            const tw = Math.max(1, Math.round(pw / bx)), th = Math.max(1, Math.round(ph / bx));
            const small = new OffscreenCanvas(tw, th);
            small.getContext("2d")!.drawImage(tmp, 0, 0, tw, th);
            ctx.imageSmoothingEnabled = true;
            ctx.drawImage(small, 0, 0, tw, th, px0, py0, pw, ph);
          }
        }
        break;
      }
    }
    ctx.restore();
  }

  /** 결과의 v 영역을 ctx에 k배로 그린다. base는 원본 비율 그대로의 사진(화면 크기든 원본 해상도든). */
  function render(ctx: CanvasRenderingContext2D, base: CanvasImageSource, v: Rect, scale: number, list: Op[]) {
    ctx.save();
    ctx.setTransform(scale, 0, 0, scale, -v.x * scale, -v.y * scale);
    ctx.save();
    ctx.transform(...mat);
    if (hasFilter && (adj.bright !== 100 || adj.contrast !== 100 || adj.saturate !== 100)) {
      ctx.filter = `brightness(${adj.bright}%) contrast(${adj.contrast}%) saturate(${adj.saturate}%)`;
    }
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(base, 0, 0, bw, bh);
    ctx.restore();
    for (const op of list) drawOp(ctx, op, scale);
    ctx.restore();
  }

  /** 그리는 중인 개체 (아직 목록에 없음) */
  let draft = $state.raw<Op | null>(null);
  /** 자르기 중인 영역 */
  let cropDraft = $state.raw<Rect | null>(null);

  $effect(() => {
    if (!canvas || !frame?.bmp || !bw) return;
    const W = Math.max(1, Math.round(view.w * k));
    const H = Math.max(1, Math.round(view.h * k));
    if (canvas.width !== W) canvas.width = W;
    if (canvas.height !== H) canvas.height = H;
    const ctx = canvas.getContext("2d")!;
    void [adj.bright, adj.contrast, adj.saturate];
    ctx.clearRect(0, 0, W, H);
    render(ctx, frame.bmp, view, k, draft ? [...ops, draft] : ops);
  });

  // ───── 기록 (실행 취소) ─────
  function snap(): Snap {
    return { ops, crop, mat, adj: { ...adj } };
  }
  function restore(s: Snap) {
    ops = s.ops;
    crop = s.crop;
    mat = s.mat;
    adj = { ...s.adj };
    selected = -1;
  }
  function commit(change: () => void) {
    undoStack.push(snap());
    redoStack = [];
    change();
    dirty = true;
    canUndo = true;
    canRedo = false;
  }
  export function undo() {
    if (editing) return finishText(false);
    const s = undoStack.pop();
    if (!s) return;
    redoStack.push(snap());
    restore(s);
    canUndo = undoStack.length > 0;
    canRedo = true;
  }
  function redo() {
    const s = redoStack.pop();
    if (!s) return;
    undoStack.push(snap());
    restore(s);
    canUndo = true;
    canRedo = redoStack.length > 0;
  }

  // ───── 회전·반전 ─────
  /** 결과 좌표에 대한 변환 t를 사진·개체·자르기 영역 모두에 적용 */
  function turn(kind: "cw" | "ccw" | "h" | "v") {
    const W = fw, H = fh;
    const t: Mat =
      kind === "cw" ? [0, 1, -1, 0, H, 0] : kind === "ccw" ? [0, -1, 1, 0, 0, W] : kind === "h" ? [-1, 0, 0, 1, W, 0] : [1, 0, 0, -1, 0, H];
    const pt = (x: number, y: number) => applyMat(t, x, y);
    const rect = (r: Rect): Rect => {
      const [x1, y1] = pt(r.x, r.y);
      const [x2, y2] = pt(r.x + r.w, r.y + r.h);
      return norm({ x: x1, y: y1, w: x2 - x1, h: y2 - y1 });
    };
    commit(() => {
      mat = mul(t, mat);
      crop = crop && rect(crop);
      ops = ops.map((op): Op => {
        switch (op.k) {
          case "pen": {
            const pts: number[] = [];
            for (let i = 0; i < op.pts.length; i += 2) pts.push(...pt(op.pts[i], op.pts[i + 1]));
            return { ...op, pts };
          }
          case "line":
          case "arrow": {
            const [x1, y1] = pt(op.x1, op.y1);
            const [x2, y2] = pt(op.x2, op.y2);
            return { ...op, x1, y1, x2, y2 };
          }
          case "text": {
            const [x, y] = pt(op.x, op.y);
            const m = mul([t[0], t[1], t[2], t[3], 0, 0], [op.m[0], op.m[1], op.m[2], op.m[3], 0, 0]);
            return { ...op, x, y, m: m.slice(0, 4) };
          }
          default:
            return { ...op, r: rect(op.r) };
        }
      });
    });
  }

  // ───── 마우스 ─────
  function toImage(e: PointerEvent): [number, number] {
    const b = canvas!.getBoundingClientRect();
    return [view.x + ((e.clientX - b.left) * dpr) / k, view.y + ((e.clientY - b.top) * dpr) / k];
  }

  let press: { x: number; y: number; mode: string; orig?: Rect | Op; idx?: number } | null = null;

  function bbox(op: Op): Rect {
    switch (op.k) {
      case "pen": {
        const xs = op.pts.filter((_, i) => i % 2 === 0);
        const ys = op.pts.filter((_, i) => i % 2 === 1);
        const p = op.lw / 2;
        return { x: Math.min(...xs) - p, y: Math.min(...ys) - p, w: Math.max(...xs) - Math.min(...xs) + 2 * p, h: Math.max(...ys) - Math.min(...ys) + 2 * p };
      }
      case "line":
      case "arrow":
        return norm({ x: op.x1, y: op.y1, w: op.x2 - op.x1, h: op.y2 - op.y1 });
      case "text":
        return textBoxes.get(op) ?? { x: op.x, y: op.y, w: op.size, h: op.size };
      default:
        return op.r;
    }
  }

  function hit(x: number, y: number): number {
    const tol = 8 / k * dpr;
    for (let i = ops.length - 1; i >= 0; i--) {
      const b = bbox(ops[i]);
      if (x >= b.x - tol && x <= b.x + b.w + tol && y >= b.y - tol && y <= b.y + b.h + tol) return i;
    }
    return -1;
  }

  function moveOp(op: Op, dx: number, dy: number): Op {
    switch (op.k) {
      case "pen":
        return { ...op, pts: op.pts.map((v, i) => v + (i % 2 ? dy : dx)) };
      case "line":
      case "arrow":
        return { ...op, x1: op.x1 + dx, y1: op.y1 + dy, x2: op.x2 + dx, y2: op.y2 + dy };
      case "text":
        return { ...op, x: op.x + dx, y: op.y + dy };
      default:
        return { ...op, r: { ...op.r, x: op.r.x + dx, y: op.r.y + dy } };
    }
  }

  /** Shift: 선은 수평·수직·45°, 도형은 정사각형·원 */
  function constrain(x1: number, y1: number, x2: number, y2: number, shift: boolean, square: boolean): [number, number] {
    if (!shift) return [x2, y2];
    const dx = x2 - x1, dy = y2 - y1;
    if (square) {
      const s = Math.max(Math.abs(dx), Math.abs(dy));
      return [x1 + Math.sign(dx || 1) * s, y1 + Math.sign(dy || 1) * s];
    }
    const a = Math.round(Math.atan2(dy, dx) / (Math.PI / 4)) * (Math.PI / 4);
    const len = Math.hypot(dx, dy);
    return [x1 + Math.cos(a) * len, y1 + Math.sin(a) * len];
  }

  function cropAspect(): number | null {
    return ratio;
  }

  function onDown(e: PointerEvent) {
    if (!canvas || saving || e.button !== 0) return;
    // 누를 때 포커스가 편집 화면으로 옮겨 가면 방금 연 글자 상자가 바로 닫힌다
    e.preventDefault();
    if (editing) finishText(true);
    const [x, y] = toImage(e);
    canvas.setPointerCapture(e.pointerId);
    switch (tool) {
      case "select": {
        const i = hit(x, y);
        selected = i;
        if (i >= 0) press = { x, y, mode: "move", orig: ops[i], idx: i };
        return;
      }
      case "crop": {
        const c = cropDraft ?? { x: 0, y: 0, w: fw, h: fh };
        const tol = (14 * dpr) / k;
        const near = (a: number, b: number) => Math.abs(a - b) < tol;
        let mode = "new";
        // 아직 사진 전체면 안쪽을 끌어도 새 영역을 그린다 (전체 틀을 옮기는 것은 의미가 없다)
        const whole = c.x <= 0.5 && c.y <= 0.5 && c.w >= fw - 1 && c.h >= fh - 1;
        const l = near(x, c.x), r = near(x, c.x + c.w), t = near(y, c.y), b = near(y, c.y + c.h);
        const inX = x > c.x - tol && x < c.x + c.w + tol, inY = y > c.y - tol && y < c.y + c.h + tol;
        if ((l || r) && inY || (t || b) && inX) mode = `edge:${l ? "l" : r ? "r" : ""}${t ? "t" : b ? "b" : ""}`;
        else if (!whole && x > c.x && x < c.x + c.w && y > c.y && y < c.y + c.h) mode = "move";
        press = { x, y, mode, orig: c };
        return;
      }
      case "text":
        startText(x, y);
        return;
      case "pen":
      case "marker":
        draft = {
          k: "pen",
          pts: [x, y],
          color,
          lw: tool === "marker" ? lw * 4 : lw,
          alpha: tool === "marker" ? 0.4 : 1,
        };
        break;
      case "line":
      case "arrow":
        draft = { k: tool, x1: x, y1: y, x2: x, y2: y, color, lw, alpha: 1 };
        break;
      case "rect":
      case "ellipse":
        draft = { k: tool, r: { x, y, w: 0, h: 0 }, color, lw, fill };
        break;
      case "mosaic":
      case "blur":
        draft = { k: tool, r: { x, y, w: 0, h: 0 }, block };
        break;
    }
    press = { x, y, mode: "draw" };
  }

  function onMove(e: PointerEvent) {
    if (!press) return;
    const [x, y] = toImage(e);
    const dx = x - press.x, dy = y - press.y;
    if (press.mode === "move" && tool === "select" && press.idx !== undefined) {
      const moved = moveOp(press.orig as Op, dx, dy);
      ops = ops.map((o, i) => (i === press!.idx ? moved : o));
      return;
    }
    if (tool === "crop") {
      const o = press.orig as Rect;
      let r: Rect;
      if (press.mode === "new") {
        let [x2, y2] = [x, y];
        const a = cropAspect();
        if (a) {
          const w = Math.abs(x2 - press.x), h = Math.abs(y2 - press.y);
          const s = Math.max(w / a, h);
          x2 = press.x + Math.sign(x2 - press.x || 1) * s * a;
          y2 = press.y + Math.sign(y2 - press.y || 1) * s;
        }
        r = norm({ x: press.x, y: press.y, w: x2 - press.x, h: y2 - press.y });
      } else if (press.mode === "move") {
        r = { ...o, x: Math.min(Math.max(0, o.x + dx), fw - o.w), y: Math.min(Math.max(0, o.y + dy), fh - o.h) };
      } else {
        const sides = press.mode.slice(5);
        let { x: rx, y: ry, w: rw, h: rh } = o;
        if (sides.includes("l")) { rx = o.x + dx; rw = o.w - dx; }
        if (sides.includes("r")) rw = o.w + dx;
        if (sides.includes("t")) { ry = o.y + dy; rh = o.h - dy; }
        if (sides.includes("b")) rh = o.h + dy;
        const a = cropAspect();
        if (a) {
          if (sides.includes("l") || sides.includes("r")) rh = rw / a;
          else rw = rh * a;
        }
        r = norm({ x: rx, y: ry, w: rw, h: rh });
      }
      // 사진 밖으로 나가지 않게
      const x0 = Math.max(0, r.x), y0 = Math.max(0, r.y);
      cropDraft = { x: x0, y: y0, w: Math.min(fw, r.x + r.w) - x0, h: Math.min(fh, r.y + r.h) - y0 };
      return;
    }
    if (!draft) return;
    const d = draft;
    switch (d.k) {
      case "pen":
        draft = { ...d, pts: [...d.pts, x, y] };
        break;
      case "line":
      case "arrow": {
        const [x2, y2] = constrain(d.x1, d.y1, x, y, e.shiftKey, false);
        draft = { ...d, x2, y2 };
        break;
      }
      default: {
        const [x2, y2] = constrain(press.x, press.y, x, y, e.shiftKey, true);
        draft = { ...d, r: norm({ x: press.x, y: press.y, w: x2 - press.x, h: y2 - press.y }) } as Op;
      }
    }
  }

  function onUp() {
    if (!press) return;
    const p = press;
    press = null;
    if (tool === "select" && p.mode === "move" && p.idx !== undefined) {
      const now = ops[p.idx];
      if (now !== p.orig) {
        // 옮긴 결과를 기록 (옮기기 전 상태로 되돌릴 수 있게)
        const after = ops;
        ops = ops.map((o, i) => (i === p.idx ? (p.orig as Op) : o));
        commit(() => (ops = after));
      }
      return;
    }
    if (!draft) return;
    const d = draft;
    draft = null;
    const tiny = "r" in d ? d.r.w < 2 || d.r.h < 2 : d.k === "line" || d.k === "arrow" ? Math.hypot(d.x2 - d.x1, d.y2 - d.y1) < 2 : false;
    if (tiny) return;
    commit(() => (ops = [...ops, d]));
  }

  // ───── 텍스트 ─────
  let editing = $state<{ x: number; y: number; text: string; idx: number } | null>(null);
  let textInput = $state<HTMLTextAreaElement>();

  async function startText(x: number, y: number, idx = -1) {
    const op = idx >= 0 ? (ops[idx] as Extract<Op, { k: "text" }>) : null;
    editing = { x: op?.x ?? x, y: op?.y ?? y, text: op?.text ?? "", idx };
    await tick();
    // 클릭이 끝난 뒤에 포커스 (누르는 중에 주면 놓을 때 빼앗긴다)
    setTimeout(() => textInput?.focus(), 0);
  }

  function finishText(keep: boolean) {
    const ed = editing;
    editing = null;
    if (!ed || !keep) return;
    const text = ed.text.replace(/\s+$/, "");
    if (ed.idx >= 0) {
      const old = ops[ed.idx] as Extract<Op, { k: "text" }>;
      if (!text) commit(() => (ops = ops.filter((_, i) => i !== ed.idx)));
      else if (text !== old.text) commit(() => (ops = ops.map((o, i) => (i === ed.idx ? { ...old, text } : o))));
      return;
    }
    if (!text) return;
    commit(() => (ops = [...ops, { k: "text", x: ed.x, y: ed.y, text, size: textSize, color, outline, m: [1, 0, 0, 1] }]));
  }

  function onDblClick(e: MouseEvent) {
    if (tool !== "select") return;
    const [x, y] = toImage(e as PointerEvent);
    const i = hit(x, y);
    if (i >= 0 && ops[i].k === "text") startText(0, 0, i);
  }

  // 텍스트 입력 상자의 화면 위치
  let textStyle = $derived.by(() => {
    if (!editing || !canvas) return "";
    const css = k / dpr;
    const left = (editing.x - view.x) * css;
    const top = (editing.y - view.y) * css;
    const op = editing.idx >= 0 ? (ops[editing.idx] as Extract<Op, { k: "text" }>) : null;
    const size = (op?.size ?? textSize) * css;
    return `left:${left}px;top:${top}px;font-size:${size}px;color:${op?.color ?? color}`;
  });

  // ───── 자르기 ─────
  function startCrop() {
    tool = "crop";
    cropDraft = crop ?? { x: 0, y: 0, w: fw, h: fh };
    selected = -1;
  }
  function applyCrop() {
    const c = cropDraft;
    tool = "select";
    cropDraft = null;
    if (!c || c.w < 4 || c.h < 4) return;
    const r = { x: Math.round(c.x), y: Math.round(c.y), w: Math.round(c.w), h: Math.round(c.h) };
    const full = r.x <= 0 && r.y <= 0 && r.w >= fw && r.h >= fh;
    commit(() => (crop = full ? null : r));
  }
  function cancelCrop() {
    cropDraft = null;
    tool = "select";
  }
  function setRatio(v: number | null | "orig") {
    ratio = v === "orig" ? fw / fh : v;
    if (ratio && cropDraft) {
      // 가운데 기준으로 비율 맞추기
      const c = cropDraft;
      let w = c.w, h = c.w / ratio;
      if (h > c.h) {
        h = c.h;
        w = h * ratio;
      }
      cropDraft = { x: c.x + (c.w - w) / 2, y: c.y + (c.h - h) / 2, w, h };
    }
  }
  // 자르기 표시 (캔버스 위 CSS 좌표)
  let cropBox = $derived.by(() => {
    if (tool !== "crop" || !cropDraft) return null;
    const css = k / dpr;
    return { l: cropDraft.x * css, t: cropDraft.y * css, w: cropDraft.w * css, h: cropDraft.h * css };
  });
  let selBox = $derived.by(() => {
    if (tool !== "select" || selected < 0 || !ops[selected]) return null;
    void ops;
    const b = bbox(ops[selected]);
    const css = k / dpr;
    return { l: (b.x - view.x) * css, t: (b.y - view.y) * css, w: b.w * css, h: b.h * css };
  });

  function setTool(t: Tool) {
    if (editing) finishText(true);
    if (tool === "crop" && t !== "crop") applyCrop();
    if (t === "crop") return startCrop();
    tool = t;
    if (t !== "select") selected = -1;
  }

  function deleteSelected() {
    if (selected < 0) return;
    const i = selected;
    commit(() => (ops = ops.filter((_, j) => j !== i)));
    selected = -1;
  }

  // 선택한 개체의 색 바꾸기
  function pickColor(c: string) {
    color = c;
    if (tool === "select" && selected >= 0) {
      const i = selected;
      const op = ops[i];
      if ("color" in op && op.color !== c) commit(() => (ops = ops.map((o, j) => (j === i ? ({ ...o, color: c } as Op) : o))));
    }
  }

  // ───── 저장 ─────
  async function fullBase(): Promise<ImageBitmap> {
    const r = await fetch(viewUrl(path, 0, 0));
    if (!r.ok) throw new Error("원본을 읽을 수 없습니다");
    if (r.headers.get("x-route") === "pixels") {
      const w = Number(r.headers.get("x-w")), h = Number(r.headers.get("x-h"));
      const buf = await r.arrayBuffer();
      return createImageBitmap(new ImageData(new Uint8ClampedArray(buf), w, h));
    }
    return createImageBitmap(await r.blob(), { imageOrientation: "from-image" });
  }

  let srcFormat = $derived((path.split(".").pop() ?? "").toLowerCase());
  let outFormat = $derived<OutFormat>(
    format !== "same"
      ? format
      : srcFormat === "png"
        ? "png"
        : srcFormat === "webp"
          ? "webp"
          : srcFormat === "heic" || srcFormat === "heif"
            ? "heic"
            : "jpeg",
  );
  let lossy = $derived(outFormat !== "png" && outFormat !== "bmp" && outFormat !== "tiff");
  let outSize = $derived.by(() => {
    const v = crop ?? { x: 0, y: 0, w: fw, h: fh };
    const L = Math.max(v.w, v.h);
    const target = resize === "none" ? L : resize.endsWith("%") ? (L * parseInt(resize)) / 100 : Math.min(L, parseInt(resize));
    const s = target / L;
    return [Math.round(v.w * s), Math.round(v.h * s)];
  });

  async function save(overwrite: boolean) {
    if (saving || !frame) return;
    if (editing) finishText(true);
    if (tool === "crop") applyCrop();
    if (overwrite) {
      const ok = await ask(`원본 "${baseName(path)}"에 덮어쓸까요?\n원본은 보관되어 ${isMac ? "⌘Z" : "Ctrl+Z"}로 되돌릴 수 있습니다.`, {
        title: "원본에 저장",
        kind: "warning",
        okLabel: "덮어쓰기",
        cancelLabel: "취소",
      });
      if (!ok) return;
    }
    saving = "원본 해상도로 그리는 중…";
    const t0 = performance.now();
    let base: ImageBitmap | null = null;
    try {
      base = await fullBase();
      const v = crop ?? { x: 0, y: 0, w: fw, h: fh };
      const c = document.createElement("canvas");
      c.width = v.w;
      c.height = v.h;
      const ctx = c.getContext("2d", { willReadFrequently: true })!;
      render(ctx, base, v, 1, ops);
      const data = ctx.getImageData(0, 0, v.w, v.h).data;
      c.width = c.height = 0;
      saving = "저장하는 중…";
      const spec: ConvertSpec = {
        save: overwrite ? { mode: "overwrite" } : { mode: "new_file" },
        suffix: "편집",
        // "원본과 같게"면 엔진이 원본 형식(저장할 수 없는 형식이면 JPG)으로 저장한다
        ...(format !== "same" ? { format } : {}),
        ...(lossy ? { quality } : {}),
        ...(resize === "none"
          ? {}
          : { resize: resize.endsWith("%") ? { kind: "percent", value: parseInt(resize) } : { kind: "long_side", value: parseInt(resize) } }),
      } as ConvertSpec;
      const d = await invoke<{ output: string; bytes: number; width: number; height: number }>(
        "save_edit",
        new Uint8Array(data.buffer, data.byteOffset, data.byteLength),
        { headers: { "x-edit": encodeURIComponent(JSON.stringify({ path, width: v.w, height: v.h, spec })) } },
      );
      const sec = ((performance.now() - t0) / 1000).toFixed(1);
      store.showToast(`${baseName(d.output)} 저장 · ${d.width}×${d.height} · ${fmtSize(d.bytes)} · ${sec}초`, { undo: true });
      dirty = false;
      store.editor = null;
      await store.reload();
      // 새 파일이면 그 파일을 고른다
      const i = store.entries.findIndex((e) => e.path === d.output);
      if (i >= 0) {
        if (store.viewer >= 0) store.viewer = i;
        store.selectOnly(i);
        store.scrollTo = i;
      }
    } catch (e) {
      store.showToast(`저장하지 못했습니다: ${e}`, { error: true });
    } finally {
      base?.close();
      saving = null;
    }
  }

  export async function close() {
    if (saving) return;
    if (dirty) {
      const ok = await ask("편집한 내용을 저장하지 않고 닫을까요?", { title: "편집 닫기", kind: "warning", okLabel: "닫기", cancelLabel: "계속 편집" });
      if (!ok) return;
    }
    store.editor = null;
  }

  /** App에서 넘겨주는 키. 처리했으면 true */
  export function onKey(e: KeyboardEvent): boolean {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.code === "KeyZ") {
      if (e.shiftKey) redo();
      else undo();
      return true;
    }
    if (mod && e.code === "KeyS") {
      save(false);
      return true;
    }
    if (e.key === "Escape") {
      if (tool === "crop") cancelCrop();
      else if (selected >= 0) selected = -1;
      else close();
      return true;
    }
    if (e.key === "Enter" && tool === "crop") {
      applyCrop();
      return true;
    }
    if ((e.key === "Delete" || e.key === "Backspace") && selected >= 0) {
      deleteSelected();
      return true;
    }
    if (mod || e.altKey) return false;
    const t = TOOLS.find((x) => `Key${x.key}` === e.code);
    if (t) {
      setTool(t.t);
      return true;
    }
    const digit = e.code.match(/^Digit([1-3])$/);
    if (digit) {
      weight = Number(digit[1]);
      return true;
    }
    return false;
  }
</script>

<div class="editor" role="dialog" aria-label="편집" tabindex="-1" oncontextmenu={(e) => e.preventDefault()}>
  <div class="topbar">
    <span class="title">편집</span>
    <span class="dim" title={path}>{baseName(path)}</span>
    {#if bw}<span class="dim small">{fw}×{fh}{crop ? ` → ${crop.w}×${crop.h}` : ""}</span>{/if}
    <span class="spacer"></span>
    <button class="icon-btn" title="실행 취소 ({isMac ? '⌘Z' : 'Ctrl+Z'})" disabled={!canUndo} onclick={undo}>↶</button>
    <button class="icon-btn" title="다시 실행 ({isMac ? '⇧⌘Z' : 'Ctrl+Shift+Z'})" disabled={!canRedo} onclick={redo}>↷</button>
    <span class="sep"></span>
    <button class="btn" onclick={() => (saveOpen = !saveOpen)} class:on={saveOpen} title="형식·크기·화질">저장 설정</button>
    <button class="btn" disabled={!!saving || !frame} onclick={() => save(true)}>원본에 저장</button>
    <button class="btn primary" disabled={!!saving || !frame} onclick={() => save(false)} title="{isMac ? '⌘S' : 'Ctrl+S'}">새 파일로 저장</button>
    <button class="icon-btn" title="닫기 (Esc)" onclick={close}><Icon name="close" /></button>
  </div>

  <div class="main">
    <div class="tools" role="toolbar" aria-label="도구">
      {#each TOOLS as t (t.t)}
        <button class="tool" class:on={tool === t.t} title="{t.label} ({t.key})" onclick={() => setTool(t.t)}>
          <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d={t.icon} /></svg>
          <span>{t.label}</span>
        </button>
      {/each}
      <span class="gap"></span>
      <button class="tool" title="왼쪽으로 90°" onclick={() => turn("ccw")}>⟲<span>왼쪽 90°</span></button>
      <button class="tool" title="오른쪽으로 90°" onclick={() => turn("cw")}>⟳<span>오른쪽 90°</span></button>
      <button class="tool" title="좌우 뒤집기" onclick={() => turn("h")}>⇋<span>좌우 반전</span></button>
      <button class="tool" title="상하 뒤집기" onclick={() => turn("v")}>⇵<span>상하 반전</span></button>
    </div>

    <div class="stage" bind:this={stage} bind:clientWidth={sw} bind:clientHeight={sh}>
      {#if !frame}
        <p class="loading">불러오는 중…</p>
      {/if}
      <div class="canvas-wrap" style="width:{(view.w * k) / dpr}px;height:{(view.h * k) / dpr}px">
        <canvas
          bind:this={canvas}
          class:crosshair={tool !== "select" && tool !== "crop"}
          style="width:{(view.w * k) / dpr}px;height:{(view.h * k) / dpr}px"
          onpointerdown={onDown}
          onpointermove={onMove}
          onpointerup={onUp}
          onpointercancel={onUp}
          ondblclick={onDblClick}
        ></canvas>
        {#if cropBox}
          <div class="crop" style="left:{cropBox.l}px;top:{cropBox.t}px;width:{cropBox.w}px;height:{cropBox.h}px">
            <span class="grid"></span>
            {#each ["tl", "t", "tr", "r", "br", "b", "bl", "l"] as h (h)}<span class="handle {h}"></span>{/each}
          </div>
        {/if}
        {#if selBox}
          <div class="sel" style="left:{selBox.l - 4}px;top:{selBox.t - 4}px;width:{selBox.w + 8}px;height:{selBox.h + 8}px"></div>
        {/if}
        {#if editing}
          <textarea
            bind:this={textInput}
            bind:value={editing.text}
            class="text-input"
            style={textStyle}
            rows="1"
            placeholder="글자 입력"
            onkeydown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                finishText(false);
              } else if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
                e.preventDefault();
                finishText(true);
              }
            }}
            onblur={() => finishText(true)}
          ></textarea>
        {/if}
      </div>
      {#if saving}
        <div class="saving">{saving}</div>
      {/if}
    </div>

    <aside class="props">
      {#if tool === "crop"}
        <h3>자르기 비율</h3>
        <div class="chips">
          {#each RATIOS as r (r.label)}
            <button class:on={(r.v === "orig" ? ratio === fw / fh : ratio === r.v)} onclick={() => setRatio(r.v)}>{r.label}</button>
          {/each}
        </div>
        <p class="note">사진 위를 끌어 영역을 정하고, 가장자리를 끌어 조절합니다.</p>
        {#if cropDraft}<p class="note">{Math.round(cropDraft.w)}×{Math.round(cropDraft.h)}</p>{/if}
        <div class="row">
          <button class="btn" onclick={() => (cropDraft = { x: 0, y: 0, w: fw, h: fh })}>전체</button>
          <button class="btn" onclick={cancelCrop}>취소</button>
          <button class="btn primary" onclick={applyCrop} title="Enter">적용</button>
        </div>
      {:else}
        <h3>색</h3>
        <div class="colors">
          {#each COLORS as c (c)}
            <button class="swatch" class:on={color === c} style="background:{c}" title={c} aria-label={c} onclick={() => pickColor(c)}></button>
          {/each}
          <input type="color" value={color} oninput={(e) => pickColor(e.currentTarget.value)} title="다른 색" />
        </div>
        <h3>{tool === "text" ? "글자 크기" : tool === "mosaic" || tool === "blur" ? "세기" : "굵기"}</h3>
        <div class="seg">
          {#each [1, 2, 3] as w (w)}
            <button class:on={weight === w} onclick={() => (weight = w)}>{["", "작게", "보통", "크게"][w]}</button>
          {/each}
        </div>
        {#if tool === "rect" || tool === "ellipse"}
          <label class="check"><input type="checkbox" bind:checked={fill} /> 안을 채우기</label>
        {/if}
        {#if tool === "text"}
          <label class="check"><input type="checkbox" bind:checked={outline} /> 글자 테두리</label>
          <p class="note">사진을 클릭하고 입력한 뒤 Enter. 선택 도구로 끌어 옮기고, 두 번 클릭해 고칩니다.</p>
        {/if}
        {#if tool === "mosaic" || tool === "blur"}
          <p class="note">가릴 곳을 끌어서 지정합니다.</p>
        {/if}
        {#if tool === "line" || tool === "arrow" || tool === "rect" || tool === "ellipse"}
          <p class="note">Shift를 누르면 {tool === "line" || tool === "arrow" ? "수평·수직·45°" : "정사각형·정원"}</p>
        {/if}
        {#if tool === "select"}
          <p class="note">개체를 클릭해 고르고 끌어서 옮깁니다. Delete로 지웁니다. 색을 누르면 고른 개체의 색이 바뀝니다.</p>
          {#if selected >= 0}<button class="btn danger" onclick={deleteSelected}>고른 개체 지우기</button>{/if}
        {/if}

        {#if hasFilter}
          <h3>보정</h3>
          {#each [["bright", "밝기"], ["contrast", "대비"], ["saturate", "채도"]] as [key, label] (key)}
            <label class="slider">
              <span>{label}</span>
              <input
                type="range"
                min="50"
                max="150"
                value={adj[key as keyof Adjust]}
                onpointerdown={() => commit(() => {})}
                oninput={(e) => (adj[key as keyof Adjust] = Number(e.currentTarget.value))}
                ondblclick={() => commit(() => (adj[key as keyof Adjust] = 100))}
              />
              <span class="num">{adj[key as keyof Adjust]}%</span>
            </label>
          {/each}
        {/if}
      {/if}

      {#if saveOpen}
        <h3>저장</h3>
        <label class="row">
          형식
          <select bind:value={format}>
            <option value="same">원본과 같게</option>
            <option value="jpeg">JPG</option>
            <option value="png">PNG</option>
            <option value="webp">WebP</option>
            {#if isMac}<option value="heic">HEIC</option>{/if}
          </select>
        </label>
        <label class="row">
          크기
          <select bind:value={resize}>
            <option value="none">그대로</option>
            <option value="3840">긴 변 3840px</option>
            <option value="2560">긴 변 2560px</option>
            <option value="1920">긴 변 1920px</option>
            <option value="1280">긴 변 1280px</option>
            <option value="50%">50%</option>
          </select>
        </label>
        {#if lossy}
          <label class="slider"><span>화질</span><input type="range" min="40" max="100" bind:value={quality} /><span class="num">{quality}</span></label>
        {/if}
        <p class="note">결과 {outSize[0]}×{outSize[1]} · {outFormat.toUpperCase()} · 촬영 날짜·위치 정보는 그대로 남습니다.</p>
      {/if}
    </aside>
  </div>
</div>

<style>
  .editor {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    flex-direction: column;
    background: #0d0d0f;
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 44px;
    padding: 0 10px 0 16px;
    white-space: nowrap;
    border-bottom: 1px solid #1d1e22;
    background: var(--panel);
  }
  .title {
    font-weight: 600;
  }
  .dim {
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .small {
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .sep {
    width: 1px;
    height: 20px;
    background: var(--border);
  }
  .icon-btn {
    font-size: 17px;
  }
  .btn:disabled {
    opacity: 0.4;
  }
  .btn.on {
    border-color: var(--accent);
  }
  .btn.danger {
    margin-top: 8px;
    background: #3a2224;
    border-color: #5a2c30;
    color: #ffb4b4;
  }
  .main {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .tools {
    width: 74px;
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 2px;
    padding: 8px 6px;
    overflow-y: auto;
    border-right: 1px solid #1d1e22;
    background: var(--panel);
  }
  .tool {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 6px 2px;
    border-radius: 6px;
    color: var(--text-dim);
    font-size: 16px;
  }
  .tool span {
    font-size: 10px;
  }
  .tool:hover {
    background: var(--hover);
    color: var(--text);
  }
  .tool.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .gap {
    height: 10px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 8px;
  }
  .stage {
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    padding: 0;
  }
  .canvas-wrap {
    position: relative;
    flex: none;
  }
  canvas {
    display: block;
    touch-action: none;
  }
  canvas.crosshair {
    cursor: crosshair;
  }
  .loading {
    position: absolute;
    color: var(--text-dim);
  }
  .crop {
    position: absolute;
    outline: 1px solid #fff;
    box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0.55);
    pointer-events: none;
  }
  .crop .grid {
    position: absolute;
    inset: 0;
    background:
      linear-gradient(to right, transparent 33.2%, rgba(255, 255, 255, 0.35) 33.3%, transparent 33.4%, transparent 66.6%, rgba(255, 255, 255, 0.35) 66.7%, transparent 66.8%),
      linear-gradient(to bottom, transparent 33.2%, rgba(255, 255, 255, 0.35) 33.3%, transparent 33.4%, transparent 66.6%, rgba(255, 255, 255, 0.35) 66.7%, transparent 66.8%);
  }
  .handle {
    position: absolute;
    width: 12px;
    height: 12px;
    margin: -6px;
    background: #fff;
    border-radius: 2px;
  }
  .handle.tl { left: 0; top: 0; }
  .handle.t { left: 50%; top: 0; }
  .handle.tr { left: 100%; top: 0; }
  .handle.r { left: 100%; top: 50%; }
  .handle.br { left: 100%; top: 100%; }
  .handle.b { left: 50%; top: 100%; }
  .handle.bl { left: 0; top: 100%; }
  .handle.l { left: 0; top: 50%; }
  .sel {
    position: absolute;
    outline: 1px dashed var(--accent-line);
    pointer-events: none;
  }
  .text-input {
    position: absolute;
    min-width: 120px;
    padding: 0;
    margin: 0;
    border: 1px dashed var(--accent-line);
    background: rgba(0, 0, 0, 0.25);
    font-weight: 600;
    line-height: 1.25;
    resize: none;
    overflow: hidden;
    field-sizing: content;
    outline: none;
  }
  .saving {
    position: absolute;
    bottom: 20px;
    padding: 8px 14px;
    border-radius: 8px;
    background: rgba(20, 20, 22, 0.9);
    font-size: 13px;
  }
  .props {
    width: 230px;
    flex: none;
    overflow-y: auto;
    padding: 6px 14px 20px;
    border-left: 1px solid #1d1e22;
    background: var(--panel);
    font-size: 13px;
  }
  h3 {
    margin: 14px 0 8px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
  }
  .colors {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .swatch {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 2px solid var(--border);
  }
  .swatch.on {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-soft);
  }
  input[type="color"] {
    width: 26px;
    height: 24px;
    padding: 0;
    border: none;
    background: none;
  }
  .seg,
  .chips {
    display: flex;
    flex-wrap: wrap;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }
  .seg button,
  .chips button {
    flex: 1;
    padding: 5px 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .chips {
    border: none;
    gap: 4px;
  }
  .chips button {
    flex: 0 0 auto;
    border: 1px solid var(--border);
    border-radius: 5px;
  }
  .seg button.on,
  .chips button.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .check,
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 10px 0 0;
  }
  .row select {
    flex: 1;
  }
  .row .btn {
    flex: 1 1 auto;
    padding: 5px 8px;
    white-space: nowrap;
    font-size: 12px;
  }
  .slider {
    display: grid;
    grid-template-columns: 34px 1fr 40px;
    align-items: center;
    gap: 6px;
    margin: 6px 0;
  }
  .slider input {
    accent-color: var(--accent);
    min-width: 0;
  }
  .num {
    font-size: 11px;
    color: var(--text-dim);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .note {
    margin: 8px 0 0;
    font-size: 11px;
    color: var(--text-faint);
    line-height: 1.45;
  }
</style>
