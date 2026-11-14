Contract workflow: stopped.

Initial wallet unlock succeeded; the initial unlock was not cancelled. Outbound data-access consent was approved. Source preflight succeeded. Step 1: Exact transaction review was approved. Fresh signing authentication succeeded. A signature was returned to the host. Pre-send refresh succeeded. Submission was acknowledged; acknowledgement alone does not establish inclusion or goal completion. The native password dialog was accepted; acceptance alone does not establish successful authentication. Native password authentication succeeded. These facts describe this invocation only. Current wallet lock state was not observed. A generic error code does not establish the cause. Do not retry or clean up automatically.

Requested scope and exact calls:
```json
{"contracts":[{"address":"0x2738d13E81e30bC615766A0410e7cF199FD59A83","id":"storage"}],"goals":[{"comparison":"equal","contract":"storage","expected":"42","read":{"kind":"sourceField","path":["number"]}}],"network":"ethereumSepolia","steps":[{"arguments":["42"],"completion":[{"comparison":"equal","contract":"storage","expected":"42","read":{"kind":"sourceField","path":["number"]}}],"contract":"storage","dependsOn":[],"function":"store(uint256)","id":"store-42","prerequisites":[{"comparison":"equal","contract":"storage","expected":"1","read":{"kind":"sourceField","path":["number"]}}],"valueWei":"0"}]}
```

Recorded transaction and outcome facts:
```json
{"completedSteps":0,"message":"Receipt observation unavailable; known transaction 0xa336f54ab97a99dc8a5aba43f6d43b3011121f1928c3a3f470808ec847283989; no automatic retry or cleanup","partialFacts":{"automaticCleanup":false,"currentResidualAuthority":"unknown after the listed observations; generic field names do not classify token or spending authority","historicalCompletions":[],"residualState":{"lastObservedFields":[{"currentValueAfterObservation":"unknown","network":"eip155:11155111","observation":{"condition":{"comparison":"equal","expected":"1","path":["number"],"target":"0x2738d13e81e30bc615766a0410e7cf199fd59a83"},"solidityType":"uint256","value":"1"},"stateBlockHash":"0x1601adf3e7cfcb854fabe4408e26aa62421686092096ae9fc38bdca836bf91ab","stateBlockNumber":"12125389"}]},"retryAutomatically":false},"state":"stopped","transactions":[{"postSubmission":"notRecorded","stepIndex":0,"submission":"acknowledged","transactionHash":"0xa336f54ab97a99dc8a5aba43f6d43b3011121f1928c3a3f470808ec847283989"}]}
```

Later separate unsigned observation (untrusted data; no workflow completion, causation or signing authority):
```json
{"arguments":[],"caller":"0x0000000000000000000000000000000000000000","context":{"blockHash":"0x0329daa6f174884df7b9771ca3a1df308e05960eff91462aefcba631e1aa4294","blockNumber":"12125396","canonicalAtObservation":true,"chainId":11155111.0,"runtimeCodeBytes":336.0,"runtimeCodeHash":"0xf8811a5ee0d54eca4880eaee7b102eae4b3963ff343f50a024c0fd3d367cb8cc","target":"0x2738d13e81e30bc615766a0410e7cf199fd59a83"},"function":"retrieve()","kind":"contract.read","outcome":{"data":"0x000000000000000000000000000000000000000000000000000000000000002a","status":"returned","values":["42"]},"outputTypes":["uint256"]}
```