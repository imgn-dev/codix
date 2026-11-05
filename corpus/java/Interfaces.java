public interface Repository<T> {
    int PAGE_SIZE = 50;

    T find(String id);

    default List<T> all() {
        return List.of();
    }
}

@interface Audited {
    String value() default "";
}

record Point(int x, int y) {
    Point {
        if (x < 0) throw new IllegalArgumentException();
    }

    static Point origin() {
        return new Point(0, 0);
    }
}
