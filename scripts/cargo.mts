import { runCargo } from "./rust-toolchain";

runCargo(Bun.argv.slice(2));
