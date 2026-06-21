import { add } from "./util/math";

export function greet(name: string): string {
  return "hi " + name + add(1, 2);
}

class Internal {}
