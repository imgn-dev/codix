package shapes

import "math"

const Pi, Tau = 3.14, 6.28

var registry = map[string]Shape{}

type Shape interface {
	Area() float64
	Name() string
}

type Point struct {
	X, Y float64
	label string
}

type Celsius float64

type Alias = Point

type Box[T any] struct{ items []T }

func NewPoint(x, y float64) *Point {
	const local = 1
	return &Point{X: x, Y: y}
}

func (p *Point) Move(dx float64) {
	p.X += dx
}

func (p Point) Area() float64 { return 0 }

func (b *Box[T]) Len() int { return len(b.items) }

func (c Celsius) String() string { return "" }

func distance(a, b Point) float64 {
	return math.Hypot(a.X-b.X, a.Y-b.Y)
}
