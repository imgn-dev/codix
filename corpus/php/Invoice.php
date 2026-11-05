<?php

namespace Acme\Billing;

const VERSION = "1.0";

interface Priced
{
    public function price(): int;
}

trait Logs
{
    protected function log(string $message): void {}
}

enum Status: string
{
    case Draft = 'draft';
    case Sent = 'sent';
}

final class Invoice implements Priced
{
    use Logs;

    public const MAX_LINES = 100;
    private array $lines = [];
    public static int $count = 0;

    public function __construct(array $lines)
    {
        $this->lines = $lines;
    }

    public function price(): int
    {
        return array_sum($this->lines);
    }

    private static function empty(): self
    {
        return new self([]);
    }
}

function report(Invoice $invoice): string
{
    return (string) $invoice->price();
}
