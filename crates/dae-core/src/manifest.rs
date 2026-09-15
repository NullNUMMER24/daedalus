//! Manifests as written: every field as it appears in YAML, with source spans.
//!
//! These types exist only between parsing and resolution. Optional fields a
//! class may fill in stay `Option` here; the resolved types in
//! [`crate::resource`] have none of them.

use std::collections::BTreeMap;
use std::net::IpAddr;

use ipnet::IpNet;
use serde::Deserialize;
use serde_saphyr::Spanned;

use crate::{ByteSize, Name};

/// Just enough of a document to know what it is. Unknown fields are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Header {
    #[serde(default)]
    pub(crate) api_version: Option<Spanned<String>>,
    #[serde(default)]
    pub(crate) kind: Option<Spanned<String>>,
}

/// The name of a document that failed to parse, read as leniently as
/// possible so references to it are not reported as missing as well.
#[derive(Debug, Deserialize)]
pub(crate) struct Identity {
    #[serde(default)]
    pub(crate) metadata: Option<IdentityMetadata>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IdentityMetadata {
    #[serde(default)]
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Manifest<S> {
    #[allow(dead_code, reason = "validated from the Header before this parse")]
    pub(crate) api_version: String,
    #[allow(dead_code, reason = "validated from the Header before this parse")]
    pub(crate) kind: String,
    pub(crate) metadata: MetadataSpec,
    pub(crate) spec: S,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetadataSpec {
    pub(crate) name: Spanned<Name>,
    /// Optional. Inferred from the path; if present it must agree.
    #[serde(default)]
    pub(crate) tenant: Option<Spanned<Name>>,
    /// Optional. Inferred from the path; if present it must agree.
    #[serde(default)]
    pub(crate) environment: Option<Spanned<Name>>,
    #[serde(default)]
    pub(crate) labels: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) annotations: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderSpec {
    #[serde(rename = "type")]
    pub(crate) provider_type: ProviderTypeSpec,
    pub(crate) endpoint: Spanned<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProviderTypeSpec {
    Proxmox,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImageSpec {
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MachineClassSpec {
    #[serde(default)]
    pub(crate) cpu: Option<CpuSpec>,
    #[serde(default)]
    pub(crate) memory: Option<Spanned<ByteSize>>,
    #[serde(default)]
    pub(crate) disks: Vec<DiskSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TenantSpec {
    #[serde(default)]
    pub(crate) display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NetworkSpec {
    pub(crate) provider: Spanned<Name>,
    pub(crate) cidr: Spanned<IpNet>,
    #[serde(default)]
    pub(crate) gateway: Option<Spanned<IpAddr>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct MachineSpec {
    pub(crate) provider: Spanned<Name>,
    pub(crate) image: Spanned<Name>,
    #[serde(default)]
    pub(crate) class: Option<Spanned<Name>>,
    #[serde(default)]
    pub(crate) cpu: Option<CpuSpec>,
    #[serde(default)]
    pub(crate) memory: Option<Spanned<ByteSize>>,
    #[serde(default)]
    pub(crate) disks: Vec<DiskSpec>,
    #[serde(default)]
    pub(crate) networks: Vec<AttachmentSpec>,
    /// `Kind/name`, e.g. `Network/prod-net`.
    #[serde(default)]
    pub(crate) depends_on: Vec<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CpuSpec {
    /// Checked in resolution rather than typed `NonZeroU32`, so the error can
    /// say what is wrong in words and point at the value.
    pub(crate) cores: Spanned<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiskSpec {
    pub(crate) name: Spanned<Name>,
    pub(crate) size: Spanned<ByteSize>,
    #[serde(default)]
    pub(crate) storage: Option<Name>,
    #[serde(default)]
    pub(crate) discard: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachmentSpec {
    #[serde(rename = "ref")]
    pub(crate) network: Spanned<Name>,
    #[serde(default)]
    pub(crate) address: Option<Spanned<IpAddr>>,
}
