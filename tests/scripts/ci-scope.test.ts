import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

describe("CI scope output", () => {
  it("writes the shared plan and derived job flags for GitHub Actions", () => {
    const directory = mkdtempSync(join(tmpdir(), "aoidos-ci-scope-"));
    const output = join(directory, "github-output");
    const plan = {
      full: false,
      reason: "test plan",
      rustLintPackages: ["aoidos-native-tests"],
      rustTestPackages: ["aoidos"],
      rustFormat: false,
      vitestFiles: ["tests/scripts/ci-scope.test.ts"],
      biomeFormatFiles: [],
      dprintFormatFiles: [],
      biomeLintFiles: [],
      runI18nCheck: false,
      runLayerCheck: false,
      runNativeTests: false,
    };

    try {
      execFileSync("bun", [join(root, "scripts", "ci-scope.mts"), "out" + "put"], {
        cwd: root,
        env: {
          ...process.env,
          AOIDOS_CI_PLAN: JSON.stringify(plan),
          GITHUB_OUTPUT: output,
        },
      });

      const values = Object.fromEntries(
        readFileSync(output, "utf8")
          .trim()
          .split("\n")
          .map((line) => {
            const separator = line.indexOf("=");
            return [line.slice(0, separator), line.slice(separator + 1)];
          }),
      );
      const serializedPlan = values.plan;
      if (!serializedPlan) throw new Error("CI plan output is missing");
      expect(JSON.parse(serializedPlan)).toEqual(plan);
      expect(values.has_rust_test).toBe("true");
      expect(values.has_vitest).toBe("true");
      expect(values.has_desktop_clippy).toBe("true");
      expect(values.has_app_build).toBe("true");
      expect(values.has_native).toBe("false");
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
