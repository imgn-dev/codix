import { Shape, area as shapeArea } from "./shapes";
import type { Options } from "./options";

interface Priced {
  price: number;
  label?: string;
}

type Pair<T> = [T, T];

enum Color {
  Red,
  Green,
}

export class Cart<T extends Priced> implements Iterable<T> {
  private items: T[] = [];

  constructor(private readonly options: Options) {}

  add(item: T, color: Color = Color.Red): Pair<number> {
    this.items.push(item);
    const total: number = this.total();
    return [total, item.price];
  }

  total(): number {
    let sum = 0;
    for (const item of this.items) {
      sum += item.price;
    }
    return sum;
  }

  [Symbol.iterator](): Iterator<T> {
    return this.items[Symbol.iterator]();
  }
}

namespace Geometry {
  export function scale(shape: Shape, by: number): number {
    return shapeArea(shape) * by;
  }
}

function first<U>(values: U[], fallback?: U): U | undefined {
  return values[0] ?? fallback;
}

const scaled = Geometry.scale;
