import "./style.css";

const documentId = crypto.randomUUID();

async function reportStyle(phase: "ready" | "after-update"): Promise<void> {
  const marker = getComputedStyle(document.documentElement).getPropertyValue("--hmr-marker").trim();
  await fetch("/__report", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ documentId, phase, marker }),
  });
}

function reportRuntimeError(error: string): void {
  void fetch("/__report", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ documentId, phase: "runtime-error", error }),
  }).catch(() => {
    // 避免报告通道本身失败时再次触发 unhandledrejection 循环。
  });
}

window.addEventListener("error", (event) => reportRuntimeError(event.message));
window.addEventListener("unhandledrejection", (event) => {
  const error = event.reason instanceof Error ? event.reason.message : String(event.reason);
  reportRuntimeError(error);
});

void reportStyle("ready");

if (import.meta.hot) {
  import.meta.hot.on("vite:afterUpdate", () => void reportStyle("after-update"));
}
