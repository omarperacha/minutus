// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// Exact direct-code profile. No runtime mint, burn, owner, hook or external call.
contract MinutusQualifiedERC20 is ERC20 {
    constructor(string memory name_, string memory symbol_, address holder_, uint256 supply_)
        ERC20(name_, symbol_)
    {
        _mint(holder_, supply_);
    }
}
