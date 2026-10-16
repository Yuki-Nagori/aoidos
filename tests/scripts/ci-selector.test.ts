import { describe, expect, it } from "vitest";
import type { CargoPackage } from "../../scripts/rust-test-selector";
import { createCiPlan } from "../../scripts/ci-selector";

const root = "/workspace/aoidos";
const packages: CargoPackage[] = [
  {
    name: "aoidos-store",
    manifest_path: `${root}/src-rust/aoidos-store/Cargo.toml`,
    dependencies: [],
  },
  {
    name: "aoidos-engine",
    manifest_path: `${root}/src-rust/aoidos-engine/Cargo.toml`,
    dependencies: [{ path: `${root}/src-rust/aoidos-store` }],
  },
  {
    name: "aoidos",
    manifest_path: `${root}/src-tauri/Cargo.toml`,
    dependencies: [{ path: `${root}/src-rust/aoidos-engine` }],
  },
  {
    name: "aoidos-native-tests",
    manifest_path: `${root}/tests/rust/native-platform/Cargo.toml`,
    dependencies: [{ path: `${root}/src-tauri` }],
  },
];
const knownTests = [
  "tests/web/composables/useBilling.test.ts",
  "tests/web/App.test.ts",
  "tests/scripts/rust-layers.test.ts",
  "tests/scripts/ci-scope.test.ts",
];

describe("CI scope selection", () => {
  it("selects mirrored frontend tests and changed TypeScript lint/format files", () => {
    const plan = createCiPlan(["src-web/composables/useBilling.ts"], packages, root, knownTests);

    expect(plan.vitestFiles).toEqual(["tests/web/composables/useBilling.test.ts"]);
    expect(plan.biomeFormatFiles).toEqual(["src-web/composables/useBilling.ts"]);
    expect(plan.biomeLintFiles).toEqual(["src-web/composables/useBilling.ts"]);
    expect(plan.rustTestPackages).toEqual([]);
    expect(plan.runNativeTests).toBe(false);
  });

  it("selects changed Rust packages and their consumers for test and lint", () => {
    const plan = createCiPlan(["src-rust/aoidos-store/src/lib.rs"], packages, root, knownTests);

    expect(plan.rustTestPackages).toEqual([
      "aoidos",
      "aoidos-engine",
      "aoidos-native-tests",
      "aoidos-store",
    ]);
    expect(plan.rustLintPackages).toEqual(plan.rustTestPackages);
    expect(plan.rustFormat).toBe(true);
    expect(plan.runLayerCheck).toBe(true);
    expect(plan.runNativeTests).toBe(true);
  });

  it("uses full checks when a source file has no mirrored behavior test", () => {
    const plan = createCiPlan(["src-web/components/NewPanel.vue"], packages, root, knownTests);

    expect(plan.vitestFiles).toBe("all");
    expect(plan.dprintFormatFiles).toEqual(["src-web/components/NewPanel.vue"]);
  });

  it("falls back to all checkers for shared tool configuration", () => {
    const plan = createCiPlan(["vitest.config.ts"], packages, root, knownTests);

    expect(plan.full).toBe(true);
    expect(plan.vitestFiles).toBe("all");
    expect(plan.biomeFormatFiles).toBe("all");
    expect(plan.dprintFormatFiles).toBe("all");
    expect(plan.rustTestPackages).toEqual([
      "aoidos",
      "aoidos-engine",
      "aoidos-native-tests",
      "aoidos-store",
    ]);
  });

  it("keeps documentation formatting separate from code tests and lint", () => {
    const plan = createCiPlan(["ai-docs/standards/testing.md"], packages, root, knownTests);

    expect(plan.vitestFiles).toEqual([]);
    expect(plan.rustTestPackages).toEqual([]);
    expect(plan.dprintFormatFiles).toEqual(["ai-docs/standards/testing.md"]);
    expect(plan.biomeLintFiles).toEqual([]);
  });

  it("does not pass deleted or tool-excluded files to format and lint commands", () => {
    const paths = [
      "src-web/components/Removed.vue",
      "resources/scripts/mistbell/scenario.md",
      "resources/scripts/mistbell/entities.json",
      "public/theme-bootstrap.js",
    ];
    const plan = createCiPlan(paths, packages, root, knownTests, [
      "resources/scripts/mistbell/entities.json",
      "public/theme-bootstrap.js",
    ]);

    expect(plan.dprintFormatFiles).toEqual([]);
    expect(plan.biomeFormatFiles).toEqual([]);
    expect(plan.biomeLintFiles).toEqual([]);
  });

  it("selects matching script tests and Rust consumers for shared compiled inputs", () => {
    const scope = createCiPlan(["scripts/ci-scope.mts"], packages, root, knownTests);
    expect(scope.vitestFiles).toEqual(["tests/scripts/ci-scope.test.ts"]);

    const tauriBuild = createCiPlan(["scripts/rust/tauri-build.rs"], packages, root, knownTests);
    expect(tauriBuild.rustTestPackages).toEqual(["aoidos", "aoidos-native-tests"]);

    const locale = createCiPlan(["locales/en.json"], packages, root, knownTests);
    expect(locale.rustTestPackages).toEqual(["aoidos", "aoidos-native-tests"]);
    expect(locale.runI18nCheck).toBe(true);
  });

  it("does not run unrelated Rust or Vitest suites for workflow-only changes", () => {
    const plan = createCiPlan([".github/workflows/nightly.yml"], packages, root, knownTests);
    expect(plan.full).toBe(false);
    expect(plan.rustTestPackages).toEqual([]);
    expect(plan.vitestFiles).toEqual([]);
    expect(plan.dprintFormatFiles).toEqual([".github/workflows/nightly.yml"]);
    expect(plan.runNativeTests).toBe(false);
  });

  it("runs native smoke tests for WebView source files imported by the harness", () => {
    expect(
      createCiPlan(["src-web/composables/useGameView.ts"], packages, root, knownTests)
        .runNativeTests,
    ).toBe(true);
    expect(
      createCiPlan(["src-web/composables/useBilling.ts"], packages, root, knownTests)
        .runNativeTests,
    ).toBe(false);
    expect(
      createCiPlan(["src-web/utils/listener-group.ts"], packages, root, knownTests).runNativeTests,
    ).toBe(true);
  });

  it("selects native integration tests only for their runtime inputs", () => {
    for (const path of [
      "src-tauri/src/lib.rs",
      "tests/rust/native-platform/webview/main.ts",
      "src-web/api/game.ts",
      "src-web/composables/useGameView.ts",
      "src-web/theme-bootstrap.ts",
      "locales/en.json",
      "public/icon.png",
    ]) {
      expect(createCiPlan([path], packages, root, knownTests).runNativeTests).toBe(true);
    }
  });

  it("runs the locale checker for ordinary Vue and TypeScript source", () => {
    for (const path of ["src-web/App.vue", "src-web/composables/useBilling.ts"]) {
      expect(createCiPlan([path], packages, root, knownTests).runI18nCheck).toBe(true);
    }
  });

  it("runs all scopes for an unknown path or an empty diff", () => {
    for (const paths of [[], ["new-domain/input.dat"]]) {
      const plan = createCiPlan(paths, packages, root, knownTests);
      expect(plan.full).toBe(true);
      expect(plan.vitestFiles).toBe("all");
      expect(plan.biomeLintFiles).toBe("all");
    }
  });
});
