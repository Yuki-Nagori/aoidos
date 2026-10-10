import { readFileSync, writeFileSync } from "node:fs";
import { defineConfig, type Plugin } from "vite";

const config = JSON.parse(readFileSync("../../../../../src-tauri/tauri.conf.json", "utf8")) as {
  app: { security: { devCsp: string } };
};

const report: Plugin = {
  name: "aoidos-hmr-test-report",
  configureServer(server) {
    server.middlewares.use("/__report", (request, response, next) => {
      if (request.method !== "POST") {
        next();
        return;
      }
      const chunks: Buffer[] = [];
      request.on("data", (chunk: Buffer) => chunks.push(chunk));
      request.on("end", () => {
        writeFileSync("hmr-report.json", Buffer.concat(chunks));
        response.writeHead(204).end();
      });
    });
  },
};

export default defineConfig({
  plugins: [report],
  server: { headers: { "Content-Security-Policy": config.app.security.devCsp } },
});
