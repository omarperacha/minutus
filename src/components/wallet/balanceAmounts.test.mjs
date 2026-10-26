import assert from "node:assert/strict";
import test from "node:test";
import { totalBalance } from "./balanceAmounts.ts";
import { filteredNetworks } from "./networks.ts";

function balance(rawAmount, decimals = 18) {
  return { response: { snapshot: { assets: [{ symbol: "ETH", rawAmount, decimals }] } } };
}

test("balance totals preserve base units beyond floating-point precision", () => {
  assert.equal(totalBalance([balance("9007199254740993000000001"), balance("9")], "ETH"), "9007199.25474099300000001");
});
test("missing network or token balances do not become a misleading zero or partial total", () => {
  assert.equal(totalBalance([balance("1"), { response: { snapshot: null } }], "ETH"), null);
  assert.equal(totalBalance([balance("1")], "USDC"), null);
  assert.equal(totalBalance([], "ETH"), null);
});
test("totals handle zero, fractional units, whole units and inconsistent decimals", () => {
  assert.equal(totalBalance([balance("0"), balance("0")], "ETH"), "0");
  assert.equal(totalBalance([balance("1", 6), balance("2", 6)], "ETH"), "0.000003");
  assert.equal(totalBalance([balance("1000000", 6)], "ETH"), "1");
  assert.equal(totalBalance([balance("12", 0)], "ETH"), "12");
  assert.equal(totalBalance([balance("1", 6), balance("1", 18)], "ETH"), null);
});
test("all networks excludes test assets and an explicit filter selects only that chain", () => {
  assert.deepEqual(filteredNetworks("all").map(n => n.id), ["baseMainnet", "ethereumMainnet"]);
  assert.deepEqual(filteredNetworks("ethereumSepolia").map(n => n.id), ["ethereumSepolia"]);
});
