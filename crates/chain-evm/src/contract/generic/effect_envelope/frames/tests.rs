use super::*;
use serde_json::json;

fn invocation(owner: u8, parent: Option<u16>, caller: u8) -> GenericEffectInvocation {
    let address = format!("{:#x}", Address::repeat_byte(owner));
    GenericEffectInvocation {
        parent,
        storage_address: address.clone(),
        code_address: address,
        caller: format!("{:#x}", Address::repeat_byte(caller)),
        function: "store(uint256)".into(),
        arguments: vec![json!("42")],
        value_wei: "0".into(),
        calldata: "0x6057361d000000000000000000000000000000000000000000000000000000000000002a"
            .into(),
        is_static: false,
    }
}
fn frame(node: &GenericEffectInvocation, depth: usize) -> GenericCallEffect {
    GenericCallEffect {
        kind: if node.is_static { "staticCall" } else { "call" }.into(),
        storage_address: node.storage_address.clone(),
        code_address: node.storage_address.clone(),
        caller: node.caller.clone(),
        apparent_value: format!(
            "{:#x}",
            U256::from_str_radix(&node.value_wei, 10).unwrap_or_default()
        ),
        input: node.calldata.clone(),
        output: "0x".into(),
        succeeded: true,
        committed: true,
        depth,
    }
}
#[test]
fn complete_preorder_tree_and_static_edge_match() -> Result<(), GenericError> {
    let root = invocation(0x22, None, 0x11);
    let mut child = invocation(0x44, Some(0), 0x22);
    child.is_static = true;
    let grandchild = invocation(0x55, Some(1), 0x44);
    let sibling = invocation(0x66, Some(0), 0x22);
    let nodes = vec![root, child, grandchild, sibling];
    let frames = nodes
        .iter()
        .zip([1, 2, 3, 2])
        .map(|(node, depth)| frame(node, depth))
        .collect::<Vec<_>>();
    match_tree(&nodes, &frames, |_| Ok(false))
}
#[test]
fn every_frame_field_missing_extra_and_reverted_frames_are_rejected() -> Result<(), GenericError> {
    let nodes = vec![
        invocation(0x22, None, 0x11),
        invocation(0x44, Some(0), 0x22),
    ];
    let frames = vec![frame(&nodes[0], 1), frame(&nodes[1], 2)];
    match_tree(&nodes, &frames, |_| Ok(false))?;
    let mutations: [fn(&mut GenericCallEffect); 10] = [
        |f| f.kind = "delegateCall".into(),
        |f| f.storage_address = format!("{:#x}", Address::repeat_byte(0x77)),
        |f| f.code_address = format!("{:#x}", Address::repeat_byte(0x77)),
        |f| f.caller = format!("{:#x}", Address::repeat_byte(0x11)),
        |f| f.apparent_value = "0x1".into(),
        |f| f.input = "0xdeadbeef".into(),
        |f| f.depth = 3,
        |f| f.succeeded = false,
        |f| f.committed = false,
        |f| f.kind = "staticCall".into(),
    ];
    for mutate in mutations {
        let mut changed = frames.clone();
        mutate(&mut changed[1]);
        assert!(match_tree(&nodes, &changed, |_| Ok(false)).is_err());
    }
    assert!(match_tree(&nodes, &frames[..1], |_| Ok(false)).is_err());
    let mut extra = frames.clone();
    extra.push(frames[1].clone());
    assert!(match_tree(&nodes, &extra, |_| Ok(false)).is_err());
    Ok(())
}
#[test]
fn closed_parent_cannot_be_reentered_by_an_out_of_order_child() {
    let nodes = vec![
        invocation(0x22, None, 0x11),
        invocation(0x44, Some(0), 0x22),
        invocation(0x44, Some(0), 0x22),
        invocation(0x55, Some(1), 0x44),
    ];
    let frames = nodes
        .iter()
        .zip([1, 2, 2, 3])
        .map(|(n, d)| frame(n, d))
        .collect::<Vec<_>>();
    assert!(match_tree(&nodes, &frames, |_| Ok(false)).is_err());
}
#[test]
fn exact_proxy_forwarding_retains_context_and_changes_child_depth() -> Result<(), GenericError> {
    let mut root = invocation(0x22, None, 0x11);
    root.code_address = format!("{:#x}", Address::repeat_byte(0x66));
    root.value_wei = "5".into();
    let child = invocation(0x44, Some(0), 0x22);
    let outer = frame(&root, 1);
    let mut implementation = frame(&root, 2);
    implementation.kind = "delegateCall".into();
    implementation.code_address = root.code_address.clone();
    let nodes = vec![root, child];
    let frames = vec![outer, implementation, frame(&nodes[1], 3)];
    match_tree(&nodes, &frames, |node| {
        Ok(node.storage_address != node.code_address)
    })?;
    for index in [0, 1] {
        let mut changed = frames.clone();
        changed[index].output = "0x01".into();
        assert!(
            match_tree(&nodes, &changed, |node| Ok(
                node.storage_address != node.code_address
            ))
            .is_err()
        );
    }
    let mut wrong = frames.clone();
    wrong[1].caller = nodes[0].storage_address.clone();
    assert!(
        match_tree(&nodes, &wrong, |node| Ok(
            node.storage_address != node.code_address
        ))
        .is_err()
    );
    assert!(match_tree(&nodes, &frames, |_| Ok(false)).is_err());
    Ok(())
}
