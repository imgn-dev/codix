export const VERSION = "1.0";
const handlers = new Map<string, Handler>();
let mutable = 1;

export interface Shape {
  readonly name: string;
  area(): number;
}

type Handler = (event: string) => void;

enum Color {
  Red,
  Green = "green",
}

namespace Geometry {
  export function origin(): Point {
    return new Point(0, 0);
  }
}

export abstract class Base implements Shape {
  abstract area(): number;
  get name(): string { return "base"; }
}

export class Point extends Base {
  private static count = 0;
  public readonly x: number;

  constructor(x: number, protected y: number) {
    super();
    this.x = x;
  }

  area(): number {
    return 0;
  }

  move = (dx: number) => {
    this.x += dx;
  };
}

export function distance(a: Point, b: Point): number {
  return Math.hypot(a.x - b.x, 0);
}

const add = (a: number, b: number) => a + b;

declare function external(x: number): string;
