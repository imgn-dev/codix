package com.acme.scopes;

class Members {
    private long total;
    private int count;

    long total() {
        return total;
    }

    long total(long discount) {
        return total() - discount;
    }

    void update(Members other, long total) {
        this.total = total;
        count = other.count + 1;
        other.total();
        this.total();
        Math.max(total, count);
        String text = toString();
        Members copy = new Members();
    }

    static class Nested {
        int count() {
            return new Members().count;
        }

        long outer() {
            return total();
        }
    }

    static class Child extends Members {
        long sum() {
            return total() + helper();
        }

        Members self() {
            return this;
        }
    }

    Runnable anonymous() {
        return new Runnable() {
            public void run() {
                total();
            }
        };
    }

    int helper() {
        return 0;
    }

    enum Color { RED, GREEN }

    int pick(Color color) {
        switch (color) {
            case RED:
                return 1;
            default:
                return count;
        }
    }

    <T extends Comparable<T>> T max(T a, T b) {
        return a.compareTo(b) > 0 ? a : b;
    }
}
