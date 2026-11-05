#include <stdlib.h>

#define MAX_ITEMS 64
#define SQUARE(x) ((x) * (x))

static int count = 0;
const char *names[MAX_ITEMS];

struct node {
    int value;
    struct node *next;
    char label[16];
};

typedef struct {
    double x, y;
} point;

typedef struct node *node_ptr;

enum color { RED, GREEN = 2 };

union bits { int i; float f; };

int length(const struct node *head);

static struct node *push(struct node *head, int value) {
    struct node *n = malloc(sizeof *n);
    n->value = value;
    n->next = head;
    return n;
}

int length(const struct node *head) {
    int n = 0;
    for (; head; head = head->next) n++;
    return n;
}
