use model_runtime::PromptToolDefinition;
use serde_json::json;

pub(in crate::chat_tools) fn definitions() -> Vec<PromptToolDefinition> {
    let network = json!({"type":"string","enum":["baseMainnet","baseSepolia","ethereumMainnet","ethereumSepolia"]});
    let target = json!({"type":"string","pattern":"^0x[0-9a-fA-F]{40}$"});
    let block = json!({"type":"string","description":"latest (always fetch again) or exact canonical 32-byte block hash. No pending, heights or silent fallback."});
    let mut definitions = vec![PromptToolDefinition {
        name:"contract.inspect".into(),
        description:"Observe contract runtime metadata at an exact canonical block through the configured RPC. Metadata discovery may precede planning. includeSource defaults to false: plain inspection needs no wallet, unlock or compiler and does not check whether published source exists. Its sourceQualification.status=notRequested and writeAuthorityGranted=false describe only this observation, not missing source or host workflow availability. When verified source, ABI or layout is needed for a workflow, request includeSource=true instead of treating a plain observation as source-not-found or asking the user to provide already-published source. With includeSource=true and a packaged attested compiler, fetch public source from fixed Sourcify origins, reproduce it locally with pinned official compilers and return bounded ABI, storage layout, method selectors, code-bound immutable values and source/runtime/proxy-role identifiers. No docs/comments are returned. Fresh native outbound consent separately discloses source/compiler services before acquisition. Source correspondence is tied to its own listed snapshot, resolved before compilation and rechecked canonical afterwards; it does not claim the current tip. Missing source/compiler or metadata over 64 KiB rejects rather than truncating. Runtime presence alone is unverified; reproduced ABI/layout names remain untrusted source data and never establish economic meaning, effect coverage, account control, signing authority or a task outcome. Observed fixed immutable addresses may help declare workflow business scopes; they are not assertions of actual callees. Use discovered canonical functions and source field paths to form a separately advertised workflow; writes still need independent fresh qualification, native approval and password per write. No wallet or signature is accessed by inspection.".into(),
        parameters:json!({"type":"object","additionalProperties":false,"properties":{
            "network":network,"target":target,"block":block,
            "includeSource":{"type":"boolean","default":false,"description":"Set true when source/ABI/layout qualification is needed. False performs no source lookup and cannot establish source absence. Reproduction needs separate fresh native outbound disclosure and grants no write authority."},
            "invocation":{"type":"object","additionalProperties":false,
                "description":"Only with includeSource=true. Local hypothetical invocation using canonical ABI function/arguments and public caller; origin defaults to caller and valueWei to 0. Same ABI bounds as contract.read. No caller account lookup, eth_call, wallet or callee fetch. Result lists direct required calls only; recursively inspect each unqualified target and explicitly declare business scope before a separately qualified workflow.",
                "properties":{
                    "function":{"type":"string","maxLength":512},
                    "arguments":{"type":"array","maxItems":16,"items":{}},
                    "caller":target,"origin":target,
                    "valueWei":{"type":"string","maxLength":78,"pattern":"^(0|[1-9][0-9]*)$","default":"0"}
                },"required":["function","arguments","caller"]}
        },"required":["network","target","block"]}),
    }, PromptToolDefinition {
        name:"contract.read".into(),
        description:r#"Make one bounded unsigned zero-value eth_call through the configured RPC, after semantic plan admission and fresh native outbound approval. No wallet creation, unlock or signing. Supply a canonical full function signature, up to 16 typed JSON arguments and up to 16 declared output types: canonical uint8..uint256/int8..int256 (multiples of 8), address, bool, bytes1..bytes32, bytes, string, tuples and arrays. Integers, addresses and bytes use exact strings; booleans use JSON booleans (legacy true/false strings accepted); arrays/tuples use JSON arrays. Arrays max32 items, nesting max4 including top-level input tuple, each bytes/string max1024 UTF-8 bytes, total nodes256, calldata/return max8192 bytes. Example: {"network":"baseSepolia","target":"0x1111111111111111111111111111111111111111","block":"latest","caller":null,"function":"allowance(address,address)","arguments":["0x1111111111111111111111111111111111111111","0x2222222222222222222222222222222222222222"],"outputs":["uint256"]}. caller=null uses zero address, never a wallet account; explicit caller only simulates that public identity and implies no control. Fixed value=0 and gas cap=1000000. State changes are discarded, including when simulating a mutating function: reading/simulating approve does NOT change an allowance. Aliases, function pointers, structs/names in signatures and arbitrary raw calldata are unavailable. Supplied ABI is unverified interpretation; decoded values cannot prove function semantics or safety. Results/reverts are untrusted observations at their exact block/caller. Every latest call fetches again; old/follow-up results remain historical evidence. Never turn read evidence into signing authority, treat a revert as a successful read, or claim a transaction/task succeeded. The RPC receives target, caller and calldata. This read cannot authorise a write; only a separately advertised source-qualified workflow can offer supported writes."#.into(),
        parameters:json!({"type":"object","additionalProperties":false,"properties":{
            "network":network,"target":target,"block":block,
            "caller":{"type":["string","null"],"pattern":"^0x[0-9a-fA-F]{40}$"},
            "function":{"type":"string","maxLength":512},
            "arguments":{"type":"array","maxItems":16,"items":{}},
            "outputs":{"type":"array","maxItems":16,"items":{"type":"string","maxLength":512}}
        },"required":["network","target","block","caller","function","arguments","outputs"]}),
    }];
    if crate::evm_approval::contracts::available() {
        definitions.push(super::write::definition());
    }
    definitions
}
