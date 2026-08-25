//! Local network interfaces available to interactive AES67 applications.

use anyhow::Result;
use network::list_ipv4_interfaces;
use serde::Serialize;
use std::net::Ipv4Addr;

/// A named local IPv4 interface suitable for Send or Receive configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalInterface {
    pub name: String,
    pub address: String,
    pub is_loopback: bool,
}

/// Enumerate local interfaces with loopback first, followed by named adapters.
pub fn list_local_interfaces() -> Result<Vec<LocalInterface>> {
    let mut interfaces = list_ipv4_interfaces()?;

    if !interfaces
        .iter()
        .any(|interface| interface.ipv4 == Ipv4Addr::LOCALHOST)
    {
        interfaces.push(network::NetworkInterface {
            name: "loopback".to_string(),
            ipv4: Ipv4Addr::LOCALHOST,
            is_loopback: true,
        });
    }

    interfaces.sort_by(|left, right| {
        right
            .is_loopback
            .cmp(&left.is_loopback)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.ipv4.octets().cmp(&right.ipv4.octets()))
    });

    Ok(interfaces
        .into_iter()
        .map(|interface| LocalInterface {
            name: interface.name,
            address: interface.ipv4.to_string(),
            is_loopback: interface.is_loopback,
        })
        .collect())
}
