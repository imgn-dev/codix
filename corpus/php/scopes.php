<?php

namespace Acme\Billing;

use Acme\Money\Amount;
use Acme\Money\Currency as Cur;

const LIMIT = 10;

function total(array $items, int $start = 0): int
{
    $sum = $start;
    foreach ($items as $key => $item) {
        $sum += $item;
    }
    $sum = $sum + LIMIT;
    return helper($sum);
}

function helper(int $value): int
{
    global $counter;
    $counter++;
    $double = function ($x) use ($value) {
        return $x * 2 + $value;
    };
    $triple = fn ($x) => $x * 3 + $value;
    return $double($value) + $triple($value);
}

class Cart extends Base
{
    private int $count = 0;

    public function add(Amount $amount, Cur $currency): int
    {
        $this->count++;
        $this->count = $this->count + 1;
        self::reset();
        return total([$amount->value], start: $this->count);
    }

    public static function reset(): void
    {
    }
}
