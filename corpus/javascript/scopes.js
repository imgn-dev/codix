import React, { useState as useLocal, useEffect } from "react";
import * as path from "path";

const LIMIT = 10;

function total(items, start = 0, ...rest) {
  var sum = start;
  for (let i = 0; i < items.length; i++) {
    let item = items[i];
    sum += item.price;
  }
  if (sum > LIMIT) {
    var capped = LIMIT;
  }
  return capped ?? sum + rest.length;
}

const double = (x) => x * 2;
const square = x => x * x;

class Cart {
  count = 0;

  constructor(items) {
    this.items = items;
  }

  total() {
    return total(this.items) + this.count;
  }

  add(item) {
    const { price, name: label } = item;
    const [first, second = 1] = [price, label];
    this.items.push(item);
    return this.total() + first + second;
  }
}

const api = {
  fetch: function () {
    return path.join("a", "b");
  },
  helper() {
    return fetch;
  },
};

function hoisted() {
  return later() + useLocal(0) + useEffect;
}

function later() {
  try {
    return double(square(2));
  } catch (error) {
    return error;
  }
}

function View() {
  return <div><Cart /></div>;
}

export { total as sum, Cart };
