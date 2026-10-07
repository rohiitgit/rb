import { Mid, run } from "@ws/core";

export class Svc implements Mid {
  m(): void {}
  extra(): void {}
  n(x: number): number {
    return x;
  }
}

export function main(): void {
  run(new Svc());
}
