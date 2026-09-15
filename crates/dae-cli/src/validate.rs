//! `dae validate`: check a repository without touching any infrastructure.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, ensure};
use dae_core::{Code, CoreError, Kind, Repository, Source, ValidationError};
use miette::{GraphicalReportHandler, GraphicalTheme};
use walkdir::WalkDir;

/// The only directories manifests are read from.
const LAYOUT: [&str; 3] = ["platform", "catalog", "tenants"];

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Repository root
    #[arg(default_value = ".")]
    path: PathBuf,
}

pub(crate) fn run(args: &Args) -> anyhow::Result<ExitCode> {
    let root = &args.path;
    let metadata =
        std::fs::metadata(root).with_context(|| format!("cannot read `{}`", root.display()))?;
    ensure!(metadata.is_dir(), "`{}` is not a directory", root.display());

    let (sources, mut problems) = read(root)?;
    ensure!(
        !sources.is_empty() || !problems.is_empty(),
        "no manifests in `{}`: expected YAML files under `platform/`, `catalog/` or `tenants/`",
        root.display()
    );
    tracing::debug!(files = sources.len(), "read repository");

    match dae_core::load(sources) {
        Ok(repository) if problems.is_empty() => {
            print!("{}", summary(&repository));
            return Ok(ExitCode::SUCCESS);
        }
        Ok(_) => {}
        Err(CoreError::Validation(errors)) => problems.extend(errors),
        Err(other) => return Err(other.into()),
    }

    problems.sort_by(|a, b| a.path().cmp(b.path()));
    eprint!("{}", render(&problems)?);
    Ok(ExitCode::FAILURE)
}

/// Reads every `.yaml`/`.yml` file under the layout directories. Files that are
/// not UTF-8 become diagnostics rather than aborting the run.
fn read(root: &Path) -> anyhow::Result<(Vec<Source>, Vec<ValidationError>)> {
    let mut sources = Vec::new();
    let mut problems = Vec::new();

    for top in LAYOUT {
        let dir = root.join(top);
        if !dir.is_dir() {
            continue;
        }
        let entries = WalkDir::new(&dir)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| !is_hidden(entry.file_name()));

        for entry in entries {
            let entry = entry.with_context(|| format!("cannot read `{}`", dir.display()))?;
            if entry.file_type().is_symlink() {
                tracing::warn!(path = %entry.path().display(), "skipping symlink");
                continue;
            }
            let is_yaml = entry
                .path()
                .extension()
                .is_some_and(|ext| ext == "yaml" || ext == "yml");
            if !entry.file_type().is_file() || !is_yaml {
                continue;
            }

            let relative = relative_path(root, entry.path())?;
            let bytes = std::fs::read(entry.path())
                .with_context(|| format!("cannot read `{}`", entry.path().display()))?;
            match String::from_utf8(bytes) {
                Ok(text) => sources.push(Source::new(relative, text)),
                Err(_) => problems.push(ValidationError::file(
                    Code::Unreadable,
                    relative.as_str(),
                    format!("`{relative}` is not valid UTF-8"),
                )),
            }
        }
    }
    Ok((sources, problems))
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|n| n.starts_with('.'))
}

/// `tenants/acme/tenant.yaml`, with `/` on every platform.
fn relative_path(root: &Path, path: &Path) -> anyhow::Result<String> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts = relative
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(
                part.to_str()
                    .with_context(|| format!("`{}`: file name is not UTF-8", path.display())),
            ),
            _ => None,
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(parts.join("/"))
}

fn summary(repository: &Repository) -> String {
    let counts: Vec<String> = Kind::ALL
        .into_iter()
        .map(|kind| (kind, repository.count(kind)))
        .filter(|(_, count)| *count > 0)
        .map(|(kind, count)| plural(count, kind.as_str()))
        .collect();
    let tenants = repository.tenants().len();
    let environments = repository.environments().len();
    format!(
        "✓ {} in {}, {}\n  {}\n",
        plural(repository.len(), "resource"),
        plural(tenants, "tenant"),
        plural(environments, "environment"),
        counts.join(" · ")
    )
}

fn render(problems: &[ValidationError]) -> anyhow::Result<String> {
    let colour = std::io::stderr().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    let theme = if colour {
        GraphicalTheme::unicode()
    } else {
        GraphicalTheme::unicode_nocolor()
    };
    let handler = GraphicalReportHandler::new_themed(theme)
        .with_width(100)
        .with_links(false);

    let mut out = String::new();
    for problem in problems {
        handler
            .render_report(&mut out, problem)
            .context("rendering a diagnostic")?;
        out.push('\n');
    }
    let files: BTreeSet<_> = problems.iter().map(ValidationError::path).collect();
    writeln!(
        out,
        "✗ {} in {}",
        plural(problems.len(), "error"),
        plural(files.len(), "file")
    )?;
    Ok(out)
}

fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        _ if noun.ends_with('s') => format!("{count} {noun}es"),
        _ => format!("{count} {noun}s"),
    }
}
