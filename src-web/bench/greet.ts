// tinybench 基准示例：bun run bench:web
import { Bench } from "tinybench";
import { formatGreeting } from "../utils/greet";

const bench = new Bench({ warmupIterations: 100, iterations: 1_000 });
bench.add("formatGreeting", () => formatGreeting("aoidos"));

await bench.run();
console.table(bench.table());

// 门禁断言（verify 的 bench 项）：每个任务到达 completed 状态，否则非零退出；
// 基准数值本身不做阈值判定。tinybench 6 以 state（completed / errored）表达
// 成败，error 只存在于 errored 变体。
const failed = bench.tasks.filter((task) => !task.result || task.result.state !== "completed");
if (bench.tasks.length === 0 || failed.length > 0) {
  console.error(`bench 产物校验失败：${failed.length} 个任务异常 / 共 ${bench.tasks.length} 个`);
  process.exit(1);
}
