//! Bounded host-owned presentation policy for exact managed rich inputs.

use std::collections::BTreeSet;

use crate::{
    AppCore, AppCoreError, AssetId, Conversation, MessagePart, ToolOutcome, ToolResultPart,
};

const MAX_PRESENTED_ASSETS: usize = 64;

/// Prompt policy derived from request-local rich-input presentation facts.
///
/// This type carries no extraction, semantic claim, permission, or completion
/// state. It only prevents exact managed bytes already shown in this request
/// from being attached again automatically.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RichInputPresentationPolicy {
    presented_identities: Vec<(String, String, String)>,
    requested_asset_ids: BTreeSet<String>,
    retain_presented: bool,
}

impl RichInputPresentationPolicy {
    /// Builds a bounded policy from exact managed asset/digest pairs previously
    /// presented during this request. Invalid identities are ignored rather than
    /// broadening suppression.
    #[must_use]
    pub fn from_verified(identities: impl IntoIterator<Item = (AssetId, String, String)>) -> Self {
        Self {
            retain_presented: false,
            requested_asset_ids: BTreeSet::new(),
            presented_identities: identities
                .into_iter()
                .take(MAX_PRESENTED_ASSETS)
                .filter(|(asset_id, digest, binding)| {
                    !asset_id.0.is_empty()
                        && digest.len() == 64
                        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                        && binding.len() == 64
                        && binding.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .map(|(asset_id, digest, binding)| (asset_id.0, digest, binding))
                .collect(),
        }
    }

    /// Keeps this request's bounded source set available across advisory stages.
    /// Delivery alone is not extraction. Exact admission still enforces context
    /// limits; this does not restore unrelated images from earlier requests.
    #[must_use]
    pub fn with_retained_presentations(mut self) -> Self {
        self.retain_presented = true;
        self
    }

    pub(super) const fn retains_presentations(&self) -> bool {
        self.retain_presented
    }

    /// Requests bounded presentation of already-selected conversation assets.
    /// Explicit inspection and output-exhaustion recovery override historical
    /// suppression, never source validation or within-prompt deduplication.
    #[must_use]
    pub fn with_requested_images(mut self, asset_ids: impl IntoIterator<Item = AssetId>) -> Self {
        self.requested_asset_ids = asset_ids
            .into_iter()
            .take(MAX_PRESENTED_ASSETS)
            .map(|id| id.0)
            .collect();
        self
    }

    pub(super) fn extend_focus(
        &self,
        conversation: &Conversation,
        focus: &mut Option<BTreeSet<String>>,
    ) {
        extend_requested_focus(&self.requested_asset_ids, conversation, focus);
        if self.retain_presented {
            let retained = self
                .presented_identities
                .iter()
                .map(|(id, _, _)| id.clone())
                .collect();
            extend_requested_focus(&retained, conversation, focus);
        }
    }

    pub(super) fn resolve(
        &self,
        core: &AppCore,
        binding_sha256: &str,
    ) -> Result<ResolvedRichInputPresentationPolicy, AppCoreError> {
        self.resolve_candidates(core, Some(binding_sha256), std::iter::empty())
    }

    pub(super) fn resolve_for_candidates(
        &self,
        core: &AppCore,
        binding_sha256: &str,
        candidate_asset_ids: impl IntoIterator<Item = AssetId>,
    ) -> Result<ResolvedRichInputPresentationPolicy, AppCoreError> {
        self.resolve_candidates(core, Some(binding_sha256), candidate_asset_ids)
    }

    pub(super) fn request_projection_for_candidates(
        &self,
        core: &AppCore,
        candidate_asset_ids: impl IntoIterator<Item = AssetId>,
    ) -> Result<ResolvedRichInputPresentationPolicy, AppCoreError> {
        self.resolve_candidates(core, None, candidate_asset_ids)
    }

    fn resolve_candidates(
        &self,
        core: &AppCore,
        binding_sha256: Option<&str>,
        candidate_asset_ids: impl IntoIterator<Item = AssetId>,
    ) -> Result<ResolvedRichInputPresentationPolicy, AppCoreError> {
        let mut suppressed_asset_ids = BTreeSet::new();
        let mut presented_digests = BTreeSet::new();
        for (asset_id, presented_digest, presented_binding) in &self.presented_identities {
            if binding_sha256.is_some_and(|binding| presented_binding != binding) {
                continue;
            }
            let identity = core.rich_input_identity(&AssetId(asset_id.clone()))?;
            if identity.content_sha256 == *presented_digest {
                suppressed_asset_ids.insert(asset_id.clone());
                presented_digests.insert(presented_digest.clone());
            }
        }
        let candidates = candidate_asset_ids.into_iter().collect::<Vec<_>>();
        if self.retain_presented {
            // Earliest selected occurrence wins, including a newly requested
            // alias. Keep the image in its original history position for KV
            // reuse instead of moving it to every new inspection result.
            let mut seen = BTreeSet::new();
            suppressed_asset_ids.clear();
            let mut seen_ids = BTreeSet::new();
            for asset_id in candidates {
                if !seen_ids.insert(asset_id.0.clone()) {
                    continue;
                }
                let identity = core.rich_input_identity(&asset_id)?;
                if !seen.insert(identity.content_sha256) {
                    suppressed_asset_ids.insert(asset_id.0);
                }
            }
            // A requested alias must not defeat earliest-occurrence deduplication.
            let retained = self
                .presented_identities
                .iter()
                .map(|(id, _, _)| id.clone());
            return Ok(ResolvedRichInputPresentationPolicy {
                suppressed_asset_ids,
                requested_asset_ids: self
                    .requested_asset_ids
                    .iter()
                    .cloned()
                    .chain(retained)
                    .collect(),
            });
        }
        // Choose requested instances first. Otherwise an older alias can win
        // deduplication and hide the exact newly returned inspection result.
        let mut requested_digests = BTreeSet::new();
        let mut selected_requests = BTreeSet::new();
        for asset_id in candidates
            .iter()
            .rev()
            .filter(|id| self.requested_asset_ids.contains(&id.0))
        {
            let identity = core.rich_input_identity(asset_id)?;
            if requested_digests.insert(identity.content_sha256) {
                selected_requests.insert(asset_id.0.clone());
            }
        }
        let mut seen_digests = presented_digests;
        seen_digests.extend(requested_digests);
        let mut seen_asset_ids = BTreeSet::new();
        for asset_id in candidates {
            if !seen_asset_ids.insert(asset_id.0.clone()) {
                continue;
            }
            if selected_requests.contains(&asset_id.0) {
                suppressed_asset_ids.remove(&asset_id.0);
                continue;
            }
            let identity = core.rich_input_identity(&asset_id)?;
            if !seen_digests.insert(identity.content_sha256) {
                suppressed_asset_ids.insert(asset_id.0);
            }
        }
        Ok(ResolvedRichInputPresentationPolicy {
            suppressed_asset_ids,
            requested_asset_ids: self.requested_asset_ids.clone(),
        })
    }
}

pub(crate) struct ResolvedRichInputPresentationPolicy {
    suppressed_asset_ids: BTreeSet<String>,
    requested_asset_ids: BTreeSet<String>,
}

impl ResolvedRichInputPresentationPolicy {
    pub(super) fn extend_focus(
        &self,
        conversation: &Conversation,
        focus: &mut Option<BTreeSet<String>>,
    ) {
        extend_requested_focus(&self.requested_asset_ids, conversation, focus);
    }

    pub(super) fn suppresses(&self, asset_id: &AssetId) -> bool {
        self.suppressed_asset_ids.contains(&asset_id.0)
    }

    pub(super) fn marker(&self) -> Option<&'static str> {
        (!self.suppressed_asset_ids.is_empty()).then_some(
            "<rich_input_presentation trust=\"host\">Some duplicate or previously presented rich-input bytes are omitted from this round. This is a transport fact only: it does not assert comprehension, evidence, authority, or completion. A needed reinspection can be requested through the ordinary image-inspection tool within its reported allowance.</rich_input_presentation>",
        )
    }

    #[cfg(test)]
    pub(crate) fn exact_for_test(asset_ids: impl IntoIterator<Item = AssetId>) -> Self {
        Self {
            suppressed_asset_ids: asset_ids.into_iter().map(|asset_id| asset_id.0).collect(),
            requested_asset_ids: BTreeSet::new(),
        }
    }
}

fn extend_requested_focus(
    requested: &BTreeSet<String>,
    conversation: &Conversation,
    focus: &mut Option<BTreeSet<String>>,
) {
    let Some(focus) = focus else {
        return;
    };
    for part in conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
    {
        if let MessagePart::ToolResult { result } = part
            && let ToolOutcome::Completed { content } = &result.outcome
            && content.iter().any(|part| matches!(part, ToolResultPart::Asset { asset_id } if requested.contains(&asset_id.0)))
        {
            focus.insert(result.call_id.0.clone());
        }
    }
}

/// Verified transport identity for one immutable managed inference derivative.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RichInputIdentity {
    asset_id: AssetId,
    content_sha256: String,
}

impl RichInputIdentity {
    /// Exact opaque managed asset identifier.
    #[must_use]
    pub const fn asset_id(&self) -> &AssetId {
        &self.asset_id
    }

    /// Digest of the exact normalized bytes supplied to inference.
    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
}

impl AppCore {
    /// Verifies and returns the content-free identity of one managed rich input.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata or immutable inference bytes are absent or
    /// no longer match their recorded digest.
    pub fn rich_input_identity(
        &self,
        asset_id: &AssetId,
    ) -> Result<RichInputIdentity, AppCoreError> {
        let _ = self.inference_asset(asset_id)?;
        let asset = self
            .with_storage(|storage| storage.asset(asset_id))?
            .ok_or(AppCoreError::AssetNotFound)?;
        Ok(RichInputIdentity {
            asset_id: asset_id.clone(),
            content_sha256: asset.derivative.content_digest,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use tempfile::TempDir;

    use super::*;

    fn png() -> Result<Vec<u8>, image::ImageError> {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::from_pixel(1, 1, Rgb([1, 2, 3])))
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)?;
        Ok(bytes)
    }

    #[test]
    fn presentation_policy_suppresses_only_exact_recorded_assets() {
        let recorded = AssetId("asset-recorded".to_owned());
        let policy = ResolvedRichInputPresentationPolicy::exact_for_test([recorded.clone()]);
        assert!(policy.suppresses(&recorded));
        assert!(!policy.suppresses(&AssetId("asset-new".to_owned())));
        let marker = policy.marker().unwrap_or_default();
        assert!(marker.contains("transport fact only"));
        assert!(marker.contains("within its reported allowance"));
        assert!(!marker.contains("completed\":true"));
        assert!(!marker.contains("contentSha256"));
    }

    #[test]
    fn digest_mismatch_does_not_suppress_a_managed_asset() -> Result<(), Box<dyn std::error::Error>>
    {
        let temporary = TempDir::new()?;
        let app = AppCore::initialize(temporary.path().join("Zeta"))?;
        let asset = app.import_image(&png()?, "2026-08-30T12:00:00Z")?;
        let identity = app.rich_input_identity(&asset.id)?;
        let matching = RichInputPresentationPolicy::from_verified([(
            asset.id.clone(),
            identity.content_sha256().to_owned(),
            "b".repeat(64),
        )])
        .resolve(&app, &"b".repeat(64))?;
        assert!(matching.suppresses(&asset.id));

        let changed = RichInputPresentationPolicy::from_verified([(
            asset.id.clone(),
            "0".repeat(64),
            "b".repeat(64),
        )])
        .resolve(&app, &"b".repeat(64))?;
        assert!(!changed.suppresses(&asset.id));

        let changed_binding = RichInputPresentationPolicy::from_verified([(
            asset.id.clone(),
            identity.content_sha256().to_owned(),
            "b".repeat(64),
        )])
        .resolve(&app, &"c".repeat(64))?;
        assert!(!changed_binding.suppresses(&asset.id));
        Ok(())
    }

    #[test]
    fn exact_source_digest_is_presented_once_across_distinct_asset_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let app = AppCore::initialize(temporary.path().join("Zeta"))?;
        let first = app.import_image(&png()?, "2026-08-31T12:00:00Z")?;
        let second = app.import_image(&png()?, "2026-08-31T12:00:01Z")?;

        let projection = RichInputPresentationPolicy::default().resolve_for_candidates(
            &app,
            &"b".repeat(64),
            [first.id.clone(), second.id.clone()],
        )?;
        assert!(!projection.suppresses(&first.id));
        assert!(projection.suppresses(&second.id));

        let first_identity = app.rich_input_identity(&first.id)?;
        let after_presentation = RichInputPresentationPolicy::from_verified([(
            first.id.clone(),
            first_identity.content_sha256().to_owned(),
            "b".repeat(64),
        )])
        .resolve_for_candidates(&app, &"b".repeat(64), [second.id.clone()])?;
        assert!(after_presentation.suppresses(&second.id));
        Ok(())
    }

    #[test]
    fn retained_source_keeps_original_position_when_an_alias_is_requested()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let app = AppCore::initialize(temporary.path().join("Zeta"))?;
        let first = app.import_image(&png()?, "2026-09-03T00:00:00Z")?;
        let alias = app.import_image(&png()?, "2026-09-03T00:00:01Z")?;
        let identity = app.rich_input_identity(&first.id)?;
        let policy = RichInputPresentationPolicy::from_verified([(
            first.id.clone(),
            identity.content_sha256().to_owned(),
            "b".repeat(64),
        )])
        .with_retained_presentations()
        .with_requested_images([alias.id.clone()]);
        let resolved = policy.resolve_for_candidates(
            &app,
            &"b".repeat(64),
            [first.id.clone(), first.id.clone(), alias.id.clone()],
        )?;
        assert!(!resolved.suppresses(&first.id));
        assert!(resolved.suppresses(&alias.id));
        Ok(())
    }
}
