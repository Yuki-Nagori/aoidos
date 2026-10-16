import { execFileSync } from "node:child_process";
import type { WorkspacePackage } from "./rust-layers";
import { checkWorkspaceLayers } from "./rust-layers";

type CargoMetadata = { workspace_members: string[]; packages: WorkspacePackage[] };

const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--locked", "--no-deps", "--format-version", "1"], {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  }),
) as CargoMetadata;
const workspaceIds = new Set(metadata.workspace_members);
const workspacePackages = metadata.packages.filter(({ id }) => workspaceIds.has(id));
const errors = checkWorkspaceLayers(
  workspacePackages.map(({ name }) => name),
  workspacePackages,
);

if (errors.length > 0) {
  console.error("Rust workspace layering check failed:");
  for (const error of errors) console.error(`- ${error}`);
  process.exitCode = 1;
} else {
  console.log(`Rust workspace layering check passed (${workspacePackages.length} crates).`);
}
