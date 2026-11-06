package com.acme.scopes;

import java.util.List;
import java.io.Reader;

class Locals {
    int count;

    int sum(List<Integer> values, int start) {
        int total = start;
        for (int value : values) {
            total += value;
        }
        for (int i = 0; i < total; i++) {
            int count = i;
            total = total - count;
        }
        {
            int total2 = total;
            total = total2;
        }
        return total + count;
    }

    void shadow() {
        int a = 1;
        {
            int b = a;
        }
        int b = 2;
        Runnable r = () -> {
            int c = a + b;
        };
        java.util.function.Function<Integer, Integer> f = x -> x + b;
    }

    void resources(Reader in) throws Exception {
        try (Reader reader = in) {
            reader.read();
        } catch (RuntimeException e) {
            throw e;
        }
        Object o = in;
        if (o instanceof Reader r) {
            r.close();
        }
    }

    int later() {
        return early;
    }

    int early = 1;
}
