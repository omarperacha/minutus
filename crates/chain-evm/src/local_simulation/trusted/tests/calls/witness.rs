//! Independently verified synthetic tries for each contract's storage owner.
use super::*;
use alloy_trie::EMPTY_ROOT_HASH;

pub(super) struct Source<'a>(pub(super) &'a FixtureFacts);
impl SimulationStateSource for Source<'_> {
    fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError> {
        control.remaining()?;
        let key = serde_json::to_value(query).map_err(|_| SimulationError::Protocol)?;
        self.0
            .iter()
            .find(|(q, _)| serde_json::to_value(q).ok().as_ref() == Some(&key))
            .map(|(_, v)| v.clone())
            .ok_or(SimulationError::StateUnavailable)
    }
}
fn l1() -> Address {
    let mut bytes = [0; 20];
    bytes[0] = 0x42;
    bytes[19] = 0x15;
    Address::from(bytes)
}
fn stored_words(owner: Address) -> Result<BTreeMap<U256, U256>, Box<dyn std::error::Error>> {
    let mut words = (0..5u64)
        .map(|slot| (U256::from(slot), U256::ZERO))
        .collect::<BTreeMap<_, _>>();
    for slot in CONTROL_SLOTS {
        words.insert(U256::from_str(slot)?, U256::ZERO);
    }
    if owner == address(0x22) {
        words.insert(U256::ZERO, U256::from(5));
        words.insert(U256::ONE, U256::from_be_slice(address(0x44).as_slice()));
        // Public synthetic Batch fixture mapping at slot3. Each absent key
        // has its own production-verified proof; replay cannot invent zeros.
        for key in 0..=32u64 {
            let mut input = U256::from(key).to_be_bytes::<32>().to_vec();
            input.extend(U256::from(3).to_be_bytes::<32>());
            words.insert(U256::from_be_bytes(keccak256(input).0), U256::ZERO);
        }
    }
    if owner == address(0x44) {
        words.insert(U256::ZERO, U256::from(7));
        for actor in [address(0x11), address(0x22)] {
            let mut input = U256::from_be_slice(actor.as_slice())
                .to_be_bytes::<32>()
                .to_vec();
            input.extend(U256::from(4).to_be_bytes::<32>());
            words.insert(U256::from_be_bytes(keccak256(input).0), U256::ZERO);
        }
    }
    if owner == l1() {
        words = BTreeMap::from([
            (U256::ONE, U256::from(1_000_000_000u64)),
            (
                U256::from(3),
                (U256::from(1_000_000) << 96) | (U256::from(1_000_000) << 64),
            ),
            (U256::from(7), U256::from(1_000_000)),
            (
                U256::from(8),
                (U256::from(16) << 96) | (U256::from(10) << 64) | U256::from(1000),
            ),
        ]);
    }
    Ok(words)
}
fn storage(
    owner: Address,
    values: &mut FixtureSlots,
    facts: &mut FixtureFacts,
) -> Result<B256, Box<dyn std::error::Error>> {
    let words = stored_words(owner)?;
    let leaves = words
        .iter()
        .map(|(key, value)| (keccak256(key.to_be_bytes::<32>()), (*key, *value)))
        .collect::<BTreeMap<_, _>>();
    let mut tree = HashBuilder::default().with_proof_retainer(
        leaves
            .keys()
            .map(Nibbles::unpack)
            .collect::<ProofRetainer>(),
    );
    for (hash, (_, value)) in &leaves {
        if *value != U256::ZERO {
            tree.add_leaf(Nibbles::unpack(hash), &alloy_rlp::encode(*value));
        }
    }
    let root = tree.root();
    let nodes = tree.take_proof_nodes();
    for (hash, (slot, value)) in leaves {
        let proof = nodes
            .matching_nodes_sorted(&Nibbles::unpack(hash))
            .into_iter()
            .map(|(_, n)| format!("{n:#x}"))
            .collect::<Vec<_>>();
        let answer = json!({"storageProof":[{"key":format!("{slot:#x}"),"value":format!("{value:#x}"),"proof":proof}]});
        let checked = proof::storage(root, slot, &answer)?;
        values.insert((owner, slot), checked);
        facts.push((
            StateQuery::Storage {
                address: format!("{owner:#x}"),
                slot: format!("{slot:#066x}"),
            },
            StateValue::Storage {
                value: format!("{checked:#066x}"),
            },
        ));
    }
    Ok(root)
}
pub(super) fn build_with_proxies(
    artifacts: &Artifacts,
    network: EvmNetwork,
    proxies: &[(Address, Address)],
) -> Result<Witness, Box<dyn std::error::Error>> {
    let mut slots = BTreeMap::new();
    let mut facts = Vec::new();
    let mut material = BTreeMap::new();
    material.insert(
        address(0x11),
        (
            TrieAccount {
                nonce: 3,
                balance: U256::from(1_000_000_000_000_000_000u64),
                ..Default::default()
            },
            Bytes::new(),
        ),
    );
    for (owner, artifact) in artifacts {
        let root = storage(*owner, &mut slots, &mut facts)?;
        let code = Bytes::copy_from_slice(artifact.runtime());
        material.insert(
            *owner,
            (
                TrieAccount {
                    nonce: 13,
                    storage_root: root,
                    code_hash: keccak256(&code),
                    ..Default::default()
                },
                code,
            ),
        );
    }
    for (owner, implementation) in proxies {
        let root = storage(*owner, &mut slots, &mut facts)?;
        let code = Bytes::from(alloy_primitives::hex::decode(format!(
            "363d3d373d3d3d363d73{implementation:x}5af43d82803e903d91602b57fd5bf3"
        ))?);
        assert_eq!(code.len(), 45);
        material.insert(
            *owner,
            (
                TrieAccount {
                    nonce: 13,
                    storage_root: root,
                    code_hash: keccak256(&code),
                    ..Default::default()
                },
                code,
            ),
        );
    }
    if network.is_base() {
        for last in [0x15, 0x19, 0x1a, 0x1b] {
            let mut owner = l1().into_array();
            owner[19] = last;
            let owner = Address::from(owner);
            let root = if owner == l1() {
                storage(owner, &mut slots, &mut facts)?
            } else {
                EMPTY_ROOT_HASH
            };
            material.insert(
                owner,
                (
                    TrieAccount {
                        nonce: 1,
                        storage_root: root,
                        ..Default::default()
                    },
                    Bytes::new(),
                ),
            );
        }
    }
    let (root, accounts) = accounts(&material, &mut facts)?;
    let amsterdam = network == EvmNetwork::EthereumSepolia;
    Ok(Witness {
        state: ProvedState::new(accounts, slots),
        facts,
        header: Header {
            state_root: root,
            number: 53_000_000,
            timestamp: 1_794_000_000,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1),
            excess_blob_gas: Some(0),
            beneficiary: address(0x33),
            mix_hash: B256::repeat_byte(0x77),
            slot_number: amsterdam.then_some(10_000_000),
            block_access_list_hash: amsterdam.then_some(B256::ZERO),
            ..Default::default()
        },
    })
}
fn accounts(
    material: &BTreeMap<Address, (TrieAccount, Bytes)>,
    facts: &mut FixtureFacts,
) -> Result<(B256, FixtureAccounts), Box<dyn std::error::Error>> {
    let mut targets = material.keys().copied().collect::<Vec<_>>();
    targets.push(address(0x33));
    let leaves = material
        .iter()
        .map(|(address, (account, _))| (keccak256(address), *account))
        .collect::<BTreeMap<_, _>>();
    let mut tree = HashBuilder::default().with_proof_retainer(
        targets
            .iter()
            .map(|a| Nibbles::unpack(keccak256(a)))
            .collect::<ProofRetainer>(),
    );
    for (hash, account) in leaves {
        tree.add_leaf(Nibbles::unpack(hash), &alloy_rlp::encode(account));
    }
    let root = tree.root();
    let nodes = tree.take_proof_nodes();
    let mut accounts = BTreeMap::new();
    for owner in targets {
        let (account, code) = material.get(&owner).cloned().unwrap_or_default();
        let proof = nodes
            .matching_nodes_sorted(&Nibbles::unpack(keccak256(owner)))
            .into_iter()
            .map(|(_, n)| format!("{n:#x}"))
            .collect::<Vec<_>>();
        let answer = json!({"address":format!("{owner:#x}"),"nonce":format!("{:#x}",account.nonce),"balance":format!("{:#x}",account.balance),"storageHash":format!("{:#x}",account.storage_root),"codeHash":format!("{:#x}",account.code_hash),"accountProof":proof});
        let (exists, checked) = proof::account(root, owner, &answer)?;
        assert_eq!(keccak256(&code), checked.code_hash);
        facts.push((
            StateQuery::Account {
                address: format!("{owner:#x}"),
            },
            StateValue::Account {
                exists,
                nonce: checked.nonce,
                balance: format!("{:#x}", checked.balance),
                code: format!("{code:#x}"),
            },
        ));
        accounts.insert(owner, (exists, checked, code));
    }
    Ok((root, accounts))
}
pub(super) fn request(
    witness: &Witness,
    intent: &crate::contract::UnsignedContractIntent,
) -> Result<SimulationRequest, Box<dyn std::error::Error>> {
    Ok(SimulationRequest {
        network: match intent.network() {
            EvmNetwork::EthereumMainnet => SimulationNetwork::EthereumMainnet,
            EvmNetwork::EthereumSepolia => SimulationNetwork::EthereumSepolia,
            EvmNetwork::BaseMainnet => SimulationNetwork::BaseMainnet,
            EvmNetwork::BaseSepolia => SimulationNetwork::BaseSepolia,
        },
        block: SimulationBlock {
            hash: format!("{:#x}", witness.header.hash_slow()),
            state_root: format!("{:#x}", witness.header.state_root),
            number: witness.header.number,
            timestamp: witness.header.timestamp,
            beneficiary: format!("{:#x}", witness.header.beneficiary),
            gas_limit: witness.header.gas_limit,
            base_fee: 1,
            prevrandao: format!("{:#x}", witness.header.mix_hash),
            excess_blob_gas: 0,
            slot_number: witness.header.slot_number,
            block_access_list_hash: witness
                .header
                .block_access_list_hash
                .map(|v| format!("{v:#x}")),
        },
        transaction: SimulationTransaction {
            caller: intent.account().into(),
            target: intent.call().target().into(),
            value: format!("{:#x}", U256::from_str_radix(intent.value_wei(), 10)?),
            calldata: intent.call().calldata().into(),
            nonce: 3,
            gas_limit: 1_000_000,
            max_fee_per_gas: 2,
            priority_fee_per_gas: 1,
        },
    })
}
