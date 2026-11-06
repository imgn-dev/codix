#include <vector>

namespace geo {

template <typename T>
class Shape {
public:
    explicit Shape(T scale) : scale_(scale) {}
    T area() const { return scale_ * factor(); }
    T factor() const;

private:
    T scale_;
};

template <typename T>
T Shape<T>::factor() const {
    return scale_ + 1;
}

class Square : public Shape<int> {
public:
    int side() const { return area(); }
};

int total(const std::vector<int>& values) {
    int sum = 0;
    for (const int& value : values) {
        sum += value;
    }
    auto twice = [sum](int x) { return x * 2 + sum; };
    return twice(sum);
}

}  // namespace geo
