<?php

namespace App\Handlers;

use App\Orders\Order;
use function App\Orders\parseOrder;
use Psr\Log\LoggerInterface;
use App\Missing\Thing;

class Handler
{
    public function handle($input)
    {
        $order = parseOrder($input);
        auditOrder($order->id);
        $order->summary();
        Order::fromArray([$order->id]);
        $order->refresh();
        unknownExternalCall();
        $input->strip();
    }
}
