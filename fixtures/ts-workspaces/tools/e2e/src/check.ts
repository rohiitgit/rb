import { Base, run } from "@ws/core";

const probe: Base = {
  m(): void {},
  n(x: number): number {
    return x;
  },
};

export function check(): void {
  run(probe);
}
