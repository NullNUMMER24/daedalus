//! The validated resource model: what a repository means once it has loaded.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::str::FromStr;

use ipnet::IpNet;

use crate::{ByteSize, CoreError, Name};

/// The only manifest schema version so far.
pub const API_VERSION: &str = "daedalus.io/v1alpha1";

/// Every kind of resource a manifest can declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Provider,
    Image,
    MachineClass,
    Tenant,
    Network,
    Machine,
}

impl Kind {
    pub const ALL: [Self; 6] = [
        Self::Provider,
        Self::Image,
        Self::MachineClass,
        Self::Tenant,
        Self::Network,
        Self::Machine,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "Provider",
            Self::Image => "Image",
            Self::MachineClass => "MachineClass",
            Self::Tenant => "Tenant",
            Self::Network => "Network",
            Self::Machine => "Machine",
        }
    }
}

impl FromStr for Kind {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| CoreError::UnknownKind(s.to_owned()))
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a resource lives, which is decided by its path in the repository.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// Shared by every tenant: providers and the catalog.
    Platform,
    Tenant(Name),
    Environment {
        tenant: Name,
        environment: Name,
    },
}

impl Scope {
    #[must_use]
    pub const fn tenant(&self) -> Option<&Name> {
        match self {
            Self::Platform => None,
            Self::Tenant(tenant) | Self::Environment { tenant, .. } => Some(tenant),
        }
    }
}

/// `platform`, `acme`, `acme/prod`.
impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Platform => f.write_str("platform"),
            Self::Tenant(tenant) => write!(f, "{tenant}"),
            Self::Environment {
                tenant,
                environment,
            } => write!(f, "{tenant}/{environment}"),
        }
    }
}

/// A resource's identity: unique within a repository.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceKey {
    pub scope: Scope,
    pub kind: Kind,
    pub name: Name,
}

/// `Provider/pve-main`, `Tenant/acme`, `Machine/acme/prod/web-01`.
impl fmt::Display for ResourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.scope {
            Scope::Platform => write!(f, "{}/{}", self.kind, self.name),
            // A Tenant's scope is itself; `Tenant/acme/acme` would say it twice.
            Scope::Tenant(_) if self.kind == Kind::Tenant => {
                write!(f, "{}/{}", self.kind, self.name)
            }
            scope => write!(f, "{}/{scope}/{}", self.kind, self.name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub name: Name,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    Proxmox,
}

/// A hypervisor or cluster Daedalus drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub metadata: Metadata,
    pub provider_type: ProviderType,
    pub endpoint: String,
}

/// A bootable image in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub metadata: Metadata,
    pub description: Option<String>,
}

/// A named machine shape in the catalog. Every field is a default that a
/// machine may override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineClass {
    pub metadata: Metadata,
    pub cpu_cores: Option<NonZeroU32>,
    pub memory: Option<ByteSize>,
    pub disks: Vec<Disk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    pub metadata: Metadata,
    pub display_name: Option<String>,
}

/// An L3 network within one environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub metadata: Metadata,
    pub provider: Name,
    pub cidr: IpNet,
    pub gateway: Option<IpAddr>,
}

/// A virtual machine, with its class already merged in: nothing is optional
/// here that the hypervisor needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub metadata: Metadata,
    pub provider: Name,
    pub class: Option<Name>,
    pub image: Name,
    pub cpu_cores: NonZeroU32,
    pub memory: ByteSize,
    pub disks: Vec<Disk>,
    pub networks: Vec<NetworkAttachment>,
    /// Explicit ordering dependencies, all in the machine's environment.
    pub depends_on: Vec<ResourceKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub name: Name,
    pub size: ByteSize,
    pub storage: Option<Name>,
    pub discard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAttachment {
    pub network: Name,
    /// A static address inside the network; `None` means DHCP.
    pub address: Option<IpAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Provider(Provider),
    Image(Image),
    MachineClass(MachineClass),
    Tenant(Tenant),
    Network(Network),
    Machine(Machine),
}

impl Resource {
    #[must_use]
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Provider(_) => Kind::Provider,
            Self::Image(_) => Kind::Image,
            Self::MachineClass(_) => Kind::MachineClass,
            Self::Tenant(_) => Kind::Tenant,
            Self::Network(_) => Kind::Network,
            Self::Machine(_) => Kind::Machine,
        }
    }

    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        match self {
            Self::Provider(r) => &r.metadata,
            Self::Image(r) => &r.metadata,
            Self::MachineClass(r) => &r.metadata,
            Self::Tenant(r) => &r.metadata,
            Self::Network(r) => &r.metadata,
            Self::Machine(r) => &r.metadata,
        }
    }
}

/// A repository that loaded without a single validation error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Repository {
    resources: BTreeMap<ResourceKey, Resource>,
}

impl Repository {
    pub(crate) const fn new(resources: BTreeMap<ResourceKey, Resource>) -> Self {
        Self { resources }
    }

    #[must_use]
    pub fn get(&self, key: &ResourceKey) -> Option<&Resource> {
        self.resources.get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ResourceKey, &Resource)> {
        self.resources.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    #[must_use]
    pub fn count(&self, kind: Kind) -> usize {
        self.resources.keys().filter(|key| key.kind == kind).count()
    }

    #[must_use]
    pub fn tenants(&self) -> BTreeSet<&Name> {
        self.resources
            .keys()
            .filter_map(|key| key.scope.tenant())
            .collect()
    }

    #[must_use]
    pub fn environments(&self) -> BTreeSet<(&Name, &Name)> {
        self.resources
            .keys()
            .filter_map(|key| match &key.scope {
                Scope::Environment {
                    tenant,
                    environment,
                } => Some((tenant, environment)),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> Name {
        Name::parse(s).unwrap()
    }

    #[test]
    fn kinds_round_trip_through_strings() {
        for kind in Kind::ALL {
            assert_eq!(kind.as_str().parse::<Kind>().unwrap(), kind);
        }
        assert!(
            "machine".parse::<Kind>().is_err(),
            "kinds are case-sensitive"
        );
    }

    #[test]
    fn keys_display_with_their_scope() {
        let key = |scope, kind, n| ResourceKey {
            scope,
            kind,
            name: name(n),
        };
        assert_eq!(
            key(Scope::Platform, Kind::Provider, "pve-main").to_string(),
            "Provider/pve-main"
        );
        assert_eq!(
            key(Scope::Tenant(name("acme")), Kind::Tenant, "acme").to_string(),
            "Tenant/acme"
        );
        assert_eq!(
            key(
                Scope::Environment {
                    tenant: name("acme"),
                    environment: name("prod")
                },
                Kind::Machine,
                "web-01"
            )
            .to_string(),
            "Machine/acme/prod/web-01"
        );
    }
}
