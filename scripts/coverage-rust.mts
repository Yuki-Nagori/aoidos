// coverage:rust 门禁执行器：跑 llvm-cov（JSON 导出），按根目录
// coverage-rust.config.mts 的逐文件预算裁决。门槛设计与函数实例统计注意事项见该配置。
import { exit } from "node:process";
import config from "../coverage-rust.config.mts";

const reportPath = "target/coverage-rust.json";
const ignoreRegex = config.ignore.map((pattern) => pattern.source).join("|");

const run = Bun.spawnSync(
  [
    "cargo",
    "llvm-cov",
    "--workspace",
    "--lib",
    "--json",
    `--ignore-filename-regex=${ignoreRegex}`,
    `--output-path=${reportPath}`,
  ],
  { stdout: "inherit", stderr: "inherit" },
);
if (!run.success) {
  exit(run.exitCode ?? 1);
}

const report = JSON.parse(await Bun.file(reportPath).text());
let violations = 0;
let totalUncovered = 0;
for (const file of report.data[0].files) {
  const path = file.filename.replaceAll("\\", "/");
  if (config.ignore.some((pattern) => pattern.test(path))) {
    continue;
  }
  const uncovered = file.summary.lines.count - file.summary.lines.covered;
  totalUncovered += uncovered;
  if (uncovered === 0) {
    continue;
  }
  const allowance = config.allowances.find((entry) => path.endsWith(`/${entry.file}`));
  const budget = allowance?.maxUncoveredLines ?? config.defaultMaxUncoveredLines;
  if (uncovered > budget) {
    violations += 1;
    const registered = allowance ? `预算 ${budget} 行` : "未登记预算（缺省必须 100%）";
    console.error(
      `${path} 未覆盖 ${uncovered} 行，超过${registered}；` +
        `未覆盖行号：${uncoveredLines(file).join(", ")}`,
    );
  }
}

/** 从 segments 还原未覆盖行号：行内所有起始段计数为 0 即未覆盖。 */
function uncoveredLines(file: {
  segments: [number, number, number, boolean, ...unknown[]][];
}): number[] {
  const starts = new Map<number, number[]>();
  for (const [line, , count, hasCount] of file.segments) {
    if (!hasCount) {
      continue;
    }
    starts.set(line, [...(starts.get(line) ?? []), count]);
  }
  return [...starts.entries()]
    .filter(([, counts]) => Math.max(...counts) === 0)
    .map(([line]) => line)
    .sort((a, b) => a - b);
}

console.log(
  `workspace lib 目标 ${report.data[0].files.length} 个文件，未覆盖 ${totalUncovered} 行。`,
);
if (violations > 0) {
  console.error(`${violations} 个文件超出 coverage-rust.config.mts 的预算。`);
  exit(1);
}
