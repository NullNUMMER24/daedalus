//! `dae_core::load` end to end, through the public API only.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a fixture that does not match should fail the test loudly"
)]

use std::collections::BTreeMap;

use dae_core::resource::{Machine, Network};
use dae_core::{
    Code, CoreError, Kind, Name, Repository, Resource, ResourceKey, Scope, Source, ValidationError,
    load,
};

const PROVIDER: &str = "platform/providers/pve-main.yaml";
const IMAGE: &str = "catalog/images/debian-12.yaml";
const CLASS: &str = "catalog/machine-classes/standard-4x8.yaml";
const TENANT: &str = "tenants/acme/tenant.yaml";
const NETWORK: &str = "tenants/acme/environments/prod/networks/prod-net.yaml";
const MACHINE: &str = "tenants/acme/environments/prod/machines/web-01.yaml";

/// A small, valid repository. Tests change one thing and look at the result.
fn base() -> BTreeMap<String, String> {
    [
        (
            PROVIDER,
            "\
apiVersion: daedalus.io/v1alpha1
kind: Provider
metadata:
  name: pve-main
spec:
  type: proxmox
  endpoint: https://pve.home.arpa:8006
",
        ),
        (
            IMAGE,
            "\
apiVersion: daedalus.io/v1alpha1
kind: Image
metadata:
  name: debian-12
spec:
  description: Debian 12 with cloud-init
",
        ),
        (
            CLASS,
            "\
apiVersion: daedalus.io/v1alpha1
kind: MachineClass
metadata:
  name: standard-4x8
spec:
  cpu:
    cores: 4
  memory: 8Gi
  disks:
    - name: root
      size: 40Gi
",
        ),
        (
            TENANT,
            "\
apiVersion: daedalus.io/v1alpha1
kind: Tenant
metadata:
  name: acme
spec:
  displayName: Acme Ltd
",
        ),
        (
            NETWORK,
            "\
apiVersion: daedalus.io/v1alpha1
kind: Network
metadata:
  name: prod-net
spec:
  provider: pve-main
  cidr: 10.20.10.0/24
  gateway: 10.20.10.1
",
        ),
        (MACHINE, &machine("web-01", "10.20.10.11")),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_owned(), text.to_owned()))
    .collect()
}

fn machine(name: &str, address: &str) -> String {
    format!(
        "\
apiVersion: daedalus.io/v1alpha1
kind: Machine
metadata:
  name: {name}
  labels:
    role: web
spec:
  provider: pve-main
  image: debian-12
  class: standard-4x8
  memory: 16Gi
  disks:
    - name: data
      size: 200Gi
  networks:
    - ref: prod-net
      address: {address}
  dependsOn:
    - Network/prod-net
"
    )
}

fn sources(files: &BTreeMap<String, String>) -> Vec<Source> {
    files
        .iter()
        .map(|(path, text)| Source::new(path.as_str(), text.as_str()))
        .collect()
}

fn ok(files: &BTreeMap<String, String>) -> Repository {
    match load(sources(files)) {
        Ok(repo) => repo,
        Err(CoreError::Validation(errors)) => {
            panic!("expected success, got:\n{}", describe(&errors))
        }
        Err(other) => panic!("unexpected error: {other}"),
    }
}

fn errors(files: &BTreeMap<String, String>) -> Vec<ValidationError> {
    match load(sources(files)) {
        Ok(_) => panic!("expected validation errors, but the repository loaded"),
        Err(CoreError::Validation(errors)) => errors,
        Err(other) => panic!("unexpected error: {other}"),
    }
}

/// The single error a change should produce.
fn only(files: &BTreeMap<String, String>) -> ValidationError {
    let errors = errors(files);
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one error, got:\n{}",
        describe(&errors)
    );
    errors.into_iter().next().unwrap()
}

fn describe(errors: &[ValidationError]) -> String {
    errors
        .iter()
        .map(|e| {
            format!(
                "  {} {}:{:?} {} (help: {:?})",
                e.code(),
                e.path(),
                e.line_col(),
                e,
                e.help_text()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Line and column of the first occurrence of `needle` in a file.
fn position(files: &BTreeMap<String, String>, path: &str, needle: &str) -> (usize, usize) {
    let text = &files[path];
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {path}"));
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (
        before.matches('\n').count() + 1,
        before[line_start..].chars().count() + 1,
    )
}

fn set(files: &mut BTreeMap<String, String>, path: &str, text: impl Into<String>) {
    files.insert(path.to_owned(), text.into());
}

fn replace(files: &mut BTreeMap<String, String>, path: &str, from: &str, to: &str) {
    let text = files.get_mut(path).unwrap();
    assert!(text.contains(from), "{from:?} not in {path}");
    *text = text.replacen(from, to, 1);
}

fn name(s: &str) -> Name {
    Name::parse(s).unwrap()
}

fn prod() -> Scope {
    Scope::Environment {
        tenant: name("acme"),
        environment: name("prod"),
    }
}

fn get_machine<'a>(repo: &'a Repository, n: &str) -> &'a Machine {
    let key = ResourceKey {
        scope: prod(),
        kind: Kind::Machine,
        name: name(n),
    };
    match repo.get(&key) {
        Some(Resource::Machine(machine)) => machine,
        other => panic!("{key}: {other:?}"),
    }
}

// ---------------------------------------------------------------- loading --

#[test]
fn loads_a_valid_repository() {
    let repo = ok(&base());
    assert_eq!(repo.len(), 6);
    for kind in Kind::ALL {
        assert_eq!(repo.count(kind), 1, "{kind}");
    }
    assert_eq!(
        repo.tenants()
            .into_iter()
            .map(Name::as_str)
            .collect::<Vec<_>>(),
        ["acme"]
    );
    assert_eq!(repo.environments().len(), 1);
}

#[test]
fn machines_merge_their_class() {
    let repo = ok(&base());
    let web = get_machine(&repo, "web-01");

    assert_eq!(web.cpu_cores.get(), 4, "from the class");
    assert_eq!(
        web.memory.to_string(),
        "16Gi",
        "the machine overrides the class"
    );
    let disks: Vec<_> = web
        .disks
        .iter()
        .map(|d| format!("{}={}", d.name, d.size))
        .collect();
    assert_eq!(
        disks,
        ["root=40Gi", "data=200Gi"],
        "class disks, then the machine's"
    );
    assert_eq!(
        web.networks[0].address,
        Some("10.20.10.11".parse().unwrap())
    );
    assert_eq!(web.depends_on[0].to_string(), "Network/acme/prod/prod-net");
    assert_eq!(web.metadata.labels["role"], "web");
}

#[test]
fn a_machine_without_a_class_sets_everything_itself() {
    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "  class: standard-4x8\n",
        "  cpu:\n    cores: 2\n",
    );
    let repo = ok(&files);
    let web = get_machine(&repo, "web-01");
    assert_eq!(web.cpu_cores.get(), 2);
    assert_eq!(web.disks.len(), 1);
}

#[test]
fn several_documents_in_one_file() {
    let mut files = base();
    let network = files.remove(NETWORK).unwrap();
    let web = files.remove(MACHINE).unwrap();
    set(
        &mut files,
        "tenants/acme/environments/prod/all.yaml",
        format!("{network}---\n{web}"),
    );
    ok(&files);
}

#[test]
fn the_same_name_in_another_environment_is_a_different_resource() {
    let mut files = base();
    let staging = |p: &str| p.replace("/prod/", "/staging/");
    let text = files[NETWORK].clone();
    set(&mut files, &staging(NETWORK), text);
    let text = files[MACHINE].clone();
    set(&mut files, &staging(MACHINE), text);
    let repo = ok(&files);
    assert_eq!(repo.count(Kind::Machine), 2);
    assert_eq!(repo.environments().len(), 2);
}

#[test]
fn every_problem_is_reported_in_file_order() {
    let mut files = base();
    replace(&mut files, MACHINE, "memory: 16Gi", "memroy: 16Gi");
    replace(&mut files, CLASS, "memory: 8Gi", "memory: 8GB");
    replace(
        &mut files,
        NETWORK,
        "cidr: 10.20.10.0/24",
        "cidr: 10.20.10.5/24",
    );

    let errors = errors(&files);
    let summary: Vec<_> = errors.iter().map(|e| (e.path(), e.code())).collect();
    assert_eq!(
        summary,
        [
            (CLASS, Code::InvalidValue),
            (MACHINE, Code::UnknownField),
            (NETWORK, Code::InvalidValue),
        ],
        "\n{}",
        describe(&errors)
    );
}

// ------------------------------------------------------------ YAML errors --

#[test]
fn unknown_field_points_at_the_key_and_suggests_the_right_one() {
    let mut files = base();
    replace(&mut files, MACHINE, "memory: 16Gi", "memroy: 16Gi");
    let err = only(&files);
    assert_eq!(err.code(), Code::UnknownField);
    assert_eq!(err.line_col(), Some(position(&files, MACHINE, "memroy")));
    assert_eq!(err.help_text(), Some("did you mean `memory`?"));
}

#[test]
fn unknown_nested_field_suggests_too() {
    let mut files = base();
    replace(&mut files, MACHINE, "size: 200Gi", "sise: 200Gi");
    let err = only(&files);
    assert_eq!(err.code(), Code::UnknownField);
    assert_eq!(err.help_text(), Some("did you mean `size`?"));
}

#[test]
fn unknown_provider_type_suggests_the_right_one() {
    let mut files = base();
    replace(&mut files, PROVIDER, "type: proxmox", "type: proxmx");
    let err = only(&files);
    assert_eq!(err.code(), Code::UnknownVariant);
    assert_eq!(err.help_text(), Some("did you mean `proxmox`?"));
}

#[test]
fn missing_required_field() {
    let mut files = base();
    replace(&mut files, MACHINE, "  image: debian-12\n", "");
    let err = only(&files);
    assert_eq!(err.code(), Code::MissingField);
    assert!(err.message().contains("image"), "{err}");
}

#[test]
fn invalid_values_point_at_the_value() {
    for (path, from, to, expect) in [
        (MACHINE, "name: web-01", "name: Web_01", "(try `web-01`)"),
        (CLASS, "memory: 8Gi", "memory: 8GB", "`GB` is not a unit"),
        (CLASS, "cores: 4", "cores: 0", "must be at least 1"),
        (
            NETWORK,
            "gateway: 10.20.10.1",
            "gateway: 10.20.10.300",
            "invalid IP address",
        ),
    ] {
        let mut files = base();
        replace(&mut files, path, from, to);
        let err = only(&files);
        assert_eq!(err.code(), Code::InvalidValue, "{to}: {err}");
        assert!(err.message().contains(expect), "{to}: {err}");
        let value = to.split_once(": ").unwrap().1;
        assert_eq!(err.line_col(), Some(position(&files, path, value)), "{to}");
    }
}

#[test]
fn only_true_and_false_are_booleans() {
    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "      size: 200Gi\n",
        "      size: 200Gi\n      discard: yes\n",
    );
    let err = only(&files);
    assert_eq!(
        err.line_col(),
        Some(position(&files, MACHINE, "yes")),
        "{err}"
    );
}

#[test]
fn duplicate_keys_are_errors() {
    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "  memory: 16Gi\n",
        "  memory: 16Gi\n  memory: 32Gi\n",
    );
    assert_eq!(only(&files).code(), Code::DuplicateKey);
}

#[test]
fn a_broken_document_does_not_hide_the_others() {
    let mut files = base();
    let web = files.remove(MACHINE).unwrap();
    let broken_syntax = "apiVersion: daedalus.io/v1alpha1\nkind: [unclosed\n";
    let unknown_field = machine("web-02", "10.20.10.12").replace("memory:", "memroy:");
    set(
        &mut files,
        "tenants/acme/environments/prod/machines/all.yaml",
        format!("{broken_syntax}---\n{web}---\n{unknown_field}"),
    );
    let codes: Vec<_> = errors(&files).iter().map(ValidationError::code).collect();
    assert_eq!(codes, [Code::Yaml, Code::UnknownField]);
}

// ---------------------------------------------------------------- headers --

#[test]
fn unknown_kind_suggests_the_right_one() {
    let mut files = base();
    replace(&mut files, MACHINE, "kind: Machine", "kind: Machnie");
    let err = only(&files);
    assert_eq!(err.code(), Code::UnknownKind);
    assert_eq!(err.help_text(), Some("did you mean `Machine`?"));
    assert_eq!(err.line_col(), Some(position(&files, MACHINE, "Machnie")));
}

#[test]
fn api_version_must_be_present_and_supported() {
    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "apiVersion: daedalus.io/v1alpha1\n",
        "",
    );
    assert_eq!(only(&files).code(), Code::MissingApiVersion);

    let mut files = base();
    replace(&mut files, MACHINE, "v1alpha1", "v2");
    assert_eq!(only(&files).code(), Code::UnsupportedApiVersion);
}

#[test]
fn kind_must_be_present() {
    let mut files = base();
    replace(&mut files, IMAGE, "kind: Image\n", "");
    let err = only(&files);
    assert_eq!(err.code(), Code::MissingKind);
    assert_eq!(
        err.line_col(),
        Some((1, 1)),
        "points at the start of the document"
    );

    // Same for a kind nobody recognises: still one error, not another for the
    // machine that uses the image.
    let mut files = base();
    replace(&mut files, IMAGE, "kind: Image", "kind: Imgae");
    assert_eq!(only(&files).code(), Code::UnknownKind);
}

#[test]
fn a_zero_core_machine_is_not_also_missing_its_cores() {
    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "  class: standard-4x8\n",
        "  cpu:\n    cores: 0\n",
    );
    assert_eq!(only(&files).message(), "`cpu.cores` must be at least 1");
}

// -------------------------------------------------------------- placement --

#[test]
fn files_outside_the_layout_are_rejected() {
    for path in [
        "machines/web-01.yaml",
        "tenants/acme/notes.yaml",
        "tenants/acme/environments/prod.yaml",
    ] {
        let mut files = base();
        set(&mut files, path, "kind: Machine\n");
        let err = only(&files);
        assert_eq!((err.code(), err.path()), (Code::Placement, path), "{err}");
        assert!(
            err.message().contains(path),
            "file-level messages name the file: {err}"
        );
    }
}

#[test]
fn directories_must_be_valid_names() {
    let mut files = base();
    let text = files.remove(TENANT).unwrap();
    set(&mut files, "tenants/Acme/tenant.yaml", text);
    let errors = errors(&files);
    let placement = errors
        .iter()
        .find(|e| e.code() == Code::Placement)
        .expect("placement error");
    assert!(placement.message().contains("(try `acme`)"), "{placement}");
}

#[test]
fn kinds_must_live_in_their_own_directory() {
    let mut files = base();
    let image = files.remove(IMAGE).unwrap();
    set(&mut files, "platform/images/debian-12.yaml", image);
    let errors = errors(&files);
    let placement = errors
        .iter()
        .find(|e| e.code() == Code::Placement)
        .expect("placement error");
    assert_eq!(
        placement.help_text(),
        Some("move this file under `catalog/`")
    );
    assert!(
        errors.iter().all(|e| e.code() == Code::Placement),
        "a misplaced image still counts as defined, so the machine is not also an error:\n{}",
        describe(&errors)
    );
}

#[test]
fn metadata_must_agree_with_the_path() {
    for (path, from, to) in [
        (
            MACHINE,
            "  name: web-01\n",
            "  name: web-01\n  tenant: bob\n",
        ),
        (
            MACHINE,
            "  name: web-01\n",
            "  name: web-01\n  environment: staging\n",
        ),
        (
            PROVIDER,
            "  name: pve-main\n",
            "  name: pve-main\n  tenant: acme\n",
        ),
    ] {
        let mut files = base();
        replace(&mut files, path, from, to);
        let err = only(&files);
        assert_eq!(err.code(), Code::ScopeMismatch, "{to}");
    }
    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "  name: web-01\n",
        "  name: web-01\n  tenant: acme\n  environment: prod\n",
    );
    ok(&files);
}

#[test]
fn a_tenant_is_named_after_its_directory() {
    let mut files = base();
    replace(&mut files, TENANT, "name: acme", "name: acme-corp");
    let errors = errors(&files);
    assert!(
        errors.iter().any(|e| e.code() == Code::ScopeMismatch),
        "{}",
        describe(&errors)
    );
}

#[test]
fn every_tenant_needs_a_tenant_manifest() {
    let mut files = base();
    files.remove(TENANT);
    let err = only(&files);
    assert_eq!((err.code(), err.path()), (Code::MissingTenant, TENANT));

    let mut files = base();
    set(&mut files, TENANT, "# empty\n");
    assert_eq!(only(&files).code(), Code::MissingTenant);
}

#[test]
fn a_broken_tenant_manifest_is_reported_once() {
    let mut files = base();
    replace(&mut files, TENANT, "displayName", "displayname");
    assert_eq!(only(&files).code(), Code::UnknownField);
}

// ------------------------------------------------------------- references --

#[test]
fn duplicates_point_at_the_first_definition() {
    let mut files = base();
    let copy = MACHINE.replace("web-01.yaml", "web-01-copy.yaml");
    set(&mut files, &copy, machine("web-01", "10.20.10.12"));
    let err = only(&files);
    assert_eq!((err.code(), err.path()), (Code::DuplicateResource, MACHINE));
    let (line, _) = position(&files, &copy, "web-01");
    assert_eq!(
        err.help_text(),
        Some(format!("first defined at {copy}:{line}").as_str())
    );
}

#[test]
fn unresolved_references_suggest_from_the_same_scope() {
    for (from, to, kind) in [
        ("ref: prod-net", "ref: prod-nett", "Network"),
        ("provider: pve-main", "provider: pve-mian", "Provider"),
        ("image: debian-12", "image: debian-21", "Image"),
        ("class: standard-4x8", "class: standard-4x9", "MachineClass"),
    ] {
        let mut files = base();
        replace(&mut files, MACHINE, from, to);
        let err = only(&files);
        assert_eq!(err.code(), Code::UnresolvedReference, "{to}");
        assert!(
            err.message().starts_with(&format!("no {kind} named")),
            "{err}"
        );
        let expected = from.split_once(": ").unwrap().1;
        assert_eq!(
            err.help_text(),
            Some(format!("did you mean `{expected}`?").as_str()),
            "{to}"
        );
        assert_eq!(
            err.line_col(),
            Some(position(&files, MACHINE, to.split_once(": ").unwrap().1))
        );
    }
}

/// An error in tenant acme must never reveal what tenant bob has named things.
#[test]
fn suggestions_never_reveal_another_tenants_resources() {
    let mut files = base();
    let text = files[TENANT].replace("acme", "bob");
    set(&mut files, "tenants/bob/tenant.yaml", text);
    let text = files[NETWORK].replace("name: prod-net", "name: prod-net2");
    set(
        &mut files,
        "tenants/bob/environments/prod/networks/prod-net2.yaml",
        text,
    );
    replace(&mut files, MACHINE, "ref: prod-net", "ref: prod-net2");

    let err = only(&files);
    assert_eq!(err.code(), Code::UnresolvedReference);
    let help = err.help_text().unwrap();
    assert!(
        !help.contains("prod-net2"),
        "leaked bob's network name: {help}"
    );
    assert_eq!(
        help, "did you mean `prod-net`?",
        "suggests only acme/prod's own network"
    );
}

#[test]
fn a_broken_resource_is_reported_once_not_at_every_reference() {
    let mut files = base();
    replace(&mut files, NETWORK, "cidr:", "cdir:");
    assert_eq!(only(&files).code(), Code::UnknownField);

    let mut files = base();
    replace(&mut files, CLASS, "cores: 4", "cores: many");
    assert_eq!(
        only(&files).code(),
        Code::InvalidValue,
        "and no missing cpu/memory on web-01"
    );
}

#[test]
fn missing_values_explain_where_they_could_come_from() {
    let mut files = base();
    replace(&mut files, CLASS, "  memory: 8Gi\n", "");
    replace(&mut files, MACHINE, "  memory: 16Gi\n", "");
    let err = only(&files);
    assert_eq!(err.code(), Code::MissingValue);
    assert_eq!(err.message(), "Machine `web-01` has no `spec.memory`");
    assert_eq!(
        err.help_text(),
        Some("class `standard-4x8` does not set it either: set `spec.memory` here or in the class")
    );

    let mut files = base();
    replace(&mut files, MACHINE, "  class: standard-4x8\n", "");
    let err = only(&files);
    assert_eq!(err.message(), "Machine `web-01` has no `spec.cpu.cores`");
    assert_eq!(
        err.help_text(),
        Some("set `spec.cpu.cores`, or `spec.class` to a class that provides it")
    );
}

// --------------------------------------------------------------- networks --

#[test]
fn networks_need_canonical_cidrs_and_a_usable_gateway() {
    let mut files = base();
    replace(
        &mut files,
        NETWORK,
        "cidr: 10.20.10.0/24",
        "cidr: 10.20.10.5/24",
    );
    let err = only(&files);
    assert_eq!(err.help_text(), Some("did you mean `10.20.10.0/24`?"));

    let mut files = base();
    replace(
        &mut files,
        NETWORK,
        "gateway: 10.20.10.1",
        "gateway: 10.20.99.1",
    );
    assert!(
        only(&files)
            .message()
            .contains("is outside `10.20.10.0/24`")
    );
}

#[test]
fn static_addresses_must_be_usable_and_unique() {
    for (address, code, expect) in [
        (
            "10.20.99.11",
            Code::InvalidValue,
            "is outside `10.20.10.0/24`",
        ),
        ("10.20.10.0", Code::InvalidValue, "network address"),
        ("10.20.10.255", Code::InvalidValue, "broadcast address"),
        ("10.20.10.1", Code::AddressConflict, "is the gateway"),
    ] {
        let mut files = base();
        set(&mut files, MACHINE, machine("web-01", address));
        let err = only(&files);
        assert_eq!(err.code(), code, "{address}: {err}");
        assert!(err.message().contains(expect), "{address}: {err}");
    }
}

#[test]
fn an_address_conflict_names_the_other_machine() {
    let mut files = base();
    let second = MACHINE.replace("web-01", "web-02");
    set(&mut files, &second, machine("web-02", "10.20.10.11"));
    let err = only(&files);
    assert_eq!(
        (err.code(), err.path()),
        (Code::AddressConflict, second.as_str())
    );
    let (line, _) = position(&files, MACHINE, "10.20.10.11");
    assert_eq!(
        err.help_text(),
        Some(format!("Machine `web-01` has it, at {MACHINE}:{line}").as_str())
    );
}

#[test]
fn the_same_address_on_another_network_is_fine() {
    let mut files = base();
    let staging = |p: &str| p.replace("/prod/", "/staging/");
    let text = files[NETWORK].clone();
    set(&mut files, &staging(NETWORK), text);
    let text = files[MACHINE].clone();
    set(&mut files, &staging(MACHINE), text);
    ok(&files);
}

#[test]
fn a_machine_attaches_only_to_networks_on_its_provider() {
    let mut files = base();
    let text = files[PROVIDER].replace("pve-main", "pve-edge");
    set(&mut files, "platform/providers/pve-edge.yaml", text);
    replace(
        &mut files,
        NETWORK,
        "provider: pve-main",
        "provider: pve-edge",
    );
    let err = only(&files);
    assert_eq!(
        err.message(),
        "network `prod-net` is on provider `pve-edge`, but this machine is on `pve-main`"
    );
}

#[test]
fn networks_are_attached_once_and_disks_listed_once() {
    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "      address: 10.20.10.11\n",
        "      address: 10.20.10.11\n    - ref: prod-net\n",
    );
    assert!(only(&files).message().contains("attached twice"));

    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "      size: 200Gi\n",
        "      size: 200Gi\n    - name: data\n      size: 1Gi\n",
    );
    assert!(only(&files).message().contains("listed twice"));

    let mut files = base();
    replace(&mut files, MACHINE, "size: 200Gi", "size: 0");
    assert!(only(&files).message().contains("size of zero"));
}

// ----------------------------------------------------------- dependencies --

#[test]
fn dependencies_name_a_machine_or_network_in_the_same_environment() {
    for (entry, code, expect) in [
        ("prod-net", Code::InvalidValue, "is not a dependency"),
        (
            "Netwrok/prod-net",
            Code::UnknownKind,
            "did you mean `Network`?",
        ),
        (
            "Image/debian-12",
            Code::InvalidValue,
            "cannot depend on a Image",
        ),
        (
            "Machine/web-01",
            Code::InvalidValue,
            "cannot depend on itself",
        ),
        (
            "Machine/db-01",
            Code::UnresolvedReference,
            "no Machine named `db-01` in `acme/prod`",
        ),
        ("Network/Prod", Code::InvalidValue, "invalid name `Prod`"),
    ] {
        let mut files = base();
        replace(
            &mut files,
            MACHINE,
            "- Network/prod-net",
            &format!("- {entry}"),
        );
        let err = only(&files);
        assert_eq!(err.code(), code, "{entry}: {err}");
        let text = format!("{} {}", err.message(), err.help_text().unwrap_or(""));
        assert!(text.contains(expect), "{entry}: {text}");
    }

    let mut files = base();
    replace(
        &mut files,
        MACHINE,
        "- Network/prod-net",
        "- Network/prod-net\n    - Network/prod-net",
    );
    assert!(only(&files).message().contains("listed twice"));
}

#[test]
fn provider_endpoints_must_be_https() {
    let mut files = base();
    replace(
        &mut files,
        PROVIDER,
        "https://pve.home.arpa:8006",
        "http://pve.home.arpa:8006",
    );
    let err = only(&files);
    assert_eq!(err.code(), Code::InvalidValue);
    assert_eq!(err.line_col(), Some(position(&files, PROVIDER, "http://")));
}

#[test]
fn ipv6_networks_work() {
    let mut files = base();
    replace(
        &mut files,
        NETWORK,
        "cidr: 10.20.10.0/24",
        "cidr: fd00:20::/64",
    );
    replace(
        &mut files,
        NETWORK,
        "gateway: 10.20.10.1",
        "gateway: fd00:20::1",
    );
    set(&mut files, MACHINE, machine("web-01", "fd00:20::11"));
    let repo = ok(&files);
    let key = ResourceKey {
        scope: prod(),
        kind: Kind::Network,
        name: name("prod-net"),
    };
    let Some(Resource::Network(Network { cidr, .. })) = repo.get(&key) else {
        panic!()
    };
    assert_eq!(cidr.to_string(), "fd00:20::/64");
}
