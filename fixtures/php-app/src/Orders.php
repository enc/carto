<?php

namespace App\Orders;

function parseOrder($input)
{
    validate($input);
    return new Order($input);
}

function validate($input)
{
    return $input !== '';
}

class Order
{
    public function __construct($id)
    {
        $this->id = $id;
    }

    public function summary()
    {
        return "order #{$this->id}";
    }

    public static function fromArray(array $data)
    {
        return new self($data['id']);
    }

    private function refresh()
    {
        $this->id = null;
    }
}
