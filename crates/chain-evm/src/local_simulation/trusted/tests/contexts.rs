//! Real candidate joins for fixed delegation and network-specific fee contexts.
use super::*;
use crate::contract::generic::GenericNativeAccountMode;
use evm_simulation::{ReplayCallKind, SimulationStateSource, offline::FrozenBaseRequest};

fn l1() -> Address {
    Address::from_slice(&[
        0x42, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x15,
    ])
}
fn fee_accounts() -> [Address; 4] {
    [0x15, 0x19, 0x1a, 0x1b].map(|last| {
        let mut bytes = l1().into_array();
        bytes[19] = last;
        Address::from(bytes)
    })
}
fn minimal_runtime() -> Result<Bytes, Box<dyn std::error::Error>> {
    Ok(Bytes::from(alloy_primitives::hex::decode(format!(
        "363d3d373d3d3d363d73{:x}5af43d82803e903d91602b57fd5bf3",
        address(0x44)
    ))?))
}
fn native_storage(
    values: &mut FixtureSlots,
    facts: &mut FixtureFacts,
) -> Result<B256, Box<dyn std::error::Error>> {
    let entries = [
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
    ];
    let sorted = entries
        .into_iter()
        .map(|(slot, value)| (keccak256(slot.to_be_bytes::<32>()), (slot, value)))
        .collect::<BTreeMap<_, _>>();
    let mut tree = HashBuilder::default().with_proof_retainer(
        sorted
            .keys()
            .map(Nibbles::unpack)
            .collect::<ProofRetainer>(),
    );
    for (hash, (_, value)) in &sorted {
        tree.add_leaf(Nibbles::unpack(hash), &alloy_rlp::encode(*value));
    }
    let root = tree.root();
    let nodes = tree.take_proof_nodes();
    for (hash, (slot, value)) in sorted {
        let proof = nodes
            .matching_nodes_sorted(&Nibbles::unpack(hash))
            .into_iter()
            .map(|(_, node)| format!("{node:#x}"))
            .collect::<Vec<_>>();
        let answer = json!({"storageProof":[{"key":format!("{slot:#x}"),"value":format!("{value:#x}"),"proof":proof}]});
        let checked = proof::storage(root, slot, &answer)?;
        values.insert((l1(), slot), checked);
        facts.push((
            StateQuery::Storage {
                address: format!("{:#x}", l1()),
                slot: format!("{slot:#066x}"),
            },
            StateValue::Storage {
                value: format!("{checked:#066x}"),
            },
        ));
    }
    Ok(root)
}
fn account_material(
    artifact: &ReproducedArtifact,
    storage_root: B256,
    native_root: Option<B256>,
    proxy: bool,
) -> Result<BTreeMap<Address, (TrieAccount, Bytes)>, Box<dyn std::error::Error>> {
    let runtime = Bytes::copy_from_slice(artifact.runtime());
    let target_code = if proxy {
        minimal_runtime()?
    } else {
        runtime.clone()
    };
    let mut accounts = BTreeMap::from([
        (
            address(0x11),
            (
                TrieAccount {
                    nonce: 3,
                    balance: U256::from(1_000_000_000_000_000_000u64),
                    ..Default::default()
                },
                Bytes::new(),
            ),
        ),
        (
            address(0x22),
            (
                TrieAccount {
                    nonce: 13,
                    storage_root,
                    code_hash: keccak256(&target_code),
                    ..Default::default()
                },
                target_code,
            ),
        ),
    ]);
    if proxy {
        accounts.insert(
            address(0x44),
            (
                TrieAccount {
                    nonce: 13,
                    code_hash: keccak256(&runtime),
                    ..Default::default()
                },
                runtime,
            ),
        );
    }
    if let Some(root) = native_root {
        for address in fee_accounts() {
            accounts.insert(
                address,
                (
                    TrieAccount {
                        nonce: 1,
                        storage_root: if address == l1() {
                            root
                        } else {
                            alloy_trie::EMPTY_ROOT_HASH
                        },
                        ..Default::default()
                    },
                    Bytes::new(),
                ),
            );
        }
    }
    Ok(accounts)
}
fn prove_accounts(
    material: &BTreeMap<Address, (TrieAccount, Bytes)>,
    facts: &mut FixtureFacts,
) -> Result<(B256, FixtureAccounts), Box<dyn std::error::Error>> {
    let mut targets = material.keys().copied().collect::<Vec<_>>();
    targets.push(address(0x33));
    let leaves = material
        .iter()
        .map(|(address, (account, _))| (keccak256(address), account))
        .collect::<BTreeMap<_, _>>();
    let mut tree = HashBuilder::default().with_proof_retainer(
        targets
            .iter()
            .map(|a| Nibbles::unpack(keccak256(a)))
            .collect::<ProofRetainer>(),
    );
    for (hash, account) in leaves {
        tree.add_leaf(Nibbles::unpack(hash), &alloy_rlp::encode(*account));
    }
    let root = tree.root();
    let nodes = tree.take_proof_nodes();
    let mut accounts = BTreeMap::new();
    for address in targets {
        let (account, code) = material.get(&address).cloned().unwrap_or_default();
        let proof = nodes
            .matching_nodes_sorted(&Nibbles::unpack(keccak256(address)))
            .into_iter()
            .map(|(_, node)| format!("{node:#x}"))
            .collect::<Vec<_>>();
        let answer = json!({"address":format!("{address:#x}"),"nonce":format!("{:#x}",account.nonce),"balance":format!("{:#x}",account.balance),"storageHash":format!("{:#x}",account.storage_root),"codeHash":format!("{:#x}",account.code_hash),"accountProof":proof});
        let (exists, checked) = proof::account(root, address, &answer)?;
        assert_eq!(keccak256(&code), checked.code_hash);
        facts.push((
            StateQuery::Account {
                address: format!("{address:#x}"),
            },
            StateValue::Account {
                exists,
                balance: format!("{:#x}", checked.balance),
                nonce: checked.nonce,
                code: format!("{code:#x}"),
            },
        ));
        accounts.insert(address, (exists, checked, code));
    }
    Ok((root, accounts))
}
fn context_witness(
    artifact: &ReproducedArtifact,
    network: EvmNetwork,
    proxy: bool,
    timestamp: u64,
) -> Result<Witness, Box<dyn std::error::Error>> {
    let (storage_root, mut values, mut facts) = storage_witness()?;
    let native_root = if network.is_base() {
        Some(native_storage(&mut values, &mut facts)?)
    } else {
        None
    };
    let material = account_material(artifact, storage_root, native_root, proxy)?;
    let (root, accounts) = prove_accounts(&material, &mut facts)?;
    let amsterdam = network == EvmNetwork::EthereumSepolia && timestamp >= 1_791_294_816;
    Ok(Witness {
        state: ProvedState::new(accounts, values),
        facts,
        header: Header {
            state_root: root,
            number: 53_000_000,
            timestamp,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1),
            excess_blob_gas: Some(0),
            beneficiary: address(0x33),
            mix_hash: B256::repeat_byte(0x44),
            slot_number: amsterdam.then_some(10_000_000),
            block_access_list_hash: amsterdam.then_some(B256::ZERO),
            ..Default::default()
        },
    })
}
fn request(
    witness: &Witness,
    intent: &crate::contract::UnsignedContractIntent,
) -> SimulationRequest {
    let network = match intent.network() {
        EvmNetwork::EthereumMainnet => SimulationNetwork::EthereumMainnet,
        EvmNetwork::EthereumSepolia => SimulationNetwork::EthereumSepolia,
        EvmNetwork::BaseMainnet => SimulationNetwork::BaseMainnet,
        EvmNetwork::BaseSepolia => SimulationNetwork::BaseSepolia,
    };
    SimulationRequest {
        network,
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
                .map(|hash| format!("{hash:#x}")),
        },
        transaction: SimulationTransaction {
            caller: intent.account().into(),
            target: intent.call().target().into(),
            value: "0x0".into(),
            calldata: intent.call().calldata().into(),
            nonce: 3,
            gas_limit: 1_000_000,
            max_fee_per_gas: 2,
            priority_fee_per_gas: 1,
        },
    }
}
struct ProvedFixtureSource<'a>(&'a FixtureFacts);
impl SimulationStateSource for ProvedFixtureSource<'_> {
    fn query(
        &mut self,
        query: &StateQuery,
        control: &SimulationControl<'_>,
    ) -> Result<StateValue, SimulationError> {
        control.remaining()?;
        let encoded = serde_json::to_value(query).map_err(|_| SimulationError::Protocol)?;
        self.0
            .iter()
            .find(|(key, _)| serde_json::to_value(key).ok().as_ref() == Some(&encoded))
            .map(|(_, value)| value.clone())
            .ok_or(SimulationError::StateUnavailable)
    }
}
fn contextual_candidate(
    artifact: &Arc<ReproducedArtifact>,
    network: EvmNetwork,
    proxy: bool,
    timestamp: u64,
) -> Result<GenericContractCandidate, Box<dyn std::error::Error>> {
    let witness = context_witness(artifact, network, proxy, timestamp)?;
    let artifacts = [(
        address(if proxy { 0x44 } else { 0x22 }),
        Arc::clone(artifact),
    )];
    let identity = GenericContractIdentity::from_source_state(
        network,
        address(0x22),
        &witness.state,
        &artifacts,
    )?;
    let intent = generic_contract_intent(
        &identity,
        &format!("{:#x}", address(0x11)),
        "0",
        "store(uint256)",
        &[json!("42")],
    )?;
    let request = request(&witness, &intent);
    assert_eq!(
        request.block.hash,
        format!("{:#x}", witness.header.hash_slow())
    );
    assert_eq!(request.block.slot_number, witness.header.slot_number);
    assert_eq!(
        request.block.block_access_list_hash,
        witness
            .header
            .block_access_list_hash
            .map(|hash| format!("{hash:#x}"))
    );
    let cancelled = AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let prepared_at = Instant::now();
    let execution = replay_frozen(&request, &witness.facts, &control)?;
    let written = execution
        .written_slots()
        .iter()
        .map(|(a, s)| Ok((Address::from_str(a)?, U256::from_str(s)?)))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let (base, base_fee) = if network.is_base() {
        assert!(execution.is_base_application());
        let manifest = witness
            .facts
            .iter()
            .map(|(q, _)| q.clone())
            .collect::<Vec<_>>();
        let mut source = ProvedFixtureSource(&witness.facts);
        let snapshot =
            FrozenBaseRequest::prepare(request.clone(), &manifest, &mut source, &control)?;
        let ceiling = snapshot.fee_ceiling()?.total_wei();
        assert_eq!(execution.fee_ceiling_wei(), Some(ceiling.as_str()));
        (Some(snapshot), Some(ceiling))
    } else {
        assert!(!execution.is_base_application());
        (None, None)
    };
    let replay = TrustedReplay {
        request,
        state: witness.state,
        execution,
        written,
        prepared_at,
        base,
        base_fee,
    };
    Ok(GenericContractCandidate::from_replay(
        intent, replay, &artifacts,
    )?)
}
#[test]
#[ignore = "requires normal attested compiler helper and embedded unit-test pin section"]
fn actual_compiled_proxy_and_all_network_fee_contexts_qualify()
-> Result<(), Box<dyn std::error::Error>> {
    let artifact = compile()?;
    for (network, timestamp) in [
        (EvmNetwork::EthereumMainnet, 1_793_300_000),
        (EvmNetwork::EthereumSepolia, 1_790_000_000),
        (EvmNetwork::EthereumSepolia, 1_793_300_000),
        (EvmNetwork::BaseMainnet, 1_793_300_000),
        (EvmNetwork::BaseMainnet, 1_794_000_000),
        (EvmNetwork::BaseSepolia, 1_794_000_000),
    ] {
        for proxy in [false, true] {
            let candidate = contextual_candidate(&artifact, network, proxy, timestamp)?;
            assert_eq!(candidate.intent().network(), network);
            assert_eq!(
                candidate.decoded().chain_id(),
                network.descriptor().chain_id()
            );
            assert_eq!(candidate.effects().storage_changes().len(), 1);
            assert_eq!(
                candidate.effects().storage_changes()[0].storage_address,
                format!("{:#x}", address(0x22))
            );
            assert_eq!(
                candidate.effects().storage_changes()[0].fields[0].after,
                json!("42")
            );
            assert_eq!(
                candidate.effects().native_account_mode(),
                if network.is_base() {
                    GenericNativeAccountMode::ConservativeBasePrepayment
                } else {
                    GenericNativeAccountMode::ExactTransaction
                }
            );
            assert_eq!(candidate.replay().calls().len(), if proxy { 2 } else { 1 });
            if proxy {
                let delegate = &candidate.replay().calls()[1];
                assert_eq!(delegate.kind(), ReplayCallKind::DelegateCall);
                assert_eq!(delegate.target(), format!("{:#x}", address(0x22)));
                assert_eq!(delegate.code_address(), format!("{:#x}", address(0x44)));
            }
        }
    }
    Ok(())
}
