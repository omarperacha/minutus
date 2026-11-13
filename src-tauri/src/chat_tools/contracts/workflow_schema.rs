//! One generic immutable declaration schema, without execution-authority fields.
use model_runtime::PromptToolDefinition;
use serde_json::{Value, json};
pub(super) fn definition() -> PromptToolDefinition {
    let identifier =
        json!({"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_-]+$"});
    let function = json!({"type":"string","minLength":1,"maxLength":512});
    let arguments = json!({"type":"array","maxItems":16,"items":abi_value(0)});
    let condition = json!({"type":"object","additionalProperties":false,"properties":{"contract":identifier,"read":{"type":"object","additionalProperties":false,"properties":{"kind":{"type":"string","const":"sourceField"},"path":{"type":"array","minItems":1,"maxItems":8,"items":{"type":"string","minLength":1,"maxLength":256}}},"required":["kind","path"]},"comparison":{"type":"string","enum":["equal","atLeast","atMost"]},"expected":abi_value(0)},"required":["contract","read","comparison","expected"]});
    PromptToolDefinition{name:"contract.request_workflow".into(),description:"Request one immutable ordered workflow of arbitrary source-ABI-decoded contract functions, including a single write. Version 2 binds a fixed network, selected wallet account, all root and external business contract addresses, exact functions/arguments/native values, historical dependencies and separately declared current/completion conditions. The host fetches public source at fixed Sourcify origins, reproduces it locally with pinned official Solidity compilers, matches current proof-checked runtime/dispatch and independently qualifies complete bounded structural execution effects. Provide a concise model-written summary for each step when generating this tool call, before host work begins: one plain paragraph of at most 512 Unicode characters and 2048 UTF-8 bytes describing the intended action. It is displayed as an unverified model explanation, not checked effects or transaction approval; do not claim safety, verification or approval. No extra model call is made after native approval. Every business callee must be listed in contracts before any source/RPC acquisition. Exact minimal-proxy code-only implementations may be discovered from proved runtime; that does not grant permission to invoke them as additional business targets. No function-name catalogue supplies authority; missing source, unsupported compiler/layout/dispatch/effects or mismatched evidence reject. Source field and event names are declarations, not token/economic safety classifications. Conditions use exact reproduced storage-layout paths: field/member names, canonical mapping keys and array indices. Integers are decimal strings; use boolean values for bool. Completion conditions are historical observations after canonical inclusion; dependsOn retains their receipt/observation anchors without demanding consumed old values still hold. Prerequisites are freshly proved before each signature and dispatch. Final goals are separately proved after all writes. Empty completion/goals arrays declare no business postcondition; report executionConfirmed with goals undeclared, never a verified business outcome. Source/ABI, bounded inclusion-time effect relations, condition feasibility and complete-report capacity for ALL steps are admitted before the first write. The host rejects workflows whose worst-case complete historical/goal/residual report would exceed 64 KiB; it never truncates post-send evidence. Requires a ready externalWrite semantic plan, native outbound consent before RPC/source/compiler acquisition, independent exact approval and fresh password for EVERY write, and an existing selected wallet. If locked, the host prompts native unlock in chat before outbound scope consent; cancelling ends this attempt. Unlocking grants no transaction approval and does not replace the separate fresh password after exact approval for each write. A later denial/failure/reorg/uncertain send stops the workflow; earlier effects may remain. Report exact proved values/as-of blocks and unknown subsequent residual authority. No automatic cleanup, replacement, retry or raw-body signing. Qualification builds require packaged attested compiler/execution helpers; release availability remains disabled pending native interaction and final security review. Report only explicit host lifecycle facts for the same attempt and step. Initial unlock and outbound-scope consent do not establish exact transaction approval or fresh signing authentication. An error code alone does not establish which prompt, authentication, signature or submission occurred; retain unknown states and do not invent denial, skipped passwords, source changes or a known-unsent outcome. A missing returned signature does not prove signing never began. Earlier reads describe their listed blocks and do not prove current or unchanged state. An executed workflow failure remains stopped without automatic retry or cleanup.".into(),parameters:json!({"type":"object","additionalProperties":false,"properties":{"contractVersion":{"type":"integer","const":2},"network":{"type":"string","enum":["baseMainnet","baseSepolia","ethereumMainnet","ethereumSepolia"]},"contracts":{"type":"array","description":"Every fixed root and external business call target across all steps; implementation code-only dependencies grant no additional business target.","minItems":1,"maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{"id":identifier,"address":{"type":"string","pattern":"^0x[0-9a-fA-F]{40}$"}},"required":["id","address"]}},"steps":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{"id":identifier,"contract":identifier,"function":function,"arguments":arguments,"summary":{"type":"string","description":"Optional model-written explanation shown as unverified. One plain paragraph; no control, bidi or invisible formatting characters. At most 512 Unicode characters and 2048 UTF-8 bytes; does not change the transaction or grant authority.","minLength":1,"maxLength":512},"valueWei":{"type":"string","minLength":1,"maxLength":78,"pattern":"^(0|[1-9][0-9]*)$"},"dependsOn":{"type":"array","maxItems":15,"items":identifier},"prerequisites":{"type":"array","maxItems":16,"items":condition},"completion":{"type":"array","minItems":0,"maxItems":16,"items":condition}},"required":["id","contract","function","arguments","valueWei","dependsOn","prerequisites","completion"]}},"goals":{"type":"array","minItems":0,"maxItems":16,"items":condition}},"required":["contractVersion","network","contracts","steps","goals"]})}
}
fn abi_value(depth: usize) -> Value {
    let mut variants = vec![
        json!({"type":"string","maxLength":2050}),
        json!({"type":"boolean"}),
    ];
    if depth < 4 {
        variants.push(json!({"type":"array","maxItems":32,"items":abi_value(depth+1)}));
    }
    json!({"oneOf":variants})
}

#[cfg(test)]
mod tests {
    #[test]
    fn step_summary_is_optional_bounded_presentation_without_authority_fields() {
        let definition = super::definition();
        let step = &definition.parameters["properties"]["steps"]["items"];
        assert_eq!(step["properties"]["summary"]["maxLength"], 512);
        assert_eq!(step["properties"]["summary"]["type"], "string");
        assert!(
            !step["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == "summary"))
        );
        assert_eq!(step["additionalProperties"], false);
        assert!(step["properties"].get("trustedSummary").is_none());
        assert!(
            definition
                .description
                .contains("unverified model explanation")
        );
    }
}
