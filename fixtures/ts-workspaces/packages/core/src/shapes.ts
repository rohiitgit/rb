export interface Base {
  m(): void;
  n(x: number): number;
}

export interface Mid extends Base {
  m(): void;
  extra(): void;
}

export interface Deep extends Mid {
  m(): void;
}

export interface Other extends Mid, Base {
  n(x: number): number;
}

export interface Named {
  m: () => void;
}
