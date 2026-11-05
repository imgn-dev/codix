#include <vector>

#define PI 3.14159

namespace geo {

struct Point {
    double x, y;
};

class Shape {
public:
    Shape();
    virtual ~Shape() = default;
    virtual double area() const = 0;
    bool operator==(const Shape &other) const;

protected:
    int id_;
};

class Circle : public Shape {
public:
    explicit Circle(double r) : r_(r) {}
    double area() const override { return PI * r_ * r_; }

private:
    double r_ = 1.0;
};

using Shapes = std::vector<Shape *>;

enum class Kind { Circle, Square };

template <typename T>
class Box {
public:
    T get() const;
};

double total(const Shapes &shapes);

}  // namespace geo

geo::Shape::Shape() : id_(0) {}

bool geo::Shape::operator==(const Shape &other) const { return id_ == other.id_; }

template <typename T>
T geo::Box<T>::get() const { return T(); }

double geo::total(const Shapes &shapes) {
    double sum = 0;
    for (auto *s : shapes) sum += s->area();
    return sum;
}
