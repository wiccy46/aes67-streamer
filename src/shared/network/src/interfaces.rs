//! Cross-platform network-interface discovery for AES67 transports.

use anyhow::{Result, anyhow};
use netdev::get_interfaces;
use std::net::Ipv4Addr;

/// IPv4 address assigned to a local network interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInterface {
    /// Operating-system interface name, such as `en0`, `eth0`, or `lo0`.
    pub name: String,
    /// IPv4 address assigned to the interface.
    pub ipv4: Ipv4Addr,
    /// Whether the address belongs to an IPv4 loopback interface.
    pub is_loopback: bool,
}

/// Enumerate local IPv4 interfaces that can be shown in interactive settings.
pub fn list_ipv4_interfaces() -> Result<Vec<NetworkInterface>> {
    let mut found = Vec::new();

    for interface in get_interfaces() {
        for network in &interface.ipv4 {
            let ipv4 = network.addr();
            let candidate = NetworkInterface {
                name: interface.name.clone(),
                ipv4,
                is_loopback: interface.is_loopback() || ipv4.is_loopback(),
            };

            if !found.iter().any(|existing: &NetworkInterface| {
                existing.name == candidate.name && existing.ipv4 == candidate.ipv4
            }) {
                found.push(candidate);
            }
        }
    }

    found.sort_by(|left, right| {
        left.is_loopback
            .cmp(&right.is_loopback)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.ipv4.octets().cmp(&right.ipv4.octets()))
    });
    Ok(found)
}

/// Resolve an interface name or direct IPv4 address to an interface IPv4 address.
pub fn resolve_interface_ip(interface_name: &str) -> Result<Ipv4Addr> {
    let interface_name = interface_name.trim();
    if interface_name.is_empty() {
        return Err(anyhow!("Network interface cannot be empty"));
    }

    if let Ok(ip) = interface_name.parse::<Ipv4Addr>() {
        return Ok(ip);
    }

    if matches!(interface_name.to_lowercase().as_str(), "lo" | "loopback") {
        return Ok(Ipv4Addr::LOCALHOST);
    }

    list_ipv4_interfaces()?
        .into_iter()
        .find(|interface| interface.name == interface_name)
        .map(|interface| interface.ipv4)
        .ok_or_else(|| {
            anyhow!(
                "Network interface '{interface_name}' was not found; pass a valid interface name or IPv4 address"
            )
        })
}

/// Find the unicast MAC address associated with a local IPv4 address.
pub fn find_interface_mac_by_ipv4(interface_ip: Ipv4Addr) -> Option<[u8; 6]> {
    get_interfaces()
        .into_iter()
        .find(|interface| {
            interface
                .ipv4
                .iter()
                .any(|network| network.addr() == interface_ip)
        })
        .and_then(|interface| interface.mac_addr)
        .map(|mac| mac.octets())
        .filter(|mac| *mac != [0; 6] && mac[0] & 1 == 0)
}
