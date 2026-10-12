import { satisfies } from "semver";

export function supported(version: string): boolean {
    return satisfies(version, ">=7.0.0");
}

export function run(): boolean {
    return supported("7.8.5");
}
