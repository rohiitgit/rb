export function old(): number {
  return 1;
}

export function caller(): number {
  return old();
}
