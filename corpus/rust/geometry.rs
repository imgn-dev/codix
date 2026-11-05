//! Geometry.

pub const ORIGIN: Point = Point { x: 0.0, y: 0.0 };
static mut COUNT: u32 = 0;

pub type Coord = f64;

#[derive(Debug)]
pub struct Point {
    pub x: Coord,
    y: Coord,
}

pub(crate) enum Shape {
    Circle { radius: f64 },
    Square(f64),
    Empty,
}

pub trait Area {
    type Output;
    fn area(&self) -> f64;
    fn describe(&self) -> String {
        String::new()
    }
}

impl Point {
    pub const UNIT: f64 = 1.0;

    pub fn new(x: Coord, y: Coord) -> Self {
        Self { x, y }
    }
}

impl Area for Shape {
    type Output = f64;
    fn area(&self) -> f64 {
        0.0
    }
}

impl<T> Wrapper<T> {
    fn get(&self) -> &T {
        &self.0
    }
}

pub mod units {
    pub fn meters(value: f64) -> f64 {
        value
    }

    macro_rules! square {
        ($x:expr) => {
            $x * $x
        };
    }
}

pub union Bits {
    int: u32,
    float: f32,
}

fn distance(a: &Point, b: &Point) -> f64 {
    ((a.x - b.x).powi(2)).sqrt()
}
