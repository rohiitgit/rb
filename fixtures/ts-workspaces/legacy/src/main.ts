import { helper } from "@models/user";
import { helper as other } from "@other/user";

export function run(): string {
  return helper() + other();
}
