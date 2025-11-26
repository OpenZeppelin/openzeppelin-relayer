// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Script, console} from "forge-std/Script.sol";
import {Counter} from "../src/Counter.sol";

contract CounterScript is Script {
    function run() public {
        vm.createSelectFork("sepolia");
        vm.startBroadcast();
        new Counter();
        vm.stopBroadcast();

        vm.createSelectFork("bsc-testnet");
        vm.startBroadcast();
        new Counter();
        vm.stopBroadcast();

        vm.createSelectFork("linea-sepolia");
        vm.startBroadcast();
        new Counter();
        vm.stopBroadcast();

        vm.createSelectFork("arbitrum-sepolia");
        vm.startBroadcast();
        new Counter();
        vm.stopBroadcast();
    }
}
