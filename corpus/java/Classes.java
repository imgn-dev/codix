package com.acme.billing;

import java.util.List;

public class Invoice {
    private final List<Line> lines;
    static int count = 0, total;

    public Invoice(List<Line> lines) {
        this.lines = lines;
    }

    public long total() {
        return lines.stream().mapToLong(Line::amount).sum();
    }

    long total(long discount) {
        return total() - discount;
    }

    protected static class Line {
        private long amount;

        long amount() {
            return amount;
        }
    }

    enum Status {
        DRAFT, SENT("sent") {
            @Override
            String label() { return "Sent"; }
        };

        Status() {}
        Status(String code) {}
        String label() { return name(); }
    }
}
