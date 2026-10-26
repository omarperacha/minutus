use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;

struct FixedResolver {
    answers: Result<Vec<IpAddr>, ResolveError>,
    calls: AtomicUsize,
}

#[test]
fn request_identity_binds_method_url_and_fixed_broker_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let first = WebRequest::new(
        WebMethod::Get,
        CanonicalWebUrl::parse("https://public.example.com/report?item=1")?,
    );
    let same = WebRequest::new(
        WebMethod::Get,
        CanonicalWebUrl::parse("HTTPS://PUBLIC.EXAMPLE.COM:443/report?item=1")?,
    );
    let changed_method = WebRequest::new(WebMethod::Head, same.url().clone());
    let changed_url = WebRequest::new(
        WebMethod::Get,
        CanonicalWebUrl::parse("https://public.example.com/report?item=2")?,
    );

    assert_eq!(first.identity_sha256(), same.identity_sha256());
    assert_ne!(first.identity_sha256(), changed_method.identity_sha256());
    assert_ne!(first.identity_sha256(), changed_url.identity_sha256());
    assert_eq!(first.permission_descriptor().scope_id().len(), 64);
    Ok(())
}

impl FixedResolver {
    fn answers(answers: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            answers: Ok(answers.into_iter().collect()),
            calls: AtomicUsize::new(0),
        }
    }

    fn unavailable() -> Self {
        Self {
            answers: Err(ResolveError::Unavailable),
            calls: AtomicUsize::new(0),
        }
    }
}

impl HostResolver for FixedResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, ResolveError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let answers = self.answers.clone();
        Box::pin(async move { answers })
    }
}

#[test]
fn canonicalizes_https_origin_path_host_and_percent_hex() -> Result<(), Box<dyn std::error::Error>>
{
    let url = CanonicalWebUrl::parse("HTTPS://Example.COM:443/a/../b/%7e?q=%2f")?;

    assert_eq!(url.as_str(), "https://example.com/b/%7E?q=%2F");
    assert_eq!(url.origin(), "https://example.com");
    assert_eq!(url.host(), "example.com");
    assert_eq!(url.port(), 443);
    assert_eq!(url.literal_address(), None);
    Ok(())
}

#[test]
fn unicode_and_punycode_hosts_have_one_canonical_form() -> Result<(), Box<dyn std::error::Error>> {
    let unicode = CanonicalWebUrl::parse("https://bücher.example.com/")?;
    let punycode = CanonicalWebUrl::parse("https://xn--bcher-kva.example.com/")?;

    assert!(unicode == punycode);
    assert_eq!(unicode.as_str(), "https://xn--bcher-kva.example.com/");
    Ok(())
}

#[test]
fn rejects_unsupported_or_ambiguous_url_components() {
    let cases = [
        ("http://example.com/", UrlValidationError::UnsupportedScheme),
        ("ftp://example.com/", UrlValidationError::UnsupportedScheme),
        ("https:///path", UrlValidationError::MissingHost),
        (
            "https://user:password@example.com/",
            UrlValidationError::UserInfoNotAllowed,
        ),
        (
            "https://example.com/#section",
            UrlValidationError::FragmentNotAllowed,
        ),
        (
            "https://example.com:444/",
            UrlValidationError::UnsupportedPort,
        ),
        (
            "https://example.com:0443/",
            UrlValidationError::UnsupportedPort,
        ),
        ("https://example.com:/", UrlValidationError::UnsupportedPort),
        (
            "https://example.com:443:444/",
            UrlValidationError::AmbiguousSyntax,
        ),
        ("https://:443/", UrlValidationError::MissingHost),
        (
            "https://2606:4700:4700::1111/",
            UrlValidationError::AmbiguousSyntax,
        ),
        (
            "https://[2606:4700:4700::1111/",
            UrlValidationError::AmbiguousSyntax,
        ),
        ("https://example.com./", UrlValidationError::NonPublicDomain),
        ("https://localhost/", UrlValidationError::NonPublicDomain),
        (
            "https://service.local/",
            UrlValidationError::NonPublicDomain,
        ),
        ("https://example/", UrlValidationError::NonPublicDomain),
        (" https://example.com/", UrlValidationError::AmbiguousSyntax),
        (
            "https://example.com\\path",
            UrlValidationError::AmbiguousSyntax,
        ),
        (
            "https://example%2Ecom/",
            UrlValidationError::AmbiguousSyntax,
        ),
        (
            "https://[fe80::1%25en0]/",
            UrlValidationError::AmbiguousSyntax,
        ),
        (
            "https://example.com/%zz",
            UrlValidationError::AmbiguousSyntax,
        ),
        ("https://127.0.0.1/", UrlValidationError::NonPublicAddress),
        ("https://[::1]/", UrlValidationError::NonPublicAddress),
    ];

    for (input, expected) in cases {
        assert_eq!(
            CanonicalWebUrl::parse(input).err(),
            Some(expected),
            "{input}"
        );
    }
}

#[test]
fn rejects_special_use_domain_suffixes() {
    for domain in [
        "service.localhost",
        "service.local",
        "service.internal",
        "service.test",
        "service.invalid",
        "service.example",
        "service.onion",
        "service.arpa",
        "service.alt",
    ] {
        let input = format!("https://{domain}/");
        assert_eq!(
            CanonicalWebUrl::parse(&input).err(),
            Some(UrlValidationError::NonPublicDomain),
            "{domain}"
        );
    }
}

#[test]
fn rejects_alternate_numeric_ipv4_spellings() {
    for input in [
        "https://127.1/",
        "https://0177.0.0.1/",
        "https://0x7f.0.0.1/",
        "https://2130706433/",
        "https://001.002.003.004/",
    ] {
        assert_eq!(
            CanonicalWebUrl::parse(input).err(),
            Some(UrlValidationError::AmbiguousIpAddress),
            "{input}"
        );
    }
}

#[tokio::test]
async fn literal_target_is_validated_without_dns() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = FixedResolver::unavailable();
    let plan =
        validate_public_target(CanonicalWebUrl::parse("https://8.8.8.8/a")?, &resolver).await?;

    assert_eq!(resolver.calls.load(Ordering::Relaxed), 0);
    assert_eq!(plan.server_name(), "8.8.8.8");
    assert_eq!(
        plan.socket_addresses().collect::<Vec<_>>(),
        vec![SocketAddr::from(([8, 8, 8, 8], 443))]
    );
    Ok(())
}

#[tokio::test]
async fn public_ipv6_literal_retains_tls_name_and_skips_dns()
-> Result<(), Box<dyn std::error::Error>> {
    let resolver = FixedResolver::unavailable();
    let plan = validate_public_target(
        CanonicalWebUrl::parse("https://[2606:4700:4700::1111]:443/")?,
        &resolver,
    )
    .await?;

    assert_eq!(resolver.calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        plan.canonical_url().as_str(),
        "https://[2606:4700:4700::1111]/"
    );
    assert_eq!(plan.server_name(), "2606:4700:4700::1111");
    assert_eq!(
        plan.socket_addresses().collect::<Vec<_>>(),
        vec!["[2606:4700:4700::1111]:443".parse()?]
    );
    Ok(())
}

#[tokio::test]
async fn any_non_public_dns_answer_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = FixedResolver::answers([
        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    ]);

    let result = validate_public_target(
        CanonicalWebUrl::parse("https://public.example.com/")?,
        &resolver,
    )
    .await;

    assert_eq!(result.err(), Some(TargetValidationError::NonPublicAddress));
    assert_eq!(resolver.calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn connection_plan_deduplicates_and_stably_binds_all_public_answers()
-> Result<(), Box<dyn std::error::Error>> {
    let v4 = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
    let v6 = IpAddr::V6("2606:2800:220:1:248:1893:25c8:1946".parse()?);
    let resolver = FixedResolver::answers([v6, v4, v4]);

    let plan = validate_public_target(
        CanonicalWebUrl::parse("https://PUBLIC.example.com/report")?,
        &resolver,
    )
    .await?;

    assert_eq!(
        plan.canonical_url().as_str(),
        "https://public.example.com/report"
    );
    assert_eq!(plan.server_name(), "public.example.com");
    assert_eq!(
        plan.socket_addresses().collect::<Vec<_>>(),
        vec![SocketAddr::new(v4, 443), SocketAddr::new(v6, 443)]
    );
    Ok(())
}

#[tokio::test]
async fn empty_and_failed_resolution_have_distinct_sanitized_outcomes()
-> Result<(), Box<dyn std::error::Error>> {
    let url = CanonicalWebUrl::parse("https://public.example.com/")?;
    assert_eq!(
        validate_public_target(url.clone(), &FixedResolver::answers([]))
            .await
            .err(),
        Some(TargetValidationError::NoAddresses)
    );
    assert_eq!(
        validate_public_target(url, &FixedResolver::unavailable())
            .await
            .err(),
        Some(TargetValidationError::ResolutionFailed)
    );
    Ok(())
}

#[tokio::test]
async fn resolved_address_count_is_bounded_before_plan_construction()
-> Result<(), Box<dyn std::error::Error>> {
    let url = CanonicalWebUrl::parse("https://public.example.com/")?;
    let address = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));

    let boundary = validate_public_target(
        url.clone(),
        &FixedResolver::answers(vec![address; MAX_RESOLVED_ADDRESSES]),
    )
    .await?;
    assert_eq!(boundary.socket_addresses().len(), 1);
    assert_eq!(
        validate_public_target(
            url,
            &FixedResolver::answers(vec![address; MAX_RESOLVED_ADDRESSES + 1])
        )
        .await
        .err(),
        Some(TargetValidationError::TooManyAddresses)
    );
    Ok(())
}

#[test]
fn ipv4_public_policy_covers_exact_special_range_boundaries() {
    let rejected = [
        [0, 0, 0, 0],
        [10, 0, 0, 0],
        [10, 255, 255, 255],
        [100, 64, 0, 0],
        [100, 127, 255, 255],
        [127, 0, 0, 0],
        [127, 255, 255, 255],
        [169, 254, 0, 0],
        [169, 254, 255, 255],
        [172, 16, 0, 0],
        [172, 31, 255, 255],
        [192, 0, 0, 0],
        [192, 0, 0, 255],
        [192, 0, 2, 0],
        [192, 0, 2, 255],
        [192, 88, 99, 0],
        [192, 88, 99, 255],
        [192, 168, 0, 0],
        [192, 168, 255, 255],
        [198, 18, 0, 0],
        [198, 19, 255, 255],
        [198, 51, 100, 0],
        [198, 51, 100, 255],
        [203, 0, 113, 0],
        [203, 0, 113, 255],
        [224, 0, 0, 0],
        [239, 255, 255, 255],
        [240, 0, 0, 0],
        [255, 255, 255, 255],
    ];
    for octets in rejected {
        assert!(
            !is_public_web_address(IpAddr::V4(Ipv4Addr::from(octets))),
            "{octets:?}"
        );
    }

    let admitted = [
        [9, 255, 255, 255],
        [11, 0, 0, 0],
        [100, 63, 255, 255],
        [100, 128, 0, 0],
        [169, 253, 255, 255],
        [169, 255, 0, 0],
        [172, 15, 255, 255],
        [172, 32, 0, 0],
        [192, 0, 1, 0],
        [192, 0, 3, 0],
        [192, 88, 98, 255],
        [192, 88, 100, 0],
        [198, 17, 255, 255],
        [198, 20, 0, 0],
        [223, 255, 255, 255],
    ];
    for octets in admitted {
        assert!(
            is_public_web_address(IpAddr::V4(Ipv4Addr::from(octets))),
            "{octets:?}"
        );
    }
}

#[test]
fn ipv6_public_policy_rejects_special_transition_and_non_global_space()
-> Result<(), Box<dyn std::error::Error>> {
    let rejected = [
        "::",
        "::1",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
        "64:ff9b:1::1",
        "100::",
        "100::ffff:ffff:ffff:ffff",
        "100:0:0:1::",
        "2001::",
        "2001:1ff:ffff:ffff:ffff:ffff:ffff:ffff",
        "2001:db8::",
        "2002::",
        "3fff::",
        "3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff",
        "5f00::",
        "fc00::",
        "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "fe80::",
        "febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "ff00::",
        "4000::",
    ];
    for input in rejected {
        let address = input.parse::<Ipv6Addr>()?;
        assert!(!is_public_web_address(IpAddr::V6(address)), "{input}");
    }

    for input in [
        "2000::",
        "2000:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "2001:200::1",
        "2001:4860:4860::8888",
        "2fff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
    ] {
        let address = input.parse::<Ipv6Addr>()?;
        assert!(is_public_web_address(IpAddr::V6(address)), "{input}");
    }
    Ok(())
}

#[test]
fn url_length_boundary_is_exact() {
    let fixed = "https://example.com/";
    let accepted = format!("{fixed}{}", "a".repeat(MAX_WEB_URL_BYTES - fixed.len()));
    let rejected = format!("{accepted}a");

    assert!(CanonicalWebUrl::parse(&accepted).is_ok());
    assert_eq!(
        CanonicalWebUrl::parse(&rejected).err(),
        Some(UrlValidationError::InvalidLength)
    );
}
