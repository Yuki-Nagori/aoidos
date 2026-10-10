import "./style.css";

const documentId = crypto.randomUUID();

async function reportStyle(): Promise<void> {
  const marker = getComputedStyle(document.documentElement).getPropertyValue("--hmr-marker").trim();
  await fetch("/__report", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ documentId, marker }),
  });
}

void reportStyle();

if (import.meta.hot) {
  import.meta.hot.on("vite:afterUpdate", () => void reportStyle());
}
