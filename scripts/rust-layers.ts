export type WorkspacePackage = {
  id: string;
  name: string;
  dependencies: Array<{ name: string }>;
};

const ALLOWED_DEPENDENCIES: Record<string, readonly string[]> = {
  "aoidos-store": [],
  "aoidos-json": [],
  "aoidos-script": [],
  "aoidos-billing": [],
  "aoidos-theme": ["aoidos-store"],
  "aoidos-locale": ["aoidos-store"],
  "aoidos-memory": ["aoidos-json", "aoidos-store"],
  "aoidos-llm": ["aoidos-json", "aoidos-store"],
  "aoidos-engine": [
    "aoidos-billing",
    "aoidos-json",
    "aoidos-llm",
    "aoidos-locale",
    "aoidos-memory",
    "aoidos-script",
    "aoidos-store",
    "aoidos-theme",
  ],
  "aoidos-app": ["aoidos-engine", "aoidos-llm", "aoidos-store"],
  aoidos: [
    "aoidos-billing",
    "aoidos-app",
    "aoidos-engine",
    "aoidos-json",
    "aoidos-llm",
    "aoidos-locale",
    "aoidos-store",
    "aoidos-theme",
  ],
  "aoidos-native-tests": ["aoidos", "aoidos-engine", "aoidos-llm", "aoidos-locale", "aoidos-store"],
};

export function checkWorkspaceLayers(
  workspaceNames: string[],
  packages: WorkspacePackage[],
): string[] {
  const errors: string[] = [];
  const workspace = new Set(workspaceNames);
  const packageNames = new Set(packages.map(({ name }) => name));

  for (const name of workspace) {
    if (!packageNames.has(name)) errors.push(`workspace package missing from metadata: ${name}`);
    if (!Object.hasOwn(ALLOWED_DEPENDENCIES, name))
      errors.push(`unregistered workspace package: ${name}`);
  }

  for (const pkg of packages) {
    if (!workspace.has(pkg.name)) continue;
    const allowed = ALLOWED_DEPENDENCIES[pkg.name];
    if (!allowed) continue;
    for (const dependency of pkg.dependencies) {
      if (!workspace.has(dependency.name)) continue;
      if (!allowed.includes(dependency.name)) {
        errors.push(`forbidden workspace dependency: ${pkg.name} -> ${dependency.name}`);
      }
    }
  }

  return [...new Set(errors)].sort();
}
