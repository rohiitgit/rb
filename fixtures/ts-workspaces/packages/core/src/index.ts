import { Base } from "./shapes";

export { Base, Mid, Deep, Other, Named } from "./shapes";

export function run(b: Base): void {
  b.m();
}
