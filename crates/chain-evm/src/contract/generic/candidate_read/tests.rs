//! Missing evidence is distinct from an independently proved zero.
use super::*;
use std::{collections::BTreeMap, time::Instant};

#[test]
fn cached_zero_is_present_but_absent_slots_and_accounts_need_proofs() -> Result<(), GenericError> {
    let address = Address::repeat_byte(1);
    let state = ProvedState::test_fixture(
        BTreeMap::new(),
        BTreeMap::from([((address, U256::ZERO), U256::ZERO)]),
    );
    let query = |slot| StateQuery::Storage {
        address: format!("{address:#x}"),
        slot: format!("{slot:#066x}"),
    };
    assert!(
        matches!(cached(&state,&query(U256::ZERO))?,Some(StateValue::Storage {value}) if value==format!("{:#066x}",U256::ZERO))
    );
    assert!(cached(&state, &query(U256::ONE))?.is_none());
    assert!(
        cached(
            &state,
            &StateQuery::Account {
                address: format!("{address:#x}")
            }
        )?
        .is_none()
    );
    Ok(())
}
#[test]
fn same_height_alone_does_not_bind_a_cached_candidate() {
    let cohort = GenericLiveCohort {
        network: EvmNetwork::EthereumSepolia,
        block_hash: "exact".into(),
        block_number: 10,
        selected_at: Instant::now(),
    };
    assert!(check_cohort(&cohort, cohort.network(), "exact", 10).is_ok());
    for (network, hash, number) in [
        (cohort.network(), "other", 10),
        (cohort.network(), "exact", 11),
        (EvmNetwork::BaseSepolia, "exact", 10),
    ] {
        assert_eq!(
            check_cohort(&cohort, network, hash, number),
            Err(GenericError::SnapshotMismatch)
        );
    }
}

#[test]
fn reader_uses_existing_zero_without_io_but_missing_evidence_fails_closed()
-> Result<(), GenericError> {
    let address = Address::repeat_byte(1);
    let state = ProvedState::test_fixture(
        BTreeMap::new(),
        BTreeMap::from([((address, U256::ZERO), U256::ZERO)]),
    );
    let endpoint = EvmRpcUrl::loopback_test(0);
    // Private synthetic owner fixture only: this constructs no candidate,
    // source artifact or current workflow certificate.
    let mut reader = CandidateStateReader {
        endpoint: &endpoint,
        network: EvmNetwork::EthereumSepolia,
        hash: "invalid",
        number: 10,
        root: B256::ZERO,
        before: Some(&state),
        missing: None,
        queries: vec![],
    };
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let query = |slot| StateQuery::Storage {
        address: format!("{address:#x}"),
        slot: format!("{slot:#066x}"),
    };
    assert!(
        matches!(reader.query(&query(U256::ZERO),&control)?,StateValue::Storage {value} if value==format!("{:#066x}",U256::ZERO))
    );
    assert_eq!(
        reader.query(&query(U256::ONE), &control).err(),
        Some(GenericError::MissingState)
    );
    assert!(reader.missing.is_none());
    Ok(())
}
#[test]
fn cached_reads_remain_cancelled_and_uniquely_bounded() -> Result<(), GenericError> {
    let address = Address::repeat_byte(1);
    let state = ProvedState::test_fixture(
        BTreeMap::new(),
        (0..128u64)
            .map(|slot| ((address, U256::from(slot)), U256::ZERO))
            .collect(),
    );
    let endpoint = EvmRpcUrl::loopback_test(0);
    let mut reader = CandidateStateReader {
        endpoint: &endpoint,
        network: EvmNetwork::EthereumSepolia,
        hash: "invalid",
        number: 10,
        root: B256::ZERO,
        before: Some(&state),
        missing: None,
        queries: vec![],
    };
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let control = SimulationControl::start(&cancelled);
    let query = |slot| StateQuery::Storage {
        address: format!("{address:#x}"),
        slot: format!("{slot:#066x}"),
    };
    for slot in 0..128u64 {
        reader.query(&query(U256::from(slot)), &control)?;
    }
    reader.query(&query(U256::ZERO), &control)?;
    assert_eq!(
        reader.query(&query(U256::from(128)), &control).err(),
        Some(GenericError::Limit)
    );
    cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        reader.query(&query(U256::ZERO), &control).err(),
        Some(GenericError::Expired)
    );
    Ok(())
}
