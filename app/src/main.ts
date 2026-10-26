import { invoke } from "@tauri-apps/api/core";
import { mark } from "./lib/api";
mark("스크립트");
import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";

// 화면 쪽에서 처리되지 않은 오류를 엔진 로그(stderr)로 보낸다 (문제 진단용)
const report = (msg: string) => invoke("perf_log", { label: `화면 오류: ${msg}`, ms: 0 }).catch(() => {});
window.addEventListener("error", (e) => report(`${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => report(String(e.reason?.stack ?? e.reason)));

let app;
try {
  app = mount(App, { target: document.getElementById("app")! });
} catch (e) {
  report(String((e as Error)?.stack ?? e));
  throw e;
}
export default app;
