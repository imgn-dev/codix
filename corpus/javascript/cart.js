#!/usr/bin/env node
export const VERSION = "1.0";
const MAX = 10;

class Cart {
  static count = 0;
  #secret = 1;

  constructor(items) {
    this.items = items;
  }

  total() {
    return this.items.reduce((a, b) => a + b, 0);
  }

  get size() {
    return this.items.length;
  }
}

function* ids() {
  yield 1;
}

export function checkout(cart) {
  return cart.total();
}

const discount = (price) => price * 0.9;

module.exports = {
  format(value) {
    return String(value);
  },
  round: function (value) {
    return Math.round(value);
  },
};
