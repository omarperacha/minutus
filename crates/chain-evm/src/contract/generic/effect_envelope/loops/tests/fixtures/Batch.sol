// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;
contract Batch {
    uint256 public number;
    mapping(uint256=>uint256) public entries;
    int8 public signedNumber;
    bool public enabled;
    function batch(uint256[] calldata keys,uint256[] calldata items) external {
        require(keys.length==items.length);
        for(uint256 i;i<keys.length;i++) entries[keys[i]]=items[i];
    }
    function range(uint256 start,uint256 count,uint256 item) external {
        for(uint256 i=0;i<count;++i) entries[start+i]=item;
    }
    function bodyUpdate(uint256 count) external {
        uint256 i;
        for(;i<count;) { entries[i]=count; i+=1; }
    }
    function localCopy(uint256 value) external { value+=1;number=value; }
    function signedCopy(int8 value) external { value+=1;signedNumber=value; }
    function boolCopy(bool value) external { value=!value;enabled=value; }
    function arrayCopy(uint256[] memory original,uint256[] memory replacement) public { original=replacement;number=original.length; }
    function stateLoop(uint256 item) external { for(uint256 i;i<number;i++) entries[i]=item; }
    function neverTerminates(uint256 value) external { uint256 i;for(;i<100;) { i=i; }number=value; }
    function breakLoop(uint256 count) external { for(uint256 i;i<count;i++){if(i==1)break;entries[i]=count;} }
    function uncheckedLocal(uint256 value) external { unchecked { value++; }number=value; }
}
