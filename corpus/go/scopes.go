package geo

import (
	"fmt"
	str "strings"
)

type Point struct {
	X, Y int
}

var origin = Point{X: 0, Y: 0}

func (p Point) Add(q Point) Point {
	return Point{X: p.X + q.X, Y: p.Y + q.Y}
}

func sum(values []int) (total int) {
	for _, v := range values {
		total += v
	}
	x := 1
	if x := x + 1; x > 2 {
		total = x
	}
	y, x := 2, 3
	total = total + x + y
	return
}

func closures() func() int {
	count := 0
	inc := func() int {
		count++
		return count
	}
	return inc
}

func kinds(v interface{}) string {
	switch t := v.(type) {
	case int:
		return fmt.Sprint(t)
	default:
		return str.ToUpper(fmt.Sprint(t))
	}
}

func maps(key string) map[string]int {
	m := map[string]int{key: 1}
	n := Labels{key: 2}
	return m
}

type Labels map[string]int

func generic[T any](a T) T {
	var b T = a
	return b
}

func later() Point {
	return origin.Add(origin)
}
