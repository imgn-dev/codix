#!/usr/bin/env python3
"""Billing."""

MAX_LINES = 100
default_currency = "EUR"


class Invoice:
    count: int = 0
    tax_rate = 0.2

    def __init__(self, lines):
        self.lines = lines

    @property
    def total(self) -> int:
        def add(a, b):
            return a + b
        return sum(self.lines)

    @staticmethod
    def empty() -> "Invoice":
        return Invoice([])

    class Line:
        def amount(self):
            return 0


def build_report(invoice: Invoice, *, verbose=False) -> str:
    return str(invoice.total)


async def fetch(url):
    pass
