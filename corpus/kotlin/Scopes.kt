package com.acme.scopes

import java.util.UUID
import kotlin.math.max as maximum

const val LIMIT = 10

class Cart<T : Comparable<T>>(private val owner: String, size: Int) {
    private val items = ArrayList<T>(size)
    var count = 0

    fun add(item: T): Int {
        items.add(item)
        count = count + 1
        this.count++
        return total(count)
    }

    fun total(extra: Int = 0): Int {
        var sum = extra
        for ((index, value) in items.withIndex()) {
            sum += index
        }
        val best = items.maxOrNull()
        return maximum(sum, LIMIT) + owner.length
    }

    fun labels(): List<String> = items.map { it.toString() + owner }

    fun apply(block: Cart<T>.() -> Unit) = block(this)

    fun describe(): String {
        val id = UUID.randomUUID()
        return run { "$id ${count}" } + toString()
    }
}

class Special(owner: String) : Base(owner) {
    fun shout() = name() + LIMIT
}

fun build(): Cart<String> {
    val cart = Cart<String>(owner = "me", size = 2)
    try {
        cart.add("x")
    } catch (error: IllegalStateException) {
        throw error
    }
    return cart
}
