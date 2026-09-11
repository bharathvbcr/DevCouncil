// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "./Helper.sol";

contract Widget is BaseWidget, IRenderable {
    string public name;

    function render() public view returns (string memory) {
        return Helper.help(name);
    }

    function run() public {
        render();
    }
}
