<?php
namespace Probe;

use Example\Helper;

class Widget extends BaseWidget implements Renderable
{
    private $name;

    public function render()
    {
        return Helper::help($this->name);
    }
}

function main()
{
    $w = new Widget();
    $w->render();
}
