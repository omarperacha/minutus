//! Exact retained public request binding, including the JSON body of a purchase.

use web_retrieval::WebMethod;
use x402_client::X402HttpMethod;

use super::{
    AppCore, PreparedX402Approval, PurchasedX402Content, TransactionProposalId, WalletControl,
    WebCancellation, WebRequest, X402ApprovalControl, X402ExecutionError, execute_paid_retry,
    revalidate_approved, sign_and_build_payment, validate_paid_response,
};

impl X402ApprovalControl {
    pub(crate) async fn execute_approved_once(
        &self,
        core: &AppCore,
        wallet: &WalletControl,
        review_id: &TransactionProposalId,
        request: &WebRequest,
        cancellation: &WebCancellation,
    ) -> Result<PurchasedX402Content, X402ExecutionError> {
        let approved = self
            .take_approved(review_id)
            .map_err(|_| X402ExecutionError::ApprovalUnavailable)?;
        revalidate_approved(core, &approved, review_id)?;
        if let Err(error) = validate_retained_request(approved.approval(), request) {
            super::invalidate_changed_approval(
                core,
                &approved,
                review_id,
                super::unix_now_for_execution()?,
                transaction_policy::ApprovalInvalidationReason::Superseded,
            )?;
            return Err(error);
        }
        if cancellation.is_cancelled() {
            super::invalidate_changed_approval(
                core,
                &approved,
                review_id,
                super::unix_now_for_execution()?,
                transaction_policy::ApprovalInvalidationReason::Cancelled,
            )?;
            return Err(X402ExecutionError::CancelledBeforeSubmission);
        }
        let payment =
            sign_and_build_payment(core, wallet, &approved, review_id, cancellation).await?;
        let response = execute_paid_retry(
            core,
            approved.approval(),
            review_id,
            request,
            payment,
            &web_retrieval::SystemResolver,
            cancellation,
        )
        .await?;
        validate_paid_response(core, approved.approval(), review_id, &response)
    }
}

fn validate_retained_request(
    approval: &PreparedX402Approval,
    request: &WebRequest,
) -> Result<(), X402ExecutionError> {
    let method = match request.method() {
        WebMethod::Get => X402HttpMethod::Get,
        WebMethod::Head => X402HttpMethod::Head,
        WebMethod::Post => X402HttpMethod::Post,
    };
    let candidate = approval.candidate();
    if candidate.origin_policy() != x402_client::X402OriginPolicy::PublicHttps
        || candidate.resource().canonical_url() != request.url().as_str()
        || candidate.method() != method
        || candidate.request_identity_sha256() != &request.identity_sha256()
    {
        return Err(X402ExecutionError::RequestBindingMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use web_retrieval::CanonicalWebUrl;

    #[test]
    fn mainnet_approval_binds_original_url_method_and_exact_post_body()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = "https://example.com/order";
        let request = WebRequest::json_post(
            CanonicalWebUrl::parse(url)?,
            br#"{"item":"sample"}"#.to_vec(),
        )?;
        let challenge = serde_json::json!({"x402Version":2,"resource":{"url":url},"accepts":[{
            "scheme":"exact","network":"eip155:8453","amount":"10000",
            "asset":"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            "payTo":"0x209693bc6afc0c5328ba36faf03c514ef312287c","maxTimeoutSeconds":60,
            "extra":{"name":"USD Coin","version":"2"}
        }]});
        let header = STANDARD.encode(serde_json::to_vec(&challenge)?);
        let challenge =
            x402_client::parse_payment_required_response(402, &[header.as_bytes()], url, 1000)?;
        let terms = x402_client::admit_payment_requirement(&challenge, 0, 1001)?;
        let account = x402_client::ActiveX402WalletAccount::new(
            "0x857b06519e91e3a54538791bdbb0e22373e36b66",
            chain_evm::EvmNetwork::BaseMainnet,
        )?;
        let candidate = x402_client::prepare_x402_payment_authorization(
            &terms,
            X402HttpMethod::Post,
            request.identity_sha256(),
            Some(&account),
            1001,
        )?;
        let approval = x402_client::prepare_x402_payment_approval(
            &candidate,
            "synthetic-session",
            None,
            1001,
        )?;
        validate_retained_request(&approval, &request).map_err(|_| "original binding rejected")?;
        assert!(approval.presentation().body().contains("8453"));
        assert!(
            !approval
                .presentation()
                .warnings()
                .contains(&x402_client::X402ApprovalWarning::Testnet)
        );
        for changed in [
            WebRequest::json_post(
                CanonicalWebUrl::parse(url)?,
                br#"{"item":"different"}"#.to_vec(),
            )?,
            WebRequest::new(WebMethod::Get, CanonicalWebUrl::parse(url)?),
            WebRequest::json_post(
                CanonicalWebUrl::parse("https://other.example.com/order")?,
                br#"{"item":"sample"}"#.to_vec(),
            )?,
        ] {
            assert!(validate_retained_request(&approval, &changed).is_err());
        }
        Ok(())
    }
}
