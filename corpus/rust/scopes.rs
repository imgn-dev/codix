use std::collections::HashMap;
use std::fmt::{self, Display as Show};

const LIMIT: usize = 10;

pub struct Cart<T> {
    items: Vec<T>,
    count: usize,
}

impl<T: Clone> Cart<T> {
    pub fn new() -> Self {
        Self::with_capacity(LIMIT)
    }

    fn with_capacity(size: usize) -> Self {
        Cart { items: Vec::with_capacity(size), count: 0 }
    }

    pub fn add(&mut self, item: T) -> usize {
        self.items.push(item.clone());
        self.count += 1;
        let count = self.count;
        let count = count * 2;
        helper(count)
    }
}

fn helper(value: usize) -> usize {
    let doubled = |x: usize| x * 2;
    let mut total = later(value);
    for (index, item) in [1, 2].iter().enumerate() {
        total += index + item;
    }
    if let Some(found) = Some(total) {
        total = found;
    }
    match total {
        0 => doubled(total),
        n if n > LIMIT => n,
        other => other + value,
    }
}

fn later(x: usize) -> usize {
    println!("{}", x);
    x
}

mod nested {
    pub fn inner() -> usize {
        helper(1)
    }
}

fn shapes(map: HashMap<String, usize>) -> Option<usize> {
    let Point { x, y: height } = Point { x: 1, y: 2 };
    map.get("a").copied().map(|v| v + x + height)
}

struct Point {
    x: usize,
    y: usize,
}
