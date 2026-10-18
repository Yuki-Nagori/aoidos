import { describe, expect, it } from "vitest";
import type { CargoPackage } from "../../scripts/rust-test-selector";
import { selectRustTestPlan } from "../../scripts/rust-test-selector";

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
    dependencies: [{ path: `${root}/src-rust/aoidos-engine` }],
  },
  {
    name: "aoidos-theme",
    manifest_path: `${root}/src-rust/aoidos-theme/Cargo.toml`,
    dependencies: [],
  },
  {
    name: "aoidos-script",
    manifest_path: `${root}/src-rust/aoidos-script/Cargo.toml`,
    dependencies: [],
  },
];

describe("Rust test scope selection", () => {
  it("tests a changed crate and every transitive workspace consumer", () => {
    const plan = selectRustTestPlan(["src-rust/aoidos-store/src/lib.rs"], packages, root);

    expect(plan).toEqual({
      full: false,
      packages: ["aoidos", "aoidos-engine", "aoidos-native-tests", "aoidos-store"],
      reason: "affected packages and workspace dependents",
    });
  });

  it("leaves unrelated Rust packages out of a narrow change", () => {
    const plan = selectRustTestPlan(["src-rust/aoidos-theme/src/lib.rs"], packages, root);

    expect(plan.packages).toEqual(["aoidos-theme"]);
    expect(plan.full).toBe(false);
  });

  it("maps embedded scenario files to their Rust consumers", () => {
    const plan = selectRustTestPlan(["resources/scripts/mistbell/Mistbell.md"], packages, root);

    expect(plan.packages).toEqual([
      "aoidos",
      "aoidos-engine",
      "aoidos-native-tests",
      "aoidos-script",
      "aoidos-theme",
    ]);
    expect(plan.full).toBe(false);
  });

  it("maps frontend assets asserted by the theme crate", () => {
    const plan = selectRustTestPlan(["src-web/app.css"], packages, root);

    expect(plan.packages).toEqual(["aoidos-theme"]);
    expect(plan.full).toBe(false);
  });

  it("skips Rust tests for known web-only and documentation changes", () => {
    const plan = selectRustTestPlan(
      ["src-web/app/App.vue", "ai-docs/architecture/README.md"],
      packages,
      root,
    );

    expect(plan).toEqual({ full: false, packages: [], reason: "no Rust package affected" });
  });

  it.each([
    [[], "empty change set"],
    [["Cargo.lock"], "shared test configuration changed: Cargo.lock"],
    [["new-area/unknown.txt"], "unmapped path: new-area/unknown.txt"],
    [["../outside.rs"], "invalid path: ../outside.rs"],
  ])("falls back to the full workspace for uncertain input %j", (paths, reason) => {
    const plan = selectRustTestPlan(paths as string[], packages, root);

    expect(plan.full).toBe(true);
    expect(plan.packages).toEqual(packages.map(({ name }) => name).sort());
    expect(plan.reason).toBe(reason);
  });

  it("does not run Rust tests for workflow-only changes", () => {
    const plan = selectRustTestPlan([".github/workflows/ci.yml"], packages, root);

    expect(plan).toEqual({ full: false, packages: [], reason: "no Rust package affected" });
  });
});
