# Public Ethereum Sepolia receipt regression

Captured on 2026-11-13 through the default public Sepolia RPC, with no wallet
or signing operation. These fixtures are public transaction, receipt, full header
and inclusion-proof bytes for transaction
`0x543e7af2dce6b9aea0b81a40fa8decf08ab940e52d3f1a1a194127b93aed7d80`,
block `0xd0ea5ed7c54767a22267fc4ee4ff07c1ac12917d70c76f4ad3e3503f00597a7f`,
height 12,126,246, transaction index 105.

The receipt cumulative transaction gas is 138,041,977; header block gas is
96,694,775. Amsterdam EIP-8037/7778 gives these different dimensions. The receipt
remains committed by the complete header's receipt root. The regression checks
that root, transaction root, canonical encodings, exact public signing digest,
recovered account, nonce, chain and status through the production pure verifier.
Mutation tests retain rejection of changed bindings, header, proof and status.

This is offline playback of an observed block, not a fresh canonicality check,
consensus-finality proof, business completion certificate or new host authority.
The normal RPC owner still checks chain and canonical full headers before and
after live verification. No test-only certificate constructor is exposed.
