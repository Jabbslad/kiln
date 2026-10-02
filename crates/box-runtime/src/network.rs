use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{net::Ipv4Addr, path::PathBuf, process::Command};

pub const IFACE_ID: &str = "eth0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub helper: PathBuf,
    /// Fixed `boxd` scope: the host transit pool supports one networked store.
    /// Per-run ownership markers prevent adoption of another store's netns.
    pub namespace_scope: String,
    #[serde(default = "default_resolver")]
    pub resolver: Ipv4Addr,
}

fn default_resolver() -> Ipv4Addr {
    Ipv4Addr::new(1, 1, 1, 1)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub slot: u8,
    pub namespace: String,
    pub tap: String,
    pub guest_ipv4: String,
    pub gateway_ipv4: String,
    pub mac: String,
}

impl Topology {
    pub fn allocate(scope: &str, slot: u8) -> Result<Self> {
        if slot >= 8 {
            return Err(Error::Invalid("network slot pool exhausted".into()));
        }
        // The host bridge has one eight-address transit pool. A fixed scope
        // deliberately permits only one provisioned state store per host.
        if scope != "boxd" {
            return Err(Error::Invalid("invalid network namespace scope".into()));
        }
        Ok(Self {
            slot,
            namespace: format!("bd-{scope}-{slot}"),
            tap: "tap0".into(),
            guest_ipv4: format!("100.96.{slot}.2"),
            gateway_ipv4: format!("100.96.{slot}.1"),
            // Firecracker snapshots retain the anti-spoof MAC. Namespaces
            // isolate L2 domains, so clones must keep this stable identity.
            mac: "06:00:00:00:00:01".into(),
        })
    }

    pub fn validate(&self, scope: &str) -> Result<()> {
        if *self != Self::allocate(scope, self.slot)? {
            return Err(Error::Invalid("invalid network topology metadata".into()));
        }
        Ok(())
    }
}

pub fn public_ipv4(value: &str) -> bool {
    let Ok(ip) = value.parse::<Ipv4Addr>() else {
        return false;
    };
    let [a, b, _, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && matches!(b, 0 | 2 | 88 | 168))
        || (a == 198 && matches!(b, 18 | 19 | 51))
        || (a == 203 && b == 0)
        || a >= 224)
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if !self.helper.is_absolute()
            || Topology::allocate(&self.namespace_scope, 0).is_err()
            || !public_ipv4(&self.resolver.to_string())
        {
            return Err(Error::Invalid(
                "network helper must be absolute and resolver public IPv4".into(),
            ));
        }
        Ok(())
    }

    pub fn helper(
        &self,
        action: &str,
        owner: &str,
        topology: &Topology,
        jail_uid: u32,
    ) -> Result<()> {
        self.validate()?;
        topology.validate(&self.namespace_scope)?;
        if jail_uid == 0
            || !crate::storage::valid_id(owner)
            || !matches!(action, "prepare" | "activate" | "cleanup")
        {
            return Err(Error::Invalid("invalid network helper request".into()));
        }
        let status = Command::new(&self.helper)
            .args([
                action,
                owner,
                &topology.namespace,
                &topology.guest_ipv4,
                &topology.gateway_ipv4,
                &self.resolver.to_string(),
                &jail_uid.to_string(),
            ])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .status()?;
        if !status.success() {
            return Err(Error::Invalid(format!("network {action} failed")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_is_stable_and_does_not_overlap() {
        let a = Topology::allocate("boxd", 0).unwrap();
        let b = Topology::allocate("boxd", 7).unwrap();
        assert_eq!(a.guest_ipv4, "100.96.0.2");
        assert_eq!(a.gateway_ipv4, "100.96.0.1");
        assert_eq!(b.guest_ipv4, "100.96.7.2");
        assert_ne!(a.namespace, b.namespace);
        assert_eq!(
            a.mac, b.mac,
            "snapshot NIC identity must survive slot changes"
        );
    }

    #[test]
    fn topology_metadata_is_validated_as_command_input() {
        let mut topology = Topology::allocate("boxd", 0).unwrap();
        assert!(topology.validate("boxd").is_ok());
        topology.namespace = "other;true".into();
        assert!(topology.validate("boxd").is_err());
    }

    #[test]
    fn only_the_single_provisioned_scope_is_accepted() {
        assert!(Topology::allocate("boxd", 0).is_ok());
        assert!(Topology::allocate("store1", 0).is_err());
    }

    #[test]
    fn policy_rejects_non_public_destinations_and_ipv6() {
        for address in [
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
        ] {
            assert!(!public_ipv4(address), "accepted {address}");
        }
        assert!(public_ipv4("1.1.1.1"));
        assert!(!public_ipv4("2606:4700:4700::1111"));
    }
}
