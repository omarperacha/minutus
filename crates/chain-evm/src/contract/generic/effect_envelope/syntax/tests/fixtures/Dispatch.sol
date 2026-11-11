// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;
contract Foreign { function step(uint256) external {} }
contract A {
    uint256 public number;
    uint256 public hidden;
    modifier gate() virtual { if (number == 1) hidden = 9; _; }
    function step(uint256 x) internal virtual { number = x; }
    function relay(uint256 x) internal { step(x); }
    function privateStep(uint256 x) private { number = x; }
    function publicStep(uint256 x) public virtual { number = x; }
    function inherited(uint256 x) external gate { step(x); }
    function inheritedPrivate(uint256 x) external { privateStep(x); }
    function inheritedPublic(uint256 x) external { publicStep(x); }
}
contract B is A {
    function step(uint256 x) internal virtual override { super.step(x + 1); }
}
contract C is A {
    function step(uint256 x) internal virtual override { super.step(x + 2); }
}
contract D is B, C {
    Foreign public other;
    modifier gate() override { require(number < 50); _; }
    function step(uint256 x) internal override(B,C) { super.step(x + 3); }
    function publicStep(uint256 x) public override { number = x + 5; }
    function entry(uint256 x) external { step(x); }
    function viaBase(uint256 x) external { A.step(x); }
    function viaNested(uint256 x) external { A.relay(x); }
    function viaPublic(uint256 x) external { publicStep(x); }
    function shadowedField(uint256 x) external { other.step(x); }
    function viaThis(uint256 x) external { this.publicStep(x); }
    function viaExplicitModifier(uint256 x) external A.gate { step(x); }
    function pointer(function(uint256) internal target, uint256 x) internal { target(x); }
}
