#include <stdio.h>

#define MAX(a, b) ((a) > (b) ? (a) : (b))

struct point {
    int x;
    int y;
};

typedef struct point point_t;

static int counter = 0;

int sum(const int *values, int count);

int sum(const int *values, int count) {
    int total = 0;
    for (int i = 0; i < count; i++) {
        int value = values[i];
        total += value;
    }
    {
        int total = -1;
        counter = total;
    }
    return MAX(total, counter);
}

int area(struct point *p, point_t q) {
    p->x = q.y;
    counter++;
    return p->x * p->y + sum(&q.x, 1);
}
