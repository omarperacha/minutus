//! Request-local transport ledger for exact managed rich-input presentation.

use app_core::{AppCore, AppCoreError, RichInputPresentationPolicy};
use ipc_types::AssetId;

const MAX_PRESENTATION_RECORDS: usize = 64;

#[derive(Clone, Copy, Default)]
pub(super) struct RichInputCounts {
    pub(super) unique_sources: usize,
    pub(super) presentations: usize,
    pub(super) explicit_inspections: usize,
    pub(super) retained_presentations: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresentationRecord {
    asset_id: AssetId,
    content_sha256: String,
    binding_sha256: String,
    presentation_round: usize,
    semantic_stage: Option<String>,
}

/// Transport-only facts for rich bytes shown during one generation request.
///
/// Records are never persisted and contain no OCR text, semantic claims,
/// permissions, or completion state.
#[derive(Default)]
pub(super) struct RichInputPresentationLedger {
    records: Vec<PresentationRecord>,
    recovery_images: Vec<AssetId>,
    counts: RichInputCounts,
}

impl RichInputPresentationLedger {
    pub(super) const fn counts(&self) -> RichInputCounts {
        self.counts
    }

    pub(super) fn record_inspection_delivery(&mut self, explicit: usize, presented: usize) {
        self.counts.explicit_inspections =
            self.counts.explicit_inspections.saturating_add(explicit);
        self.counts.retained_presentations = self
            .counts
            .retained_presentations
            .saturating_add(presented.saturating_sub(explicit));
    }
    #[cfg(any(debug_assertions, test))]
    pub(super) fn requests_recovery_for(&self, asset_ids: &[AssetId]) -> bool {
        self.recovery_images.iter().any(|id| asset_ids.contains(id))
    }

    pub(super) fn confirmed_inspections(
        &self,
        core: &AppCore,
        pending: impl IntoIterator<Item = AssetId>,
        presented: &[AssetId],
    ) -> Result<Vec<AssetId>, AppCoreError> {
        let mut confirmed = Vec::new();
        for id in pending {
            let identity = core.rich_input_identity(&id)?;
            if self.records.iter().any(|record| {
                presented.contains(&record.asset_id)
                    && record.content_sha256 == identity.content_sha256()
            }) {
                confirmed.push(id);
            }
        }
        Ok(confirmed)
    }

    pub(super) fn prompt_policy(&self) -> RichInputPresentationPolicy {
        RichInputPresentationPolicy::from_verified(self.records.iter().map(|record| {
            (
                record.asset_id.clone(),
                record.content_sha256.clone(),
                record.binding_sha256.clone(),
            )
        }))
        .with_retained_presentations()
        .with_requested_images(self.recovery_images.iter().cloned())
    }

    pub(super) fn request_recovery_images(&mut self, asset_ids: &[AssetId]) {
        self.recovery_images = asset_ids
            .iter()
            .take(MAX_PRESENTATION_RECORDS)
            .cloned()
            .collect();
    }

    pub(super) fn prompt_policy_with_inspections(
        &self,
        inspected: impl IntoIterator<Item = AssetId>,
    ) -> RichInputPresentationPolicy {
        self.prompt_policy().with_requested_images(
            inspected
                .into_iter()
                .chain(self.recovery_images.iter().cloned()),
        )
    }

    pub(super) fn record_presented(
        &mut self,
        core: &AppCore,
        asset_ids: &[AssetId],
        binding_sha256: &str,
        presentation_round: usize,
        semantic_stage: Option<&str>,
    ) -> Result<(), AppCoreError> {
        let mut presented_digests = Vec::new();
        for asset_id in asset_ids {
            let identity = core.rich_input_identity(asset_id)?;
            self.counts.presentations = self.counts.presentations.saturating_add(1);
            if !self
                .records
                .iter()
                .any(|record| record.content_sha256 == identity.content_sha256())
            {
                self.counts.unique_sources = self.counts.unique_sources.saturating_add(1);
            }
            presented_digests.push(identity.content_sha256().to_owned());
            self.record_verified(
                identity.asset_id().clone(),
                identity.content_sha256(),
                binding_sha256,
                presentation_round,
                semantic_stage,
            );
        }
        let mut remaining = Vec::new();
        for asset_id in &self.recovery_images {
            let identity = core.rich_input_identity(asset_id)?;
            if !presented_digests
                .iter()
                .any(|digest| digest == identity.content_sha256())
            {
                remaining.push(asset_id.clone());
            }
        }
        self.recovery_images = remaining;
        Ok(())
    }

    fn record_verified(
        &mut self,
        asset_id: AssetId,
        content_sha256: &str,
        binding_sha256: &str,
        presentation_round: usize,
        semantic_stage: Option<&str>,
    ) {
        if self.records.iter().any(|record| {
            record.asset_id == asset_id
                && record.content_sha256 == content_sha256
                && record.binding_sha256 == binding_sha256
        }) {
            return;
        }
        if self.records.len() == MAX_PRESENTATION_RECORDS {
            self.records.remove(0);
        }
        self.records.push(PresentationRecord {
            asset_id,
            content_sha256: content_sha256.to_owned(),
            binding_sha256: binding_sha256.to_owned(),
            presentation_round,
            semantic_stage: semantic_stage.map(str::to_owned),
        });
    }

    #[cfg(test)]
    pub(super) fn records(&self) -> impl Iterator<Item = (&AssetId, &str, usize, Option<&str>)> {
        self.records.iter().map(|record| {
            (
                &record.asset_id,
                record.binding_sha256.as_str(),
                record.presentation_round,
                record.semantic_stage.as_deref(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_presentations_are_bounded_transport_facts() {
        let mut ledger = RichInputPresentationLedger::default();
        ledger.record_verified(
            AssetId("asset-one".to_owned()),
            &"a".repeat(64),
            &"b".repeat(64),
            2,
            Some("read-source"),
        );
        ledger.record_verified(
            AssetId("asset-one".to_owned()),
            &"a".repeat(64),
            &"b".repeat(64),
            3,
            Some("rewritten-stage-name"),
        );

        let records = ledger.records().collect::<Vec<_>>();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0.0, "asset-one");
        assert_eq!(records[0].1, "b".repeat(64));
        assert_eq!(records[0].2, 2);
        assert_eq!(records[0].3, Some("read-source"));
        assert_eq!(
            ledger.prompt_policy(),
            RichInputPresentationPolicy::from_verified([(
                AssetId("asset-one".to_owned()),
                "a".repeat(64),
                "b".repeat(64),
            )])
            .with_retained_presentations()
        );
    }

    #[test]
    fn changed_identity_establishes_a_new_presentation_record() {
        let mut ledger = RichInputPresentationLedger::default();
        ledger.record_verified(
            AssetId("asset-one".to_owned()),
            &"a".repeat(64),
            &"b".repeat(64),
            1,
            None,
        );
        ledger.record_verified(
            AssetId("asset-one".to_owned()),
            &"c".repeat(64),
            &"b".repeat(64),
            2,
            Some("read-source"),
        );
        assert_eq!(ledger.records().count(), 2);
    }
}
