use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Returns whether an address is eligible for direct public web retrieval.
///
/// The policy is deliberately narrower than raw IP routability. It admits
/// ordinary IPv4 unicast and IPv6 `2000::/3` global unicast while rejecting all
/// IANA special-purpose, documentation, benchmark, transition, local and
/// multicast ranges. Globally reachable exceptions inside a special-purpose
/// parent range are also rejected to keep the SSRF boundary conservative.
#[must_use]
pub fn is_public_web_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    const BLOCKED: &[(Ipv4Addr, u8)] = &[
        (Ipv4Addr::UNSPECIFIED, 8),
        (Ipv4Addr::new(10, 0, 0, 0), 8),
        (Ipv4Addr::new(100, 64, 0, 0), 10),
        (Ipv4Addr::new(127, 0, 0, 0), 8),
        (Ipv4Addr::new(169, 254, 0, 0), 16),
        (Ipv4Addr::new(172, 16, 0, 0), 12),
        (Ipv4Addr::new(192, 0, 0, 0), 24),
        (Ipv4Addr::new(192, 0, 2, 0), 24),
        (Ipv4Addr::new(192, 88, 99, 0), 24),
        (Ipv4Addr::new(192, 168, 0, 0), 16),
        (Ipv4Addr::new(198, 18, 0, 0), 15),
        (Ipv4Addr::new(198, 51, 100, 0), 24),
        (Ipv4Addr::new(203, 0, 113, 0), 24),
        (Ipv4Addr::new(224, 0, 0, 0), 4),
        (Ipv4Addr::new(240, 0, 0, 0), 4),
    ];

    !BLOCKED
        .iter()
        .any(|(network, prefix)| ipv4_prefix_contains(*network, *prefix, address))
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    const GLOBAL_UNICAST: (Ipv6Addr, u8) = (Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3);
    const BLOCKED: &[(Ipv6Addr, u8)] = &[
        (Ipv6Addr::UNSPECIFIED, 128),
        (Ipv6Addr::LOCALHOST, 128),
        (Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0, 0), 96),
        (Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96),
        (Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
        (Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64),
        (Ipv6Addr::new(0x100, 0, 0, 1, 0, 0, 0, 0), 64),
        (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32),
        (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
        (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
        (Ipv6Addr::new(0x5f00, 0, 0, 0, 0, 0, 0, 0), 16),
        (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
        (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
        (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
    ];

    ipv6_prefix_contains(GLOBAL_UNICAST.0, GLOBAL_UNICAST.1, address)
        && !BLOCKED
            .iter()
            .any(|(network, prefix)| ipv6_prefix_contains(*network, *prefix, address))
}

fn ipv4_prefix_contains(network: Ipv4Addr, prefix: u8, address: Ipv4Addr) -> bool {
    let shift = 32_u32.saturating_sub(u32::from(prefix));
    u32::from(network) >> shift == u32::from(address) >> shift
}

fn ipv6_prefix_contains(network: Ipv6Addr, prefix: u8, address: Ipv6Addr) -> bool {
    let shift = 128_u32.saturating_sub(u32::from(prefix));
    u128::from(network) >> shift == u128::from(address) >> shift
}
