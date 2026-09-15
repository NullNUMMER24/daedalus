//! Resolution: references, class merging, and every check that needs more
//! than one document to answer.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::ops::Range;

use ipnet::IpNet;

use crate::diagnostic::Code;
use crate::load::{Origin, Parsed, Poisoned, Spec};
use crate::manifest::{
    CpuSpec, DiskSpec, MachineClassSpec, MachineSpec, MetadataSpec, NetworkSpec, ProviderSpec,
    ProviderTypeSpec,
};
use crate::resource::{
    Disk, Image, Kind, Machine, MachineClass, Metadata, Network, NetworkAttachment, Provider,
    ProviderType, Resource, ResourceKey, Scope, Tenant,
};
use crate::suggest::{did_you_mean, list};
use crate::{Name, ValidationError};

pub(crate) fn resolve(
    parsed: &[Parsed],
    index: &BTreeMap<ResourceKey, usize>,
    poisoned: &Poisoned,
    errors: &mut Vec<ValidationError>,
) -> BTreeMap<ResourceKey, Resource> {
    let mut resolver = Resolver {
        index,
        poisoned,
        errors,
        classes: BTreeMap::new(),
        unsound_classes: BTreeSet::new(),
        networks: BTreeMap::new(),
        addresses: HashMap::new(),
        resolved: BTreeMap::new(),
    };

    // Kind order is dependency order: classes before the machines that use
    // them, networks before the machines attached to them. Duplicates are not
    // in the index, so only first definitions are resolved.
    let mut order: Vec<&Parsed> = index.values().map(|&i| &parsed[i]).collect();
    order.sort_by(|a, b| {
        (a.spec.kind(), &a.origin.file.path, a.origin.offset).cmp(&(
            b.spec.kind(),
            &b.origin.file.path,
            b.origin.offset,
        ))
    });
    for parsed in order {
        resolver.resolve(parsed);
    }
    resolver.resolved
}

enum Lookup {
    Found(ResourceKey),
    /// Defined, but failed to load; already reported.
    Broken,
    Missing,
}

struct Resolver<'a> {
    index: &'a BTreeMap<ResourceKey, usize>,
    poisoned: &'a Poisoned,
    errors: &'a mut Vec<ValidationError>,
    classes: BTreeMap<Name, MachineClass>,
    /// Classes with errors of their own. Machines using one are not told their
    /// values are missing: the class may simply have failed to provide them.
    unsound_classes: BTreeSet<Name>,
    networks: BTreeMap<ResourceKey, Network>,
    /// Static addresses handed out so far: per network, who has it and where.
    addresses: HashMap<(ResourceKey, IpAddr), (Name, String)>,
    resolved: BTreeMap<ResourceKey, Resource>,
}

impl Resolver<'_> {
    fn resolve(&mut self, parsed: &Parsed) {
        let key = parsed.key();
        let metadata = metadata(&parsed.metadata);
        let resource = match &parsed.spec {
            Spec::Provider(spec) => Resource::Provider(self.provider(parsed, metadata, spec)),
            Spec::Image(spec) => Resource::Image(Image {
                metadata,
                description: spec.description.clone(),
            }),
            Spec::MachineClass(spec) => {
                let before = self.errors.len();
                let class = self.machine_class(parsed, metadata, spec);
                if self.errors.len() > before {
                    self.unsound_classes.insert(key.name.clone());
                }
                self.classes.insert(key.name.clone(), class.clone());
                Resource::MachineClass(class)
            }
            Spec::Tenant(spec) => Resource::Tenant(Tenant {
                metadata,
                display_name: spec.display_name.clone(),
            }),
            Spec::Network(spec) => {
                let network = self.network(parsed, metadata, spec);
                self.networks.insert(key.clone(), network.clone());
                Resource::Network(network)
            }
            Spec::Machine(spec) => match self.machine(parsed, &key, metadata, spec) {
                Some(machine) => Resource::Machine(machine),
                None => return,
            },
        };
        self.resolved.insert(key, resource);
    }

    fn provider(&mut self, parsed: &Parsed, metadata: Metadata, spec: &ProviderSpec) -> Provider {
        let endpoint = &spec.endpoint.value;
        if endpoint.strip_prefix("https://").is_none_or(str::is_empty) {
            self.push(
                parsed
                    .origin
                    .error(
                        Code::InvalidValue,
                        format!(
                            "endpoint `{}` is not an https:// URL",
                            endpoint.escape_debug()
                        ),
                    )
                    .label(parsed.origin.span(&spec.endpoint), "")
                    .with_help(
                        "the Proxmox API is only served over HTTPS, usually on port 8006: \
                         `https://pve.example.lan:8006`",
                    ),
            );
        }
        Provider {
            metadata,
            provider_type: match spec.provider_type {
                ProviderTypeSpec::Proxmox => ProviderType::Proxmox,
            },
            endpoint: endpoint.clone(),
        }
    }

    fn machine_class(
        &mut self,
        parsed: &Parsed,
        metadata: Metadata,
        spec: &MachineClassSpec,
    ) -> MachineClass {
        if let Some(memory) = &spec.memory
            && memory.value.is_zero()
        {
            self.push(
                parsed
                    .origin
                    .error(Code::InvalidValue, "memory must be larger than zero")
                    .label(parsed.origin.span(memory), ""),
            );
        }
        MachineClass {
            metadata,
            cpu_cores: spec
                .cpu
                .as_ref()
                .and_then(|cpu| self.cores(&parsed.origin, cpu)),
            memory: spec.memory.as_ref().map(|m| m.value),
            disks: self.disks(&parsed.origin, &spec.disks),
        }
    }

    fn network(&mut self, parsed: &Parsed, metadata: Metadata, spec: &NetworkSpec) -> Network {
        let origin = &parsed.origin;
        self.lookup(
            origin,
            &Scope::Platform,
            Kind::Provider,
            &spec.provider.value,
            origin.span(&spec.provider),
        );

        let cidr = spec.cidr.value;
        if cidr != cidr.trunc() {
            self.push(
                origin
                    .error(Code::InvalidValue, format!("`{cidr}` has host bits set"))
                    .label(origin.span(&spec.cidr), "")
                    .with_help(format!("did you mean `{}`?", cidr.trunc())),
            );
        }
        if let Some(gateway) = &spec.gateway
            && let Some(problem) = address_problem(cidr, gateway.value)
        {
            self.push(
                origin
                    .error(Code::InvalidValue, format!("gateway {problem}"))
                    .label(origin.span(gateway), ""),
            );
        }

        Network {
            metadata,
            provider: spec.provider.value.clone(),
            cidr: cidr.trunc(),
            gateway: spec.gateway.as_ref().map(|g| g.value),
        }
    }

    fn machine(
        &mut self,
        parsed: &Parsed,
        key: &ResourceKey,
        metadata: Metadata,
        spec: &MachineSpec,
    ) -> Option<Machine> {
        let origin = &parsed.origin;
        let platform = Scope::Platform;
        let provider_found = matches!(
            self.lookup(
                origin,
                &platform,
                Kind::Provider,
                &spec.provider.value,
                origin.span(&spec.provider)
            ),
            Lookup::Found(_)
        );
        self.lookup(
            origin,
            &platform,
            Kind::Image,
            &spec.image.value,
            origin.span(&spec.image),
        );

        // A missing or broken class may well have supplied what looks missing,
        // so only report missing values when the class is known to be sound.
        let (class, class_sound) = match &spec.class {
            None => (None, true),
            Some(name) => match self.lookup(
                origin,
                &platform,
                Kind::MachineClass,
                &name.value,
                origin.span(name),
            ) {
                Lookup::Found(found) => (
                    self.classes.get(&found.name).cloned(),
                    !self.unsound_classes.contains(&found.name),
                ),
                Lookup::Broken | Lookup::Missing => (None, false),
            },
        };

        // Set-but-invalid (already reported) is not the same as not set.
        let own_cores = spec.cpu.as_ref().map(|cpu| self.cores(origin, cpu));
        let cpu_cores = match own_cores {
            Some(own) => own,
            None => class.as_ref().and_then(|c| c.cpu_cores),
        };
        let memory = spec
            .memory
            .as_ref()
            .map(|m| m.value)
            .or_else(|| class.as_ref().and_then(|c| c.memory));
        if let Some(own) = &spec.memory
            && own.value.is_zero()
        {
            self.push(
                origin
                    .error(Code::InvalidValue, "memory must be larger than zero")
                    .label(origin.span(own), ""),
            );
        }
        if class_sound {
            let no_cores = own_cores.is_none() && cpu_cores.is_none();
            self.missing_values(parsed, spec, no_cores, memory.is_none());
        }

        let own_disks = self.disks(origin, &spec.disks);
        let disks = merge_disks(class.map(|c| c.disks).unwrap_or_default(), own_disks);
        let networks = self.attachments(parsed, key, spec, provider_found);
        let depends_on = self.dependencies(parsed, key, spec);

        Some(Machine {
            metadata,
            provider: spec.provider.value.clone(),
            class: spec.class.as_ref().map(|c| c.value.clone()),
            image: spec.image.value.clone(),
            cpu_cores: cpu_cores?,
            memory: memory?,
            disks,
            networks,
            depends_on,
        })
    }

    fn missing_values(
        &mut self,
        parsed: &Parsed,
        spec: &MachineSpec,
        no_cpu: bool,
        no_memory: bool,
    ) {
        let name = &parsed.metadata.name;
        for (field, missing) in [("cpu.cores", no_cpu), ("memory", no_memory)] {
            if !missing {
                continue;
            }
            let help = match &spec.class {
                Some(class) => format!(
                    "class `{}` does not set it either: set `spec.{field}` here or in the class",
                    class.value
                ),
                None => format!("set `spec.{field}`, or `spec.class` to a class that provides it"),
            };
            self.push(
                parsed
                    .origin
                    .error(
                        Code::MissingValue,
                        format!("Machine `{}` has no `spec.{field}`", name.value),
                    )
                    .label(parsed.origin.span(name), "")
                    .with_help(help),
            );
        }
    }

    fn cores(&mut self, origin: &Origin, cpu: &CpuSpec) -> Option<NonZeroU32> {
        let cores = NonZeroU32::new(cpu.cores.value);
        if cores.is_none() {
            self.push(
                origin
                    .error(Code::InvalidValue, "`cpu.cores` must be at least 1")
                    .label(origin.span(&cpu.cores), ""),
            );
        }
        cores
    }

    fn disks(&mut self, origin: &Origin, specs: &[DiskSpec]) -> Vec<Disk> {
        let mut first_seen: HashMap<&Name, Option<Range<usize>>> = HashMap::new();
        let mut disks = Vec::new();
        for disk in specs {
            let span = origin.span(&disk.name);
            if let Some(first) = first_seen.get(&disk.name.value) {
                self.push(
                    origin
                        .error(
                            Code::InvalidValue,
                            format!("disk `{}` is listed twice", disk.name.value),
                        )
                        .label(span, "listed again here")
                        .with_help(format!("first listed at {}", origin.locate(first.as_ref()))),
                );
                continue;
            }
            first_seen.insert(&disk.name.value, span);
            if disk.size.value.is_zero() {
                self.push(
                    origin
                        .error(
                            Code::InvalidValue,
                            format!("disk `{}` has a size of zero", disk.name.value),
                        )
                        .label(origin.span(&disk.size), ""),
                );
            }
            disks.push(Disk {
                name: disk.name.value.clone(),
                size: disk.size.value,
                storage: disk.storage.clone(),
                discard: disk.discard,
            });
        }
        disks
    }

    fn attachments(
        &mut self,
        parsed: &Parsed,
        machine: &ResourceKey,
        spec: &MachineSpec,
        provider_found: bool,
    ) -> Vec<NetworkAttachment> {
        let origin = &parsed.origin;
        let mut first_seen: HashMap<&Name, Option<Range<usize>>> = HashMap::new();
        let mut attachments = Vec::new();

        for attachment in &spec.networks {
            let name = &attachment.network.value;
            let span = origin.span(&attachment.network);
            if let Some(first) = first_seen.get(name) {
                self.push(
                    origin
                        .error(
                            Code::InvalidValue,
                            format!("network `{name}` is attached twice"),
                        )
                        .label(span, "attached again here")
                        .with_help(format!(
                            "first attached at {}",
                            origin.locate(first.as_ref())
                        )),
                );
                continue;
            }
            first_seen.insert(name, span.clone());

            if let Lookup::Found(key) =
                self.lookup(origin, &parsed.scope, Kind::Network, name, span.clone())
                && let Some(network) = self.networks.get(&key).cloned()
            {
                if provider_found && network.provider != spec.provider.value {
                    self.push(
                        origin
                            .error(
                                Code::InvalidValue,
                                format!(
                                    "network `{name}` is on provider `{}`, but this machine is on `{}`",
                                    network.provider, spec.provider.value
                                ),
                            )
                            .label(span, "")
                            .with_help("a machine can only attach to networks on its own provider"),
                    );
                }
                if let Some(address) = &attachment.address {
                    self.address(origin, machine, &key, &network, address);
                }
            }

            attachments.push(NetworkAttachment {
                network: name.clone(),
                address: attachment.address.as_ref().map(|a| a.value),
            });
        }
        attachments
    }

    /// A static address must be a usable host address on its network, and
    /// nobody else's.
    fn address(
        &mut self,
        origin: &Origin,
        machine: &ResourceKey,
        network_key: &ResourceKey,
        network: &Network,
        address: &serde_saphyr::Spanned<IpAddr>,
    ) {
        let span = origin.span(address);
        let ip = address.value;
        let network_name = &network.metadata.name;

        if let Some(problem) = address_problem(network.cidr, ip) {
            self.push(
                origin
                    .error(Code::InvalidValue, format!("address {problem}"))
                    .label(span, "")
                    .with_help(format!("network `{network_name}` is `{}`", network.cidr)),
            );
        } else if network.gateway == Some(ip) {
            self.push(
                origin
                    .error(
                        Code::AddressConflict,
                        format!("`{ip}` is the gateway of network `{network_name}`"),
                    )
                    .label(span, ""),
            );
        } else if let Some((owner, at)) = self.addresses.get(&(network_key.clone(), ip)) {
            self.push(
                origin
                    .error(
                        Code::AddressConflict,
                        format!("`{ip}` is already assigned on network `{network_name}`"),
                    )
                    .label(span, "assigned again here")
                    .with_help(format!("Machine `{owner}` has it, at {at}")),
            );
        } else {
            self.addresses.insert(
                (network_key.clone(), ip),
                (machine.name.clone(), origin.locate(span.as_ref())),
            );
        }
    }

    fn dependencies(
        &mut self,
        parsed: &Parsed,
        machine: &ResourceKey,
        spec: &MachineSpec,
    ) -> Vec<ResourceKey> {
        let origin = &parsed.origin;
        let mut dependencies = Vec::new();

        for entry in &spec.depends_on {
            let span = origin.span(entry);
            let raw = entry.value.as_str();
            let invalid = |message: String| {
                origin
                    .error(Code::InvalidValue, message)
                    .label(span.clone(), "")
            };

            let Some((kind, name)) = raw.split_once('/') else {
                let err = invalid(format!("`{}` is not a dependency", raw.escape_debug()))
                    .with_help("write `Kind/name`, e.g. `Network/prod-net` or `Machine/db-01`");
                self.push(err);
                continue;
            };
            let kind = match kind.parse::<Kind>() {
                Ok(kind @ (Kind::Machine | Kind::Network)) => kind,
                Ok(other) => {
                    let err = invalid(format!("a Machine cannot depend on a {other}")).with_help(
                        "dependencies order resources within an environment: name a Machine or \
                         Network. Providers, images and classes are always resolved first",
                    );
                    self.push(err);
                    continue;
                }
                Err(_) => {
                    let help = did_you_mean(kind, ["Machine", "Network"]).map_or_else(
                        || "a Machine can depend on a `Machine` or a `Network`".to_owned(),
                        |best| format!("did you mean `{best}`?"),
                    );
                    let err = origin
                        .error(
                            Code::UnknownKind,
                            format!("unknown kind `{}`", kind.escape_debug()),
                        )
                        .label(span.clone(), "")
                        .with_help(help);
                    self.push(err);
                    continue;
                }
            };
            let name = match Name::parse(name) {
                Ok(name) => name,
                Err(err) => {
                    let err = invalid(err.to_string());
                    self.push(err);
                    continue;
                }
            };

            let target = ResourceKey {
                scope: parsed.scope.clone(),
                kind,
                name,
            };
            if target == *machine {
                let err = invalid("a Machine cannot depend on itself".to_owned());
                self.push(err);
            } else if dependencies.contains(&target) {
                let err = invalid(format!("`{raw}` is listed twice"));
                self.push(err);
            } else {
                match self.lookup(origin, &parsed.scope, kind, &target.name, span) {
                    Lookup::Found(_) | Lookup::Broken => dependencies.push(target),
                    Lookup::Missing => {}
                }
            }
        }
        dependencies
    }

    /// Finds a resource, reporting it if it does not exist. Suggestions only
    /// ever come from the same scope: an error in one tenant must not reveal
    /// the names of another tenant's resources.
    fn lookup(
        &mut self,
        origin: &Origin,
        scope: &Scope,
        kind: Kind,
        name: &Name,
        span: Option<Range<usize>>,
    ) -> Lookup {
        let key = ResourceKey {
            scope: scope.clone(),
            kind,
            name: name.clone(),
        };
        if self.index.contains_key(&key) {
            return Lookup::Found(key);
        }
        if self
            .poisoned
            .contains(&(scope.clone(), kind, name.to_string()))
        {
            return Lookup::Broken;
        }

        let index = self.index;
        let candidates: Vec<&str> = index
            .keys()
            .filter(|k| k.scope == *scope && k.kind == kind)
            .map(|k| k.name.as_str())
            .collect();
        let place = match scope {
            Scope::Platform => String::new(),
            scope => format!(" in `{scope}`"),
        };
        let help = match did_you_mean(name.as_str(), candidates.iter().copied()) {
            Some(best) => format!("did you mean `{best}`?"),
            None if candidates.is_empty() => format!("no {kind} is defined{place}"),
            None => format!("defined{place}: {}", list(&candidates, 8)),
        };
        self.push(
            origin
                .error(
                    Code::UnresolvedReference,
                    format!("no {kind} named `{name}`{place}"),
                )
                .label(span, "not found")
                .with_help(help),
        );
        Lookup::Missing
    }

    fn push(&mut self, error: ValidationError) {
        self.errors.push(error);
    }
}

fn metadata(spec: &MetadataSpec) -> Metadata {
    Metadata {
        name: spec.name.value.clone(),
        labels: spec.labels.clone(),
        annotations: spec.annotations.clone(),
    }
}

/// A class's disks, with the machine's own disks replacing any of the same
/// name and the rest appended — so a machine can resize `root` and add `data`
/// without restating everything.
fn merge_disks(class: Vec<Disk>, own: Vec<Disk>) -> Vec<Disk> {
    let mut merged = class;
    for disk in own {
        match merged.iter_mut().find(|d| d.name == disk.name) {
            Some(slot) => *slot = disk,
            None => merged.push(disk),
        }
    }
    merged
}

/// Why `ip` cannot be a host address on `cidr`, if it cannot.
fn address_problem(cidr: IpNet, ip: IpAddr) -> Option<String> {
    let net = cidr.trunc();
    if !net.contains(&ip) {
        return Some(format!("`{ip}` is outside `{net}`"));
    }
    // Point-to-point prefixes (/31, /32, /127, /128) have no reserved addresses.
    let reserves = net.prefix_len() + 1 < net.max_prefix_len();
    if reserves && ip == net.network() {
        return Some(format!("`{ip}` is the network address of `{net}`"));
    }
    if reserves && matches!(net, IpNet::V4(_)) && ip == net.broadcast() {
        return Some(format!("`{ip}` is the broadcast address of `{net}`"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(name: &str, gi: u64) -> Disk {
        Disk {
            name: Name::parse(name).unwrap(),
            size: crate::ByteSize::from_bytes(gi << 30),
            storage: None,
            discard: false,
        }
    }

    #[test]
    fn machine_disks_replace_by_name_and_append_the_rest() {
        let merged = merge_disks(
            vec![disk("root", 40), disk("swap", 4)],
            vec![disk("data", 200), disk("root", 80)],
        );
        let summary: Vec<_> = merged
            .iter()
            .map(|d| (d.name.as_str(), d.size.to_string()))
            .collect();
        assert_eq!(
            summary,
            [
                ("root", "80Gi".into()),
                ("swap", "4Gi".into()),
                ("data", "200Gi".into())
            ]
        );
    }

    #[test]
    fn host_addresses() {
        let net: IpNet = "10.20.10.0/24".parse().unwrap();
        let problem = |ip: &str| address_problem(net, ip.parse().unwrap());
        assert_eq!(problem("10.20.10.11"), None);
        assert_eq!(
            problem("10.20.11.1").unwrap(),
            "`10.20.11.1` is outside `10.20.10.0/24`"
        );
        assert!(problem("10.20.10.0").unwrap().contains("network address"));
        assert!(
            problem("10.20.10.255")
                .unwrap()
                .contains("broadcast address")
        );

        let p2p: IpNet = "10.0.0.0/31".parse().unwrap();
        assert_eq!(address_problem(p2p, "10.0.0.0".parse().unwrap()), None);

        let v6: IpNet = "fd00:20::/64".parse().unwrap();
        assert_eq!(address_problem(v6, "fd00:20::11".parse().unwrap()), None);
        assert!(address_problem(v6, "fd00:20::".parse().unwrap()).is_some());
    }
}
