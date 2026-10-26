//! Coordination of image retrieval, prompt selection and confirmed presentation.

use super::{AppCore, AppCoreError, GenerationToolState, RichInputPresentationPolicy};
use ipc_types::AssetId;

impl GenerationToolState {
    pub(super) fn image_reinspection(
        &self,
        core: &AppCore,
        presented: &[AssetId],
    ) -> Result<bool, AppCoreError> {
        let aliases = self.rich_input_presentations.confirmed_inspections(
            core,
            self.operation_ledger.pending_image_ids(),
            presented,
        )?;
        Ok(self
            .operation_ledger
            .current_stage_has_image_reinspection(presented)
            || self
                .operation_ledger
                .current_stage_has_image_reinspection(&aliases))
    }

    pub(super) fn image_presentation_policy(&self) -> RichInputPresentationPolicy {
        self.rich_input_presentations
            .prompt_policy_with_inspections(self.operation_ledger.pending_image_ids())
    }

    pub(super) fn record_image_presentation(
        &mut self,
        core: &AppCore,
        asset_ids: &[AssetId],
        binding_sha256: &str,
        round: usize,
    ) -> Result<(), AppCoreError> {
        self.rich_input_presentations.record_presented(
            core,
            asset_ids,
            binding_sha256,
            round,
            self.plan_policy.current_stage_id(),
        )?;
        let confirmed = self.rich_input_presentations.confirmed_inspections(
            core,
            self.operation_ledger.pending_image_ids(),
            asset_ids,
        )?;
        self.operation_ledger.record_image_presentations(&confirmed);
        self.rich_input_presentations
            .record_inspection_delivery(confirmed.len(), asset_ids.len());
        Ok(())
    }
}
