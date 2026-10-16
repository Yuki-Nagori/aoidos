import { describe, expect, it } from "vitest";
import type { WorkspacePackage } from "../../scripts/rust-layers";
import { checkWorkspaceLayers } from "../../scripts/rust-layers";

const packages: WorkspacePackage[] = [
  { id: "store", name: "aoidos-store", dependencies: [] },
  {
    id: "engine",
    name: "aoidos-engine",
    dependencies: [{ name: "aoidos-store" }, { name: "serde" }],
  },
  { id: "app", name: "aoidos", dependencies: [{ name: "aoidos-engine" }] },
  {
    id: "native-tests",
    name: "aoidos-native-tests",
    dependencies: [{ name: "aoidos" }],
  },
];

describe("Rust workspace layering", () => {
  it("accepts registered dependency direction and ignores external crates", () => {
    expect(
      checkWorkspaceLayers(
        packages.map(({ name }) => name),
        packages,
      ),
    ).toEqual([]);
  });

  it("rejects a dependency from a lower layer back into the engine", () => {
    const invalid = packages.map((pkg) =>
      pkg.name === "aoidos-store" ? { ...pkg, dependencies: [{ name: "aoidos-engine" }] } : pkg,
    );

    expect(
      checkWorkspaceLayers(
        invalid.map(({ name }) => name),
        invalid,
      ),
    ).toContain("forbidden workspace dependency: aoidos-store -> aoidos-engine");
  });

  it("requires every new workspace crate to declare its layer", () => {
    const extended = [...packages, { id: "new", name: "aoidos-new", dependencies: [] }];

    expect(
      checkWorkspaceLayers(
        extended.map(({ name }) => name),
        extended,
      ),
    ).toContain("unregistered workspace package: aoidos-new");
  });
});
