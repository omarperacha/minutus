import assert from "node:assert/strict";
import test from "node:test";
import { createWalletStateRefresh } from "./walletStateRefresh.ts";

const ready = { walletId: "synthetic-account", address: "synthetic-public-address", network: "eip155:8453", availability: "ready" };
const locked = { ...ready, address: null, availability: "locked" };
function fixture() {
  let state = ready;
  const reads = [];
  const observer = createWalletStateRefresh(
    () => new Promise((resolve, reject) => reads.push({ resolve, reject })),
    (next) => { state = typeof next === "function" ? next(state) : next; },
  );
  return { observer, reads, state: () => state };
}

test("revocation updates immediately and an older unlocked response cannot undo it", async () => {
  const f = fixture();
  const stale = f.observer.refresh();
  f.observer.invalidate();
  assert.deepEqual(f.state(), locked);
  f.reads[0].resolve(ready);
  await stale;
  assert.deepEqual(f.state(), locked);
  f.reads[1].reject(new Error("synthetic unavailable IPC"));
  await Promise.resolve();
  assert.deepEqual(f.state(), locked);
});

test("explicit action results supersede pending polls and disposed reads are ignored", async () => {
  const f = fixture();
  const stale = f.observer.refresh();
  f.observer.apply(locked);
  f.reads[0].resolve(ready);
  await stale;
  assert.deepEqual(f.state(), locked);
  const fresh = f.observer.refresh();
  f.observer.dispose();
  f.reads[1].resolve(ready);
  await fresh;
  assert.deepEqual(f.state(), locked);
});

test("ordinary focus refresh preserves an unlocked wallet without a locked flash", async () => {
  const f = fixture();
  const refresh = f.observer.refresh();
  assert.deepEqual(f.state(), ready);
  f.reads[0].resolve(ready);
  await refresh;
  assert.deepEqual(f.state(), ready);
});

test("a late unlocked action result cannot override revocation without a fresh authoritative read", async () => {
  const f = fixture();
  f.observer.invalidate();
  f.observer.apply(ready);
  assert.deepEqual(f.state(), locked);
  f.reads[0].resolve(ready);
  await Promise.resolve();
  assert.deepEqual(f.state(), locked);
  f.reads[1].resolve(locked);
  await Promise.resolve();
  assert.deepEqual(f.state(), locked);
});
