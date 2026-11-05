package com.acme.billing

const val MAX_LINES = 100
val defaultCurrency = "EUR"

typealias Amount = Long

interface Priced {
    fun price(): Amount
}

enum class Status {
    DRAFT, SENT;

    fun label() = name.lowercase()
}

data class Line(val label: String, private val amount: Amount = 0) : Priced {
    override fun price() = amount
}

class Invoice(val lines: List<Line>) {
    internal var paid = false

    constructor() : this(emptyList())

    fun total(): Amount {
        return lines.sumOf { it.price() }
    }

    companion object Factory {
        fun empty() = Invoice()
    }
}

object Registry {
    private val invoices = mutableListOf<Invoice>()
}

fun report(invoice: Invoice, verbose: Boolean = false): String = invoice.total().toString()
