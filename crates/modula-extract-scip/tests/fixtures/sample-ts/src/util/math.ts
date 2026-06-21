function double(n: number): number {
  return n + n;
}

export function add(a: number, b: number): number {
  return double(a) + b;
}

export interface Calc {
  run(): number;
}
