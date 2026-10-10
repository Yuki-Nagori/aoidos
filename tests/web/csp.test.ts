import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

interface TauriConfig {
  app: {
    security: {
      csp: string;
      devCsp?: string;
    };
  };
}

function config(path: string): TauriConfig {
  return JSON.parse(readFileSync(resolve(__dirname, path), "utf8")) as TauriConfig;
}

describe("Tauri content security policy", () => {
  it("keeps development origins out of the production policy", () => {
    const { csp, devCsp } = config("../../src-tauri/tauri.conf.json").app.security;
    expect(csp).toContain("connect-src ipc: http://ipc.localhost");
    expect(csp).not.toMatch(/localhost:1420|asset:|http:\/\/asset\.localhost/);
    expect(devCsp).toContain("http://localhost:1420");
    expect(devCsp).toContain("ws://localhost:1420");
    expect(devCsp).not.toContain("asset:");
  });

  it("keeps the native IPC fixture on the restricted production policy", () => {
    const { csp } = config("../../tests/rust/native-platform/tauri.conf.json").app.security;
    expect(csp).toContain("connect-src ipc: http://ipc.localhost");
    expect(csp).not.toMatch(/localhost:1420|asset:|http:\/\/asset\.localhost/);
  });
});
