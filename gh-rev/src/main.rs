use std::{
    collections::BTreeMap,
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

/// Stores the fast, tool-owned registry that points commands at durable review
/// artifacts. Markdown remains the source of review truth and can repair this
/// index after state loss.
const STATE_FILE: &str = "state.toml";
/// Serializes state replacement and review-number allocation so concurrent
/// agents cannot assign the same artifact name or lose registry updates.
const LOCK_FILE: &str = ".gh-rev.lock";
const LOCK_RETRY_ATTEMPTS: usize = 50;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(5);
const APP_VERSION: &str = env!("GH_REV_BUILD_VERSION");
const APP_GIT_COMMIT: &str = env!("GH_REV_BUILD_COMMIT");
const APP_GIT_DIRTY: &str = env!("GH_REV_BUILD_DIRTY");

#[derive(Parser)]
#[command(
    about = "Persistent local review ledger",
    disable_version_flag = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Override the default ~/gh workspace root.
    #[arg(long, global = true, env = "GH_REV_HOME")]
    home: Option<PathBuf>,

    /// Print build/package provenance as machine-readable JSON.
    #[arg(long, global = true)]
    version: bool,

    /// Run the command as if invoked from a different path.
    #[arg(short = 'C', long, global = true)]
    path: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<CommandName>,
}

#[derive(Debug)]
struct RepoDiscovery {
    slug: String,
    source: String,
    worktree_root: PathBuf,
    git_common_dir: PathBuf,
    branch: Option<String>,
    remote_name: String,
    remote: String,
    remote_source: String,
}

#[derive(Serialize)]
struct VersionOutput {
    version: &'static str,
    commit: &'static str,
    dirty: Option<bool>,
}

#[derive(Subcommand)]
enum CommandName {
    /// Register a local Git worktree under its remote-derived slug.
    Register { path: PathBuf },
    /// Resolve or create a local branch review target.
    Open {
        repo: Option<String>,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long, conflicts_with = "pr")]
        branch: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long, default_value = "main")]
        base: String,
        /// Use the local branch head when an adopted PR has diverged.
        #[arg(long, value_enum)]
        head: Option<HeadPolicy>,
    },
    /// Allocate the next review template for a branch or pull-request target.
    Next {
        repo: Option<String>,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long, conflicts_with = "pr")]
        branch: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long)]
        label: Option<String>,
        /// Use the local branch head when an adopted PR has diverged.
        #[arg(long, value_enum)]
        head: Option<HeadPolicy>,
    },
    /// Repair counters and discover/adopt matching open pull requests.
    Sync {
        repo: Option<String>,
        #[arg(long)]
        remote: Option<String>,
    },
    /// Print or create the human-authored context file for a target.
    Context {
        repo: Option<String>,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long, conflicts_with = "pr")]
        branch: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
    },
    /// Summarize review findings; use `status todo` to list unresolved findings.
    Status {
        filter: Option<String>,
        repo: Option<String>,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long, conflicts_with = "pr")]
        branch: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
    },
    /// Display the persistent workspace root and state-file path.
    Paths,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum, PartialEq, Eq)]
enum HeadPolicy {
    Local,
}

struct TargetRequest<'a> {
    branch: Option<String>,
    pr: Option<u64>,
    base: &'a str,
    head_policy: Option<HeadPolicy>,
    prefetched_pr: Option<&'a PrMetadata>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
/// The persisted top-level registry under `~/gh`.
///
/// It maps stable remote-derived slugs to local checkouts and their review
/// targets. It is an index for quick lookup, not the authoritative finding
/// store.
struct State {
    #[serde(default)]
    repos: BTreeMap<String, Repo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// One registered remote identity and the local checkout currently used for it.
///
/// Branch and pull-request maps preserve the connection from a review target
/// to its ledger directory when a checkout path changes.
struct Repo {
    path: PathBuf,
    remote: String,
    #[serde(default)]
    branches: BTreeMap<String, Target>,
    #[serde(default)]
    pull_requests: BTreeMap<u64, PrAlias>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum PrAlias {
    Branch(String),
    Legacy(Box<Target>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// The persisted allocation and revision boundary for one review target.
///
/// The directory links this target to durable Markdown artifacts; SHA fields
/// make the reviewed range explicit; the counter avoids reusing review names.
struct Target {
    directory: String,
    base_ref: String,
    base_sha: String,
    last_reviewed_head_sha: String,
    next_review_number: u64,
    #[serde(default)]
    pending: Option<ReviewSnapshot>,
    #[serde(default)]
    pull_request_head_sha: Option<String>,
    #[serde(default)]
    pull_request_base_sha: Option<String>,
    #[serde(default)]
    pull_request_base_ref: Option<String>,
    #[serde(default)]
    local_branch_head_sha: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ReviewSnapshot {
    head_sha: String,
    base_sha: String,
    authority: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PrMetadata {
    number: u64,
    state: String,
    head_ref_name: String,
    head_ref_oid: String,
    head_repository_owner: RepositoryOwner,
    head_repository: RepositoryName,
    base_ref_name: String,
    base_ref_oid: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct RepositoryOwner {
    login: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct RepositoryName {
    name: String,
}

#[derive(Serialize)]
/// JSON response confirming that a local checkout was registered.
struct RegisterOutput<'a> {
    slug: &'a str,
    path: &'a Path,
    remote: &'a str,
    state_path: PathBuf,
}

#[derive(Serialize)]
/// JSON response locating the shared review workspace and its state index.
struct PathsOutput {
    home: PathBuf,
    state_path: PathBuf,
}

#[derive(Serialize)]
/// Common JSON context for target-oriented commands.
///
/// It exposes paths and the exact revision range so agents need not infer
/// ledger locations or accidentally review a different checkout HEAD.
struct TargetOutput {
    repo: String,
    discovery: Option<DiscoveryMetadata>,
    repo_path: PathBuf,
    target: String,
    target_path: PathBuf,
    context_path: PathBuf,
    base_ref: String,
    base_sha: String,
    head_sha: String,
    head_authority: String,
    local_branch_sha: Option<String>,
    origin_branch_sha: Option<String>,
    pr_head_sha: Option<String>,
    divergence: bool,
    reviews: Vec<PathBuf>,
}

#[derive(Serialize)]
/// JSON response from allocation, extending target context with the new file.
struct NextOutput {
    #[serde(flatten)]
    target: TargetOutput,
    review_path: PathBuf,
}

#[derive(Debug, Serialize)]
struct SyncOutput {
    repo: String,
    local_repaired: Vec<RepairOutput>,
    recovered: Vec<RecoveryOutput>,
    adopted: Vec<u64>,
    merged: BTreeMap<u64, BTreeMap<u64, u64>>,
    unchanged: Vec<u64>,
    skipped_zero: Vec<String>,
    skipped_ambiguous: BTreeMap<String, Vec<u64>>,
}

#[derive(Debug, Serialize)]
struct RepairOutput {
    branch: String,
    from: u64,
    to: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct RecoveryOutput {
    pull_request: u64,
    action: String,
}

#[derive(Debug, PartialEq, Eq)]
enum AdoptionKind {
    Adopted,
    Merged,
    Unchanged,
}

#[derive(Debug)]
struct AdoptionResult {
    kind: AdoptionKind,
    review_mapping: BTreeMap<u64, u64>,
    backup_created: bool,
}

#[derive(Serialize)]
/// One unresolved Markdown finding located while scanning review artifacts.
struct FindingOutput {
    review_path: PathBuf,
    line: usize,
    text: String,
}

#[derive(Serialize)]
/// Aggregate JSON summary across the selected review targets.
struct StatusOutput {
    targets: Vec<TargetStatus>,
    total: usize,
    resolved: usize,
    todo: usize,
}

#[derive(Serialize)]
/// Per-target counts and unresolved finding locations within a status response.
struct TargetStatus {
    repo: String,
    discovery: Option<DiscoveryMetadata>,
    target: String,
    target_path: PathBuf,
    total: usize,
    resolved: usize,
    todo: usize,
    todo_findings: Vec<FindingOutput>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
struct DiscoveryMetadata {
    discovered_repo: String,
    worktree_root: PathBuf,
    git_common_dir: PathBuf,
    selected_branch: Option<String>,
    selected_remote: String,
    selected_remote_source: String,
    discovery_source: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gh-rev: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.version {
        return print_version();
    }
    let home = review_home(cli.home)?;
    let path = cli.path.as_deref();
    let command = cli.command.with_context(|| "missing command")?;
    match command {
        CommandName::Register { path } => register(&home, &path),
        CommandName::Open {
            repo,
            branch,
            pr,
            base,
            head,
            remote,
        } => open_with_head(
            &home,
            repo.as_deref(),
            path,
            remote.as_deref(),
            branch,
            pr,
            &base,
            head,
        ),
        CommandName::Next {
            repo,
            branch,
            pr,
            label,
            head,
            remote,
        } => next_with_head(
            &home,
            repo.as_deref(),
            path,
            remote.as_deref(),
            branch,
            pr,
            label,
            head,
        ),
        CommandName::Sync { repo, remote } => {
            sync(&home, repo.as_deref(), path, remote.as_deref())
        }
        CommandName::Context {
            repo,
            branch,
            pr,
            remote,
        } => {
            context(&home, repo.as_deref(), path, remote.as_deref(), branch, pr)
        }
        CommandName::Status {
            filter,
            repo,
            remote,
            branch,
            pr,
        } => status(&home, filter, repo, remote, branch, pr, path),
        CommandName::Paths => print_json(&PathsOutput {
            state_path: home.join(STATE_FILE),
            home,
        }),
    }
}

fn print_version() -> Result<()> {
    print_json(&version_output())
}

fn version_output() -> VersionOutput {
    VersionOutput {
        version: APP_VERSION,
        commit: APP_GIT_COMMIT,
        dirty: match APP_GIT_DIRTY {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
    }
}

fn review_home(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let home = env::var_os("HOME").context("HOME is not set; pass --home")?;
    Ok(PathBuf::from(home).join("gh"))
}

fn register(home: &Path, requested_path: &Path) -> Result<()> {
    let path = fs::canonicalize(requested_path).with_context(|| {
        format!("failed to resolve worktree {}", requested_path.display())
    })?;
    let remote_url = git(&path, ["remote", "get-url", "origin"])?;
    let remote = normalize_remote(&remote_url)?;
    let slug = slug_from_remote(&remote)?;

    let _lock = lock(home)?;
    let mut state = read_state(home)?;
    if let Some(repo) = state.repos.get_mut(&slug) {
        if !repo.remote.eq_ignore_ascii_case(&remote) {
            bail!(
                "repository slug {slug} is already registered for {}, not {remote}",
                repo.remote
            );
        }
        repo.path = path.clone();
    } else {
        state.repos.insert(
            slug.clone(),
            Repo {
                path: path.clone(),
                remote: remote.clone(),
                branches: BTreeMap::new(),
                pull_requests: BTreeMap::new(),
            },
        );
    }
    write_state(home, &state)?;
    print_json(&RegisterOutput {
        slug: &slug,
        path: &path,
        remote: &remote,
        state_path: home.join(STATE_FILE),
    })
}

fn discovery_metadata(
    discovery: &Option<RepoDiscovery>,
) -> Option<DiscoveryMetadata> {
    discovery.as_ref().map(|value| DiscoveryMetadata {
        discovered_repo: value.slug.clone(),
        worktree_root: value.worktree_root.clone(),
        git_common_dir: value.git_common_dir.clone(),
        selected_branch: value.branch.clone(),
        selected_remote: value.remote_name.clone(),
        selected_remote_source: value.remote_source.clone(),
        discovery_source: value.source.clone(),
    })
}

/// Discover the repository selected by the caller's current directory (or
/// `-C`) and choose exactly one remote. The checkout location is runtime
/// evidence; the normalized remote-derived slug remains the ledger identity.
fn discover_repository(
    path_override: Option<&Path>,
    remote_override: Option<&str>,
) -> Result<RepoDiscovery> {
    let requested = match path_override {
        Some(path) => fs::canonicalize(path).with_context(|| {
            format!("failed to resolve worktree {}", path.display())
        })?,
        None => env::current_dir()
            .context("failed to determine current directory")?,
    };
    let listed_worktrees = parse_worktree_list(&requested)?;
    let detected_root =
        PathBuf::from(git(&requested, ["rev-parse", "--show-toplevel"])?);
    let worktree_root = listed_worktrees
        .into_iter()
        .find(|path| requested.starts_with(path))
        .unwrap_or(detected_root);
    let common_dir =
        PathBuf::from(git(&worktree_root, ["rev-parse", "--git-common-dir"])?);
    let git_common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        fs::canonicalize(worktree_root.join(common_dir))
            .context("failed to resolve Git common directory")?
    };
    let branch = match git(&worktree_root, ["branch", "--show-current"])? {
        value if value.is_empty() => None,
        value => Some(value),
    };
    let remotes: Vec<String> = git(&worktree_root, ["remote"])?
        .lines()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    let available_remotes = if remotes.is_empty() {
        "<none>".to_owned()
    } else {
        remotes.join(", ")
    };
    let configured_upstream = branch.as_ref().and_then(|name| {
        git_dynamic(
            &worktree_root,
            ["config", "--get", &format!("branch.{name}.remote")],
        )
        .ok()
        .filter(|value| !value.is_empty())
    });
    let push_default =
        git(&worktree_root, ["config", "--get", "remote.pushDefault"])
            .ok()
            .filter(|value| !value.is_empty());
    let remote_name = remote_override
        .map(str::to_owned)
        .or(configured_upstream)
        .or(push_default)
        .or_else(|| {
            remotes
                .iter()
                .any(|name| name == "origin")
                .then(|| "origin".to_owned())
        })
        .or_else(|| (remotes.len() == 1).then(|| remotes[0].clone()))
        .with_context(|| {
            format!(
                "could not select a Git remote; available remotes: {available_remotes}; pass --remote <name>"
            )
        })?;
    if !remotes.iter().any(|name| name == &remote_name) {
        bail!(
            "selected remote {remote_name} is not configured; available remotes: {available_remotes}; pass --remote <name>"
        );
    }
    let remote_url =
        git_dynamic(&worktree_root, ["remote", "get-url", &remote_name])?;
    let remote = normalize_remote(&remote_url)?;
    let slug = slug_from_remote(&remote)?;
    Ok(RepoDiscovery {
        slug,
        source: path_override
            .map_or_else(|| "cwd".to_owned(), |_| "-C".to_owned()),
        worktree_root,
        git_common_dir,
        branch,
        remote_name,
        remote,
        remote_source: if remote_override.is_some() {
            "explicit".to_owned()
        } else {
            "configured".to_owned()
        },
    })
}

fn parse_worktree_list(requested: &Path) -> Result<Vec<PathBuf>> {
    let output = git(requested, ["worktree", "list", "--porcelain"]).ok();
    let Some(raw) = output else {
        return Ok(Vec::new());
    };
    let mut worktrees = Vec::new();
    for line in raw.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            let candidate = PathBuf::from(path);
            if let Ok(canonical) = fs::canonicalize(&candidate) {
                worktrees.push(canonical);
            } else {
                worktrees.push(candidate);
            }
        }
    }
    Ok(worktrees)
}

fn ensure_repo_from_discovery<'a>(
    state: &'a mut State,
    slug: &str,
    discovery: Option<&RepoDiscovery>,
) -> Result<(&'a mut Repo, bool)> {
    match discovery {
        Some(discovery) => match state.repos.entry(slug.to_owned()) {
            std::collections::btree_map::Entry::Occupied(entry) => {
                let repo = entry.into_mut();
                if !repo.remote.eq_ignore_ascii_case(&discovery.remote) {
                    bail!(
                        "repository slug {slug} is already registered for {}, not {}",
                        repo.remote,
                        discovery.remote
                    );
                }
                let changed = repo.path != discovery.worktree_root;
                repo.path = discovery.worktree_root.clone();
                Ok((repo, changed))
            }
            std::collections::btree_map::Entry::Vacant(entry) => Ok((
                entry.insert(Repo {
                    path: discovery.worktree_root.clone(),
                    remote: discovery.remote.clone(),
                    branches: BTreeMap::new(),
                    pull_requests: BTreeMap::new(),
                }),
                true,
            )),
        },
        None => state
            .repos
            .get_mut(slug)
            .map(|repo| (repo, false))
            .with_context(|| format!("unknown repository {slug}; run from its worktree or register it")),
    }
}

#[allow(clippy::too_many_arguments)]
fn open_with_head(
    home: &Path,
    repo: Option<&str>,
    path_override: Option<&Path>,
    remote_override: Option<&str>,
    branch: Option<String>,
    pr: Option<u64>,
    base: &str,
    head_policy: Option<HeadPolicy>,
) -> Result<()> {
    open_with_head_with_discovered_pr_fetch(
        home,
        repo,
        path_override,
        remote_override,
        branch,
        pr,
        base,
        head_policy,
        fetch_discovered_pr,
    )
}

#[allow(clippy::too_many_arguments)]
/// Opens a discovered or explicitly selected target after resolving a matching
/// PR through the supplied discovery fetch function.
///
/// The injected function keeps command behavior testable without making a
/// network request. A single discovered PR becomes the canonical target;
/// otherwise the selected branch remains the target.
fn open_with_head_with_discovered_pr_fetch<F>(
    home: &Path,
    repo: Option<&str>,
    path_override: Option<&Path>,
    remote_override: Option<&str>,
    branch: Option<String>,
    pr: Option<u64>,
    base: &str,
    head_policy: Option<HeadPolicy>,
    fetch_discovered: F,
) -> Result<()>
where
    F: FnOnce(
        &RepoDiscovery,
        Option<&str>,
    ) -> Result<Option<(String, PrMetadata)>>,
{
    let discovery = if repo.is_none() {
        Some(discover_repository(path_override, remote_override)?)
    } else {
        None
    };
    let branch = branch
        .or_else(|| discovery.as_ref().and_then(|value| value.branch.clone()));
    let slug = match repo {
        Some(value) => value.to_owned(),
        None => discovery
            .as_ref()
            .context("failed to discover repository")?
            .slug
            .to_owned(),
    };
    let prefetched_pr = if let Some(number) = pr {
        if let Some(discovery) = &discovery {
            Some((
                discovery.remote.clone(),
                gh_pr_for_remote(&discovery.remote, number)
                    .with_context(|| format!("failed to fetch PR #{number}"))?,
            ))
        } else {
            fetch_targeted_pr_without_lock(home, &slug, Some(number))?
        }
    } else if let Some(discovery) = &discovery {
        fetch_discovered(discovery, branch.as_deref())?
    } else {
        None
    };
    let pr = prefetched_pr
        .as_ref()
        .map(|(_, metadata)| metadata.number)
        .or(pr);
    validate_head_policy(branch.as_deref(), pr, head_policy)?;
    let _lock = if prefetched_pr.is_some() {
        try_lock(home)?
    } else {
        lock(home)?
    };
    let mut state = read_state(home)?;
    let (repo, _) =
        ensure_repo_from_discovery(&mut state, &slug, discovery.as_ref())?;
    validate_prefetched_remote(repo, prefetched_pr.as_ref())?;
    let repo_path = discovery
        .as_ref()
        .map(|value| value.worktree_root.clone())
        .unwrap_or_else(|| repo.path.clone());
    let (target_name, branch_name, head_sha, cleanup_pr, target) =
        resolve_or_create_target(
            home,
            &slug,
            repo,
            &repo_path,
            TargetRequest {
                branch: if pr.is_some() { None } else { branch },
                pr,
                base,
                head_policy,
                prefetched_pr: prefetched_pr
                    .as_ref()
                    .map(|(_, metadata)| metadata),
            },
        )?;
    report_reconciled_target(home, &slug, &target_name, target)?;
    target.pending = Some(ReviewSnapshot {
        head_sha: head_sha.clone(),
        base_sha: target.base_sha.clone(),
        authority: target_authority(&target_name),
    });
    let output = target_output(
        home,
        &slug,
        discovery_metadata(&discovery),
        &repo_path,
        &target_name,
        &branch_name,
        target,
        &head_sha,
    )?;
    write_state(home, &state)?;
    cleanup_adoption_backups(home, &slug, cleanup_pr)?;
    print_json(&output)
}

#[cfg(test)]
fn open(
    home: &Path,
    slug: &str,
    branch: Option<String>,
    pr: Option<u64>,
    base: &str,
) -> Result<()> {
    open_with_head(home, Some(slug), None, None, branch, pr, base, None)
}

#[allow(clippy::too_many_arguments)]
fn next_with_head(
    home: &Path,
    repo: Option<&str>,
    path_override: Option<&Path>,
    remote_override: Option<&str>,
    branch: Option<String>,
    pr: Option<u64>,
    label: Option<String>,
    head_policy: Option<HeadPolicy>,
) -> Result<()> {
    next_with_head_with_discovered_pr_fetch(
        home,
        repo,
        path_override,
        remote_override,
        branch,
        pr,
        label,
        head_policy,
        fetch_discovered_pr,
    )
}

#[allow(clippy::too_many_arguments)]
/// Allocates the next review for a discovered or explicit target after the
/// supplied function resolves any matching open PR.
///
/// This function preserves `open`'s target-selection rule so the pending
/// snapshot and allocated revision use the same branch or canonical PR ledger.
fn next_with_head_with_discovered_pr_fetch<F>(
    home: &Path,
    repo: Option<&str>,
    path_override: Option<&Path>,
    remote_override: Option<&str>,
    branch: Option<String>,
    pr: Option<u64>,
    label: Option<String>,
    head_policy: Option<HeadPolicy>,
    fetch_discovered: F,
) -> Result<()>
where
    F: FnOnce(
        &RepoDiscovery,
        Option<&str>,
    ) -> Result<Option<(String, PrMetadata)>>,
{
    let discovery = if repo.is_none() {
        Some(discover_repository(path_override, remote_override)?)
    } else {
        None
    };
    let branch = branch
        .or_else(|| discovery.as_ref().and_then(|value| value.branch.clone()));
    let slug = match repo {
        Some(value) => value.to_owned(),
        None => discovery
            .as_ref()
            .context("failed to discover repository")?
            .slug
            .to_owned(),
    };
    let prefetched_pr = if let Some(number) = pr {
        if let Some(discovery) = &discovery {
            Some((
                discovery.remote.clone(),
                gh_pr_for_remote(&discovery.remote, number)
                    .with_context(|| format!("failed to fetch PR #{number}"))?,
            ))
        } else {
            fetch_targeted_pr_without_lock(home, &slug, Some(number))?
        }
    } else if let Some(discovery) = &discovery {
        fetch_discovered(discovery, branch.as_deref())?
    } else {
        None
    };
    let pr = prefetched_pr
        .as_ref()
        .map(|(_, metadata)| metadata.number)
        .or(pr);
    validate_head_policy(branch.as_deref(), pr, head_policy)?;
    let _lock = if prefetched_pr.is_some() {
        try_lock(home)?
    } else {
        lock(home)?
    };
    let mut state = read_state(home)?;
    let (repo, _) =
        ensure_repo_from_discovery(&mut state, &slug, discovery.as_ref())?;
    validate_prefetched_remote(repo, prefetched_pr.as_ref())?;
    let repo_path = discovery
        .as_ref()
        .map(|value| value.worktree_root.clone())
        .unwrap_or_else(|| repo.path.clone());
    let base = branch
        .as_ref()
        .and_then(|name| repo.branches.get(name))
        .map_or_else(|| "main".to_owned(), |target| target.base_ref.clone());
    let (target_name, branch_name, head_sha, cleanup_pr, target) =
        resolve_or_create_target(
            home,
            &slug,
            repo,
            &repo_path,
            TargetRequest {
                branch: if pr.is_some() { None } else { branch },
                pr,
                base: &base,
                head_policy,
                prefetched_pr: prefetched_pr
                    .as_ref()
                    .map(|(_, metadata)| metadata),
            },
        )?;
    report_reconciled_target(home, &slug, &target_name, target)?;
    let authority = target_authority(&target_name);
    validate_pending_snapshot(target, &authority, &head_sha)?;
    let number = target.next_review_number;
    target.next_review_number = target
        .next_review_number
        .checked_add(1)
        .context("review number overflow; operator repair is required")?;
    let target_path = home.join(&slug).join(&target.directory);
    fs::create_dir_all(&target_path)?;
    let review_path = target_path.join(format!("rev-{number}.md"));
    let mut review_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&review_path)
        .with_context(|| {
            format!("failed to exclusively create {}", review_path.display())
        })?;
    review_file
        .write_all(
            review_template(
                &target_name,
                target,
                &head_sha,
                number,
                label.as_deref(),
            )
            .as_bytes(),
        )
        .with_context(|| format!("failed to write {}", review_path.display()))?;
    review_file.sync_all()?;
    target.last_reviewed_head_sha = head_sha.clone();
    target.pending = None;
    let output = NextOutput {
        target: target_output(
            home,
            &slug,
            discovery_metadata(&discovery),
            &repo_path,
            &target_name,
            &branch_name,
            target,
            &head_sha,
        )?,
        review_path,
    };
    write_state(home, &state)?;
    cleanup_adoption_backups(home, &slug, cleanup_pr)?;
    print_json(&output)
}

#[cfg(test)]
fn next(
    home: &Path,
    slug: &str,
    branch: Option<String>,
    pr: Option<u64>,
    label: Option<String>,
) -> Result<()> {
    next_with_head(home, Some(slug), None, None, branch, pr, label, None)
}

fn context(
    home: &Path,
    repo: Option<&str>,
    path_override: Option<&Path>,
    remote_override: Option<&str>,
    branch: Option<String>,
    pr: Option<u64>,
) -> Result<()> {
    context_with_discovered_pr_fetch(
        home,
        repo,
        path_override,
        remote_override,
        branch,
        pr,
        fetch_discovered_pr,
    )
}

/// Reads or creates context for a discovered target after resolving a matching
/// PR through the supplied discovery fetch function.
///
/// Context follows the same canonical-history selection as `open` and `next`
/// without allocating a revision.
fn context_with_discovered_pr_fetch<F>(
    home: &Path,
    repo: Option<&str>,
    path_override: Option<&Path>,
    remote_override: Option<&str>,
    branch: Option<String>,
    pr: Option<u64>,
    fetch_discovered: F,
) -> Result<()>
where
    F: FnOnce(
        &RepoDiscovery,
        Option<&str>,
    ) -> Result<Option<(String, PrMetadata)>>,
{
    let discovery = if repo.is_none() {
        Some(discover_repository(path_override, remote_override)?)
    } else {
        None
    };
    let branch = branch
        .or_else(|| discovery.as_ref().and_then(|value| value.branch.clone()));
    let slug = match repo {
        Some(value) => value.to_owned(),
        None => discovery
            .as_ref()
            .context("failed to discover repository")?
            .slug
            .to_owned(),
    };
    let prefetched_pr = if let Some(number) = pr {
        if let Some(discovery) = &discovery {
            Some((
                discovery.remote.clone(),
                gh_pr_for_remote(&discovery.remote, number)
                    .with_context(|| format!("failed to fetch PR #{number}"))?,
            ))
        } else {
            fetch_targeted_pr_without_lock(home, &slug, Some(number))?
        }
    } else if let Some(discovery) = &discovery {
        fetch_discovered(discovery, branch.as_deref())?
    } else {
        None
    };
    let pr = prefetched_pr
        .as_ref()
        .map(|(_, metadata)| metadata.number)
        .or(pr);
    let _lock = if prefetched_pr.is_some() {
        try_lock(home)?
    } else {
        lock(home)?
    };
    let mut state = read_state(home)?;
    let (repo, _) =
        ensure_repo_from_discovery(&mut state, &slug, discovery.as_ref())?;
    validate_prefetched_remote(repo, prefetched_pr.as_ref())?;
    let repo_path = discovery
        .as_ref()
        .map(|value| value.worktree_root.clone())
        .unwrap_or_else(|| repo.path.clone());
    let base = branch
        .as_ref()
        .and_then(|name| repo.branches.get(name))
        .map_or_else(|| "main".to_owned(), |target| target.base_ref.clone());
    let head_policy = branch.as_ref().map(|_| HeadPolicy::Local);
    let (target_name, branch_name, head_sha, cleanup_pr, target) =
        resolve_or_create_target(
            home,
            &slug,
            repo,
            &repo_path,
            TargetRequest {
                branch: if pr.is_some() { None } else { branch },
                pr,
                base: &base,
                head_policy,
                prefetched_pr: prefetched_pr
                    .as_ref()
                    .map(|(_, metadata)| metadata),
            },
        )?;
    report_reconciled_target(home, &slug, &target_name, target)?;
    let output = target_output(
        home,
        &slug,
        discovery_metadata(&discovery),
        &repo_path,
        &target_name,
        &branch_name,
        target,
        &head_sha,
    )?;
    write_state(home, &state)?;
    cleanup_adoption_backups(home, &slug, cleanup_pr)?;
    print_json(
        &serde_json::json!({"context_path": output.context_path, "target": output}),
    )
}

/// Reconcile persisted allocation state with durable review artifacts.
///
/// The review directory is the source of truth for already-allocated review
/// numbers: state can be lost or written by an older version of the tool, but
/// an existing artifact must never be overwritten.
fn report_reconciled_target(
    home: &Path,
    slug: &str,
    target_name: &str,
    target: &mut Target,
) -> Result<()> {
    let target_path = home.join(slug).join(&target.directory);
    fs::create_dir_all(&target_path)?;
    let artifact_next = highest_review_number(&target_path)?
        .map(|number| {
            number.checked_add(1).with_context(|| {
                format!(
                    "cannot allocate after {}: rev-{number}.md uses the largest supported review number; operator repair is required",
                    target_path.display()
                )
            })
        })
        .transpose()?;
    let repaired_next =
        target.next_review_number.max(artifact_next.unwrap_or(1));
    if repaired_next != target.next_review_number {
        let previous = target.next_review_number;
        target.next_review_number = repaired_next;
        eprintln!(
            "gh-rev: repaired review allocation state for {target_name}: next review number {previous} -> {repaired_next} from existing artifacts"
        );
    }
    Ok(())
}

fn status(
    home: &Path,
    first: Option<String>,
    second: Option<String>,
    remote_override: Option<String>,
    branch: Option<String>,
    pr: Option<u64>,
    path_override: Option<&Path>,
) -> Result<()> {
    let (todo_only, repo_filter) = parse_status_positionals(first, second)?;
    if branch.is_some() && pr.is_some() {
        bail!("use either --branch or --pr");
    }
    let discovery = if repo_filter.is_none() {
        Some(discover_repository(
            path_override,
            remote_override.as_deref(),
        )?)
    } else {
        None
    };
    let repo_filter = repo_filter
        .or_else(|| discovery.as_ref().map(|value| value.slug.clone()));
    let branch = branch
        .or_else(|| discovery.as_ref().and_then(|value| value.branch.clone()));
    let _lock = lock(home)?;
    let mut state = read_state(home)?;
    let discovery_output = discovery_metadata(&discovery);
    if let Some(discovery) = discovery.as_ref() {
        let (_, changed) = ensure_repo_from_discovery(
            &mut state,
            repo_filter.as_deref().expect("discovery supplies a slug"),
            Some(discovery),
        )?;
        if changed {
            write_state(home, &state)?;
        }
    }
    let mut targets = collect_status_targets(
        home,
        &state,
        repo_filter.as_deref(),
        branch,
        pr,
        discovery_output,
    )?;
    if todo_only {
        targets.retain(|target| target.todo > 0);
    }
    let total = targets.iter().map(|target| target.total).sum();
    let resolved = targets.iter().map(|target| target.resolved).sum();
    let todo = targets.iter().map(|target| target.todo).sum();
    print_json(&StatusOutput {
        targets,
        total,
        resolved,
        todo,
    })
}

fn collect_status_targets(
    home: &Path,
    state: &State,
    repo_filter: Option<&str>,
    branch: Option<String>,
    pr: Option<u64>,
    discovery: Option<DiscoveryMetadata>,
) -> Result<Vec<TargetStatus>> {
    let mut targets = Vec::new();
    for (slug, repo) in &state.repos {
        if repo_filter.is_some_and(|value| value != slug) {
            continue;
        }
        let selected_branch = if let Some(number) = pr {
            let Some(alias) = repo.pull_requests.get(&number) else {
                if repo_filter.is_some() {
                    bail!("PR {number} is unknown in repository {slug}");
                }
                continue;
            };
            Some(alias_branch(alias, number)?.to_owned())
        } else {
            branch.clone()
        };
        for (branch_name, target) in &repo.branches {
            if selected_branch
                .as_deref()
                .is_some_and(|value| value != branch_name)
            {
                continue;
            }
            let target_name = pr.map_or_else(
                || format!("branch:{branch_name}"),
                |number| format!("pr:{number}"),
            );
            targets.push(target_status(
                home,
                slug,
                target_name,
                target,
                discovery.clone(),
            )?);
        }
    }
    Ok(targets)
}

fn parse_status_positionals(
    first: Option<String>,
    second: Option<String>,
) -> Result<(bool, Option<String>)> {
    match (first, second) {
        (None, None) => Ok((false, None)),
        (Some(value), None) if value == "todo" => Ok((true, None)),
        (Some(repo), None) => Ok((false, Some(repo))),
        (Some(filter), Some(repo)) if filter == "todo" => Ok((true, Some(repo))),
        (Some(filter), Some(_)) => {
            bail!("unknown status filter `{filter}`; expected `todo`")
        }
        (None, Some(_)) => bail!("repository requires a preceding filter"),
    }
}

fn fetch_targeted_pr_without_lock(
    home: &Path,
    slug: &str,
    number: Option<u64>,
) -> Result<Option<(String, PrMetadata)>> {
    fetch_targeted_pr_without_lock_with(home, slug, number, gh_pr)
}

fn fetch_targeted_pr_without_lock_with<F>(
    home: &Path,
    slug: &str,
    number: Option<u64>,
    fetch: F,
) -> Result<Option<(String, PrMetadata)>>
where
    F: FnOnce(&Repo, u64) -> Result<PrMetadata>,
{
    let Some(number) = number else {
        return Ok(None);
    };
    let repo_snapshot = {
        let snapshot_lock = try_lock(home)?;
        let state = read_state(home)?;
        let snapshot = state
            .repos
            .get(slug)
            .with_context(|| {
                format!("unknown repository {slug}; run register first")
            })?
            .clone();
        drop(snapshot_lock);
        snapshot
    };
    let metadata = fetch(&repo_snapshot, number)?;
    Ok(Some((repo_snapshot.remote, metadata)))
}

fn validate_prefetched_remote(
    repo: &Repo,
    prefetched: Option<&(String, PrMetadata)>,
) -> Result<()> {
    if let Some((remote, _)) = prefetched
        && &repo.remote != remote
    {
        bail!(
            "repository changed remote identity while fetching GitHub PR metadata; rerun the command"
        );
    }
    Ok(())
}

fn resolve_or_create_target<'a>(
    home: &Path,
    slug: &str,
    repo: &'a mut Repo,
    repo_path: &Path,
    request: TargetRequest<'_>,
) -> Result<(String, String, String, Option<u64>, &'a mut Target)> {
    let TargetRequest {
        branch,
        pr,
        base,
        head_policy,
        prefetched_pr,
    } = request;
    match (branch, pr) {
        (Some(branch), None) => {
            let head_sha = exact_branch_sha(repo_path, &branch)?;
            let base_sha = exact_branch_sha(repo_path, base)?;
            let directory = branch_directory(home, slug, repo, &branch)?;
            let has_pr = repo
                .pull_requests
                .values()
                .any(|alias| matches!(alias, PrAlias::Branch(value) if value == &branch));
            let target =
                repo.branches.entry(branch.clone()).or_insert_with(|| {
                    new_target(directory, base, &base_sha, &head_sha)
                });
            if has_pr
                && target
                    .pull_request_head_sha
                    .as_deref()
                    .is_some_and(|github| github != head_sha)
                && head_policy != Some(HeadPolicy::Local)
            {
                bail!(
                    "local branch {branch} diverges from its adopted PR head; \
                     pass --head local to review the local head"
                );
            }
            target.base_ref = base.to_owned();
            target.base_sha = base_sha;
            target.local_branch_head_sha = Some(head_sha.clone());
            Ok((format!("branch:{branch}"), branch, head_sha, None, target))
        }
        (None, Some(number)) => {
            let metadata = prefetched_pr.with_context(|| {
                format!("PR {number} metadata was not fetched before locking")
            })?;
            if metadata.number != number {
                bail!(
                    "fetched PR {} metadata while resolving PR {number}",
                    metadata.number
                );
            }
            let (branch, head_sha, adoption) =
                apply_targeted_pr_metadata(home, slug, repo, metadata)?;
            let target = repo.branches.get_mut(&branch).unwrap();
            let cleanup = adoption.backup_created.then_some(metadata.number);
            Ok((format!("pr:{number}"), branch, head_sha, cleanup, target))
        }
        _ => bail!("supply exactly one of --branch or --pr"),
    }
}

fn validate_head_policy(
    branch: Option<&str>,
    pr: Option<u64>,
    policy: Option<HeadPolicy>,
) -> Result<()> {
    if policy.is_some() && (branch.is_none() || pr.is_some()) {
        bail!("--head local is only valid with --branch");
    }
    Ok(())
}

fn target_authority(target_name: &str) -> String {
    target_name
        .split_once(':')
        .map_or("branch", |(kind, _)| kind)
        .to_owned()
}

fn validate_pending_snapshot(
    target: &Target,
    authority: &str,
    head_sha: &str,
) -> Result<()> {
    if let Some(pending) = &target.pending
        && (pending.head_sha != head_sha
            || pending.base_sha != target.base_sha
            || pending.authority != authority)
    {
        bail!(
            "selected review snapshot changed after open: authority {} range \
             {}..{} is now authority {} range {}..{}; reopen to re-resolve",
            pending.authority,
            pending.base_sha,
            pending.head_sha,
            authority,
            target.base_sha,
            head_sha
        );
    }
    Ok(())
}

fn apply_targeted_pr_metadata(
    home: &Path,
    slug: &str,
    repo: &mut Repo,
    metadata: &PrMetadata,
) -> Result<(String, String, AdoptionResult)> {
    validate_pr_repository(repo, metadata)?;
    let branch = metadata.head_ref_name.clone();
    let local_head = exact_branch_sha(&repo.path, &branch)?;
    let mut legacy = take_legacy_pr_target(repo, metadata)?;
    if !repo.branches.contains_key(&branch) {
        let target = if let Some(target) = legacy.take() {
            target
        } else {
            let directory = branch_directory(home, slug, repo, &branch)?;
            new_target(
                directory,
                &metadata.base_ref_name,
                &metadata.base_ref_oid,
                &metadata.head_ref_oid,
            )
        };
        repo.branches.insert(branch.clone(), target);
    }
    apply_pr_metadata(home, slug, repo, metadata, Some(local_head), legacy)
}

fn apply_pr_metadata(
    home: &Path,
    slug: &str,
    repo: &mut Repo,
    metadata: &PrMetadata,
    known_local_head: Option<String>,
    legacy: Option<Target>,
) -> Result<(String, String, AdoptionResult)> {
    let legacy = legacy.or(take_legacy_pr_target(repo, metadata)?);
    let branch = matching_branch(repo, metadata)?;
    let local_head = known_local_head
        .map(Ok)
        .unwrap_or_else(|| exact_branch_sha(&repo.path, &branch))?;
    let adoption =
        adopt_pull_request(home, slug, repo, metadata.number, &branch)?;
    let target = repo.branches.get_mut(&branch).unwrap();
    if let Some(legacy) = legacy {
        target.next_review_number =
            target.next_review_number.max(legacy.next_review_number);
        if adoption.kind == AdoptionKind::Merged {
            target.last_reviewed_head_sha = legacy.last_reviewed_head_sha;
        }
    }
    target.local_branch_head_sha = Some(local_head);
    target.pull_request_head_sha = Some(metadata.head_ref_oid.clone());
    target.pull_request_base_sha = Some(metadata.base_ref_oid.clone());
    target.pull_request_base_ref = Some(metadata.base_ref_name.clone());
    target.base_ref = metadata.base_ref_name.clone();
    target.base_sha = metadata.base_ref_oid.clone();
    Ok((branch, metadata.head_ref_oid.clone(), adoption))
}

fn take_legacy_pr_target(
    repo: &mut Repo,
    metadata: &PrMetadata,
) -> Result<Option<Target>> {
    let Some(alias) = repo.pull_requests.remove(&metadata.number) else {
        return Ok(None);
    };
    match alias {
        PrAlias::Branch(branch) => {
            repo.pull_requests
                .insert(metadata.number, PrAlias::Branch(branch));
            Ok(None)
        }
        PrAlias::Legacy(target) => {
            let canonical = format!("pr-{}", metadata.number);
            if target.directory != canonical {
                repo.pull_requests
                    .insert(metadata.number, PrAlias::Legacy(target));
                bail!(
                    "legacy PR {} uses directory other than {}; \
                     repair that mapping before synchronization",
                    metadata.number,
                    canonical
                );
            }
            Ok(Some(*target))
        }
    }
}

fn new_target(
    directory: String,
    base_ref: &str,
    base_sha: &str,
    head_sha: &str,
) -> Target {
    Target {
        directory,
        base_ref: base_ref.to_owned(),
        base_sha: base_sha.to_owned(),
        last_reviewed_head_sha: head_sha.to_owned(),
        next_review_number: 1,
        pending: None,
        pull_request_head_sha: None,
        pull_request_base_sha: None,
        pull_request_base_ref: None,
        local_branch_head_sha: Some(head_sha.to_owned()),
    }
}

fn exact_branch_sha(worktree: &Path, branch: &str) -> Result<String> {
    let revision = format!("refs/heads/{branch}^{{commit}}");
    git(worktree, ["rev-parse", "--verify", &revision]).with_context(|| {
        format!("local branch {branch} does not resolve to a commit")
    })
}

fn alias_branch(alias: &PrAlias, number: u64) -> Result<&str> {
    match alias {
        PrAlias::Branch(branch) => Ok(branch),
        PrAlias::Legacy(target) => bail!(
            "legacy PR {number} contains target data for {}; refusing to \
             silently discard it; migrate or recover this record explicitly",
            target.directory
        ),
    }
}

fn repository_identity(repo: &Repo) -> Result<(&str, &str)> {
    let mut parts = repo.remote.split('/');
    let _host = parts.next();
    let owner = parts.next().context("registered remote has no owner")?;
    let name = parts
        .next()
        .context("registered remote has no repository")?;
    Ok((owner, name))
}

fn matching_branch(repo: &Repo, metadata: &PrMetadata) -> Result<String> {
    validate_pr_repository(repo, metadata)?;
    if repo.branches.contains_key(&metadata.head_ref_name) {
        Ok(metadata.head_ref_name.clone())
    } else {
        bail!(
            "PR {} head branch {} is not registered",
            metadata.number,
            metadata.head_ref_name
        )
    }
}

fn validate_pr_repository(repo: &Repo, metadata: &PrMetadata) -> Result<()> {
    if metadata.state != "OPEN" {
        bail!("PR {} is not open", metadata.number);
    }
    let (owner, name) = repository_identity(repo)?;
    if !metadata
        .head_repository_owner
        .login
        .eq_ignore_ascii_case(owner)
        || !metadata.head_repository.name.eq_ignore_ascii_case(name)
    {
        bail!(
            "PR {} head is from fork {}/{}; expected {owner}/{name}",
            metadata.number,
            metadata.head_repository_owner.login,
            metadata.head_repository.name
        );
    }
    Ok(())
}

fn gh_pr(repo: &Repo, number: u64) -> Result<PrMetadata> {
    let (owner, name) = repository_identity(repo)?;
    gh_pr_for_remote(&format!("github.com/{owner}/{name}"), number)
}

fn gh_pr_for_remote(remote: &str, number: u64) -> Result<PrMetadata> {
    let (owner, name) = remote
        .split_once('/')
        .and_then(|(_, path)| path.split_once('/'))
        .context("remote must have host/owner/repository form")?;
    let repository = format!("{owner}/{name}");
    let number = number.to_string();
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &number,
            "--repo",
            &repository,
            "--json",
            "number,state,headRefName,headRefOid,headRepositoryOwner,\
             headRepository,baseRefName,baseRefOid",
        ])
        .output()
        .context("failed to invoke gh")?;
    if !output.status.success() {
        bail!(
            "gh pr view failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("invalid gh PR metadata")
}

fn gh_open_prs(repo: &Repo) -> Result<Vec<PrMetadata>> {
    let (owner, name) = repository_identity(repo)?;
    gh_open_prs_for_remote(&format!("github.com/{owner}/{name}"), None)
}

/// Lists open pull requests in one repository, optionally restricted to an
/// exact head-branch name.
///
/// The `remote` argument is the normalized configured repository identity,
/// such as `github.com/owner/repository`, rather than a Git remote name.
fn gh_open_prs_for_remote(
    remote: &str,
    branch: Option<&str>,
) -> Result<Vec<PrMetadata>> {
    let (owner, name) = remote
        .split_once('/')
        .and_then(|(_, path)| path.split_once('/'))
        .context("remote must have host/owner/repository form")?;
    let repository = format!("{owner}/{name}");
    let mut command = Command::new("gh");
    command.args([
        "pr",
        "list",
        "--repo",
        &repository,
        "--state",
        "open",
        "--limit",
        "1000",
    ]);
    if let Some(branch) = branch {
        command.args(["--head", branch]);
    }
    let output = command
        .args([
            "--json",
            "number,state,headRefName,headRefOid,headRepositoryOwner,\
             headRepository,baseRefName,baseRefOid",
        ])
        .output()
        .context("failed to invoke gh")?;
    if !output.status.success() {
        bail!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("invalid gh PR metadata")
}

/// Fetches the one open PR whose head branch matches the discovered worktree.
///
/// A detached worktree has no branch and therefore no automatic PR target.
fn fetch_discovered_pr(
    discovery: &RepoDiscovery,
    branch: Option<&str>,
) -> Result<Option<(String, PrMetadata)>> {
    fetch_discovered_pr_with(discovery, branch, gh_open_prs_for_remote)
}

/// Selects one exact head-branch match from a discovery-scoped PR query.
///
/// Zero matches preserve branch history. More than one match is ambiguous and
/// fails before the caller obtains a ledger lock or mutates ledger state.
fn fetch_discovered_pr_with<F>(
    discovery: &RepoDiscovery,
    branch: Option<&str>,
    fetch: F,
) -> Result<Option<(String, PrMetadata)>>
where
    F: FnOnce(&str, Option<&str>) -> Result<Vec<PrMetadata>>,
{
    let Some(branch) = branch else {
        return Ok(None);
    };
    let matches: Vec<_> = fetch(&discovery.remote, Some(branch))?
        .into_iter()
        .filter(|metadata| metadata.head_ref_name == branch)
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some((
            discovery.remote.clone(),
            matches.into_iter().next().unwrap(),
        ))),
        _ => bail!(
            "multiple open pull requests match current branch {branch}; pass --pr to select one"
        ),
    }
}

fn sync(
    home: &Path,
    repo: Option<&str>,
    path_override: Option<&Path>,
    remote_override: Option<&str>,
) -> Result<()> {
    let output =
        sync_with(home, repo, path_override, remote_override, gh_open_prs)?;
    print_json(&output)
}

fn sync_with<F>(
    home: &Path,
    repo: Option<&str>,
    path_override: Option<&Path>,
    remote_override: Option<&str>,
    fetch: F,
) -> Result<SyncOutput>
where
    F: FnOnce(&Repo) -> Result<Vec<PrMetadata>>,
{
    let discovery = if repo.is_none() {
        Some(discover_repository(path_override, remote_override)?)
    } else {
        None
    };
    let slug = repo
        .map(str::to_owned)
        .or_else(|| discovery.as_ref().map(|value| value.slug.clone()))
        .context("failed to discover repository")?;
    if let Some(discovery) = discovery.as_ref() {
        let _lock = lock(home)?;
        let mut state = read_state(home)?;
        let (_, changed) =
            ensure_repo_from_discovery(&mut state, &slug, Some(discovery))?;
        if changed {
            write_state(home, &state)?;
        }
    }
    sync_with_fetch(home, &slug, fetch)
}

fn sync_with_fetch<F>(home: &Path, slug: &str, fetch: F) -> Result<SyncOutput>
where
    F: FnOnce(&Repo) -> Result<Vec<PrMetadata>>,
{
    // Network access must not hold the ledger lock. Snapshot only the stable
    // repository identity needed by `gh`, then reacquire the lock and reread
    // state before applying anything.
    let repo_snapshot = {
        let snapshot_lock = try_lock(home)?;
        let state = read_state(home)?;
        let snapshot = state
            .repos
            .get(slug)
            .with_context(|| {
                format!("unknown repository {slug}; run register first")
            })?
            .clone();
        drop(snapshot_lock);
        snapshot
    };
    let metadata_result = fetch(&repo_snapshot);

    let _lock = try_lock(home)?;
    let mut state = read_state(home)?;
    let repo = state.repos.get_mut(slug).with_context(|| {
        format!("unknown repository {slug}; run register first")
    })?;
    if repo.remote != repo_snapshot.remote {
        bail!(
            "repository {slug} changed remote identity while sync fetched GitHub metadata; rerun sync"
        );
    }
    let metadata = match metadata_result {
        Ok(value) => value,
        Err(error) => {
            let recovered = recover_incomplete_adoptions(home, slug, repo, &[])?;
            let local_repaired = reconcile_repo_targets(home, slug, repo)?;
            write_state(home, &state)?;
            if !recovered.is_empty() || !local_repaired.is_empty() {
                eprintln!(
                    "gh-rev: persisted local recovery before GitHub sync failed"
                );
            }
            bail!("github: unavailable: {error:#}");
        }
    };
    let recovered = recover_incomplete_adoptions(home, slug, repo, &metadata)?;
    let local_repaired = reconcile_repo_targets(home, slug, repo)?;

    let mut by_branch: BTreeMap<String, Vec<PrMetadata>> = repo
        .branches
        .keys()
        .map(|branch| (branch.clone(), Vec::new()))
        .collect();
    for item in metadata {
        if let Ok(branch) = matching_branch(repo, &item) {
            by_branch.entry(branch).or_default().push(item);
        }
    }
    let mut adopted = Vec::new();
    let mut merged = BTreeMap::new();
    let mut unchanged = Vec::new();
    let mut skipped_zero = Vec::new();
    let mut skipped_ambiguous = BTreeMap::new();
    let mut cleanup_numbers = Vec::new();
    for (branch, matches) in by_branch {
        if matches.is_empty() {
            skipped_zero.push(branch);
            continue;
        }
        if matches.len() > 1 {
            skipped_ambiguous.insert(
                branch,
                matches.iter().map(|item| item.number).collect(),
            );
            continue;
        }
        let item = &matches[0];
        if let Some(PrAlias::Branch(existing)) =
            repo.pull_requests.get(&item.number)
            && existing != &branch
        {
            skipped_ambiguous.insert(branch, vec![item.number]);
            continue;
        }
        let (_, _, outcome) =
            apply_pr_metadata(home, slug, repo, item, None, None)?;
        if outcome.backup_created {
            cleanup_numbers.push(item.number);
        }
        match outcome.kind {
            AdoptionKind::Adopted => adopted.push(item.number),
            AdoptionKind::Merged => {
                merged.insert(item.number, outcome.review_mapping);
            }
            AdoptionKind::Unchanged => unchanged.push(item.number),
        }
    }
    write_state(home, &state)?;
    cleanup_adoption_backups(home, slug, cleanup_numbers)?;
    Ok(SyncOutput {
        repo: slug.to_owned(),
        local_repaired,
        recovered,
        adopted,
        merged,
        unchanged,
        skipped_zero,
        skipped_ambiguous,
    })
}

fn reconcile_repo_targets(
    home: &Path,
    slug: &str,
    repo: &mut Repo,
) -> Result<Vec<RepairOutput>> {
    let mut local_repaired = Vec::new();
    for (branch, target) in &mut repo.branches {
        let before = target.next_review_number;
        report_reconciled_target(
            home,
            slug,
            &format!("branch:{branch}"),
            target,
        )?;
        if before != target.next_review_number {
            local_repaired.push(RepairOutput {
                branch: branch.clone(),
                from: before,
                to: target.next_review_number,
            });
        }
    }
    Ok(local_repaired)
}

#[derive(Default)]
struct AdoptionArtifacts {
    stages: Vec<PathBuf>,
    branch_backup: Option<PathBuf>,
    pr_backup: Option<PathBuf>,
}

fn recover_incomplete_adoptions(
    home: &Path,
    slug: &str,
    repo: &Repo,
    metadata: &[PrMetadata],
) -> Result<Vec<RecoveryOutput>> {
    let repo_home = home.join(slug);
    if !repo_home.is_dir() {
        return Ok(Vec::new());
    }
    let mut branch_by_number = BTreeMap::new();
    for (number, alias) in &repo.pull_requests {
        if let PrAlias::Branch(branch) = alias {
            branch_by_number.insert(*number, branch.clone());
        }
    }
    for item in metadata {
        if let Ok(branch) = matching_branch(repo, item) {
            branch_by_number.entry(item.number).or_insert(branch);
        }
    }

    let mut artifacts: BTreeMap<u64, AdoptionArtifacts> = BTreeMap::new();
    for entry in fs::read_dir(&repo_home)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(rest) = name.strip_prefix(".adopt-pr-") else {
            continue;
        };
        let (number, suffix) = rest.split_once('-').with_context(|| {
            format!("unrecognized interrupted-adoption artifact {name}")
        })?;
        let number = number.parse::<u64>().with_context(|| {
            format!("unrecognized interrupted-adoption artifact {name}")
        })?;
        if !entry.file_type()?.is_dir() {
            bail!(
                "interrupted-adoption artifact {} is not a directory; it was preserved",
                entry.path().display()
            );
        }
        let item = artifacts.entry(number).or_default();
        match suffix {
            "branch-backup" => item.branch_backup = Some(entry.path()),
            "pr-backup" => item.pr_backup = Some(entry.path()),
            value
                if value.strip_prefix("stage-").is_some_and(|pid| {
                    !pid.is_empty()
                        && pid.bytes().all(|byte| byte.is_ascii_digit())
                }) =>
            {
                item.stages.push(entry.path());
            }
            _ => {
                bail!("unrecognized interrupted-adoption artifact {name}");
            }
        }
    }

    let mut recovered = Vec::new();
    for (number, item) in artifacts {
        let branch = branch_by_number.get(&number).with_context(|| {
            format!(
                "unfinished PR {number} adoption has no current branch alias or matching open-PR metadata; artifacts were preserved"
            )
        })?;
        let canonical_name = format!("pr-{number}");
        let canonical_path = repo_home.join(&canonical_name);
        let committed = matches!(
            repo.pull_requests.get(&number),
            Some(PrAlias::Branch(value)) if value == branch
        ) && repo
            .branches
            .get(branch)
            .is_some_and(|target| target.directory == canonical_name);

        if committed {
            if !canonical_path.is_dir() {
                bail!(
                    "unfinished PR {number} adoption is ambiguous: state points to {} but that directory is missing",
                    canonical_path.display()
                );
            }
            remove_recovery_paths(&item.stages)?;
            remove_optional_recovery_path(item.branch_backup.as_deref())?;
            remove_optional_recovery_path(item.pr_backup.as_deref())?;
            recovered.push(RecoveryOutput {
                pull_request: number,
                action: "finalized persisted adoption".to_owned(),
            });
            continue;
        }

        let target = repo.branches.get(branch).with_context(|| {
            format!(
                "unfinished PR {number} adoption is ambiguous: branch {branch} has no target"
            )
        })?;
        let branch_path = repo_home.join(&target.directory);
        match (item.branch_backup.as_deref(), item.pr_backup.as_deref()) {
            (Some(branch_backup), pr_backup) => {
                if branch_path.exists() {
                    bail!(
                        "unfinished PR {number} adoption is ambiguous: both branch target {} and backup {} exist",
                        branch_path.display(),
                        branch_backup.display()
                    );
                }
                if pr_backup.is_none()
                    && !item.stages.is_empty()
                    && !canonical_path.is_dir()
                {
                    bail!(
                        "unfinished PR {number} adoption is ambiguous: merge staging and branch backup exist but canonical PR source {} is missing; artifacts were preserved",
                        canonical_path.display()
                    );
                }
                if let Some(pr_backup) = pr_backup {
                    if canonical_path.exists() {
                        fs::remove_dir_all(&canonical_path)?;
                    }
                    // Keep the PR backup as a durable rollback discriminator
                    // until the branch is restored too. A crash while copying
                    // can then safely retry from the untouched backup.
                    copy_directory(pr_backup, &canonical_path)?;
                } else if item.stages.is_empty()
                    && !matches!(
                        repo.pull_requests.get(&number),
                        Some(PrAlias::Legacy(_))
                    )
                    && canonical_path.exists()
                {
                    // Branch-only adoption copied this directory before state
                    // persistence. A merge stage or legacy PR mapping means
                    // the canonical directory predates adoption and must be
                    // preserved.
                    fs::remove_dir_all(&canonical_path)?;
                }
                fs::rename(branch_backup, &branch_path)?;
                remove_optional_recovery_path(pr_backup)?;
                remove_recovery_paths(&item.stages)?;
                recovered.push(RecoveryOutput {
                    pull_request: number,
                    action: "rolled back unpersisted adoption".to_owned(),
                });
            }
            (None, Some(pr_backup)) => {
                if branch_path.is_dir() && canonical_path.is_dir() {
                    // A previous recovery restored both sources and crashed
                    // before deleting its final PR backup.
                    remove_optional_recovery_path(Some(pr_backup))?;
                    remove_recovery_paths(&item.stages)?;
                    recovered.push(RecoveryOutput {
                        pull_request: number,
                        action: "completed interrupted rollback".to_owned(),
                    });
                } else {
                    bail!(
                        "unfinished PR {number} adoption is ambiguous: PR backup {} exists without a branch backup",
                        pr_backup.display()
                    );
                }
            }
            (None, None) if !item.stages.is_empty() => {
                if !branch_path.is_dir() || !canonical_path.is_dir() {
                    bail!(
                        "unfinished PR {number} adoption is ambiguous: abandoned merge staging exists but source directories {} and {} are not both authoritative",
                        branch_path.display(),
                        canonical_path.display()
                    );
                }
                remove_recovery_paths(&item.stages)?;
                recovered.push(RecoveryOutput {
                    pull_request: number,
                    action: "discarded abandoned staging".to_owned(),
                });
            }
            (None, None) => {}
        }
    }
    Ok(recovered)
}

fn remove_recovery_paths(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

fn remove_optional_recovery_path(path: Option<&Path>) -> Result<()> {
    if let Some(path) = path
        && path.exists()
    {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn adopt_pull_request(
    home: &Path,
    slug: &str,
    repo: &mut Repo,
    number: u64,
    branch: &str,
) -> Result<AdoptionResult> {
    if let Some(alias) = repo.pull_requests.get(&number) {
        let existing = alias_branch(alias, number)?;
        if existing != branch {
            bail!("PR {number} already aliases branch {existing}");
        }
    }
    let target = repo
        .branches
        .get_mut(branch)
        .with_context(|| format!("cannot adopt missing branch {branch}"))?;
    let repo_home = home.join(slug);
    let branch_path = repo_home.join(&target.directory);
    let canonical_name = format!("pr-{number}");
    let canonical_path = repo_home.join(&canonical_name);
    let mut result = AdoptionResult {
        kind: AdoptionKind::Unchanged,
        review_mapping: BTreeMap::new(),
        backup_created: false,
    };
    if target.directory != canonical_name {
        if branch_path.exists() && canonical_path.exists() {
            result.review_mapping = merge_review_directories(
                &repo_home,
                &branch_path,
                &canonical_path,
                number,
            )?;
            result.kind = AdoptionKind::Merged;
            result.backup_created = true;
        } else if branch_path.exists() {
            backup_and_copy_branch_directory(
                &repo_home,
                &branch_path,
                &canonical_path,
                number,
            )?;
            result.kind = AdoptionKind::Adopted;
            result.backup_created = true;
        } else {
            fs::create_dir_all(&canonical_path)?;
            result.kind = AdoptionKind::Adopted;
        }
        target.directory = canonical_name;
    }
    repo.pull_requests
        .insert(number, PrAlias::Branch(branch.to_owned()));
    report_reconciled_target(home, slug, &format!("pr:{number}"), target)?;
    Ok(result)
}

fn backup_and_copy_branch_directory(
    repo_home: &Path,
    branch_path: &Path,
    canonical_path: &Path,
    number: u64,
) -> Result<()> {
    let backup = repo_home.join(format!(".adopt-pr-{number}-branch-backup"));
    if backup.exists() || canonical_path.exists() {
        bail!(
            "unfinished PR {number} adoption exists; recover its backup \
             before retrying"
        );
    }
    fs::rename(branch_path, &backup)?;
    if let Err(error) = copy_directory(&backup, canonical_path) {
        let _ = fs::remove_dir_all(canonical_path);
        let _ = fs::rename(&backup, branch_path);
        return Err(error);
    }
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn merge_review_directories(
    repo_home: &Path,
    branch_path: &Path,
    pr_path: &Path,
    number: u64,
) -> Result<BTreeMap<u64, u64>> {
    let stage = repo_home
        .join(format!(".adopt-pr-{number}-stage-{}", std::process::id()));
    let branch_backup =
        repo_home.join(format!(".adopt-pr-{number}-branch-backup"));
    let pr_backup = repo_home.join(format!(".adopt-pr-{number}-pr-backup"));
    if stage.exists() || branch_backup.exists() || pr_backup.exists() {
        bail!(
            "unfinished PR {number} adoption exists; recover the .adopt-pr-* \
             directories before retrying"
        );
    }
    fs::create_dir(&stage)?;
    let mapping = match prepare_merged_directory(branch_path, pr_path, &stage) {
        Ok(mapping) => mapping,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
    };
    fs::rename(branch_path, &branch_backup)?;
    if let Err(error) = fs::rename(pr_path, &pr_backup) {
        let _ = fs::rename(&branch_backup, branch_path);
        let _ = fs::remove_dir_all(&stage);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&stage, pr_path) {
        let _ = fs::rename(&pr_backup, pr_path);
        let _ = fs::rename(&branch_backup, branch_path);
        let _ = fs::remove_dir_all(&stage);
        return Err(error.into());
    }
    Ok(mapping)
}

fn prepare_merged_directory(
    branch_path: &Path,
    pr_path: &Path,
    stage: &Path,
) -> Result<BTreeMap<u64, u64>> {
    copy_directory_contents(branch_path, stage)?;
    let mut next = highest_review_number(stage)?.unwrap_or(0);
    let mut pr_reviews = review_paths(pr_path)?;
    pr_reviews.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(review_number_from_name)
            .unwrap_or(u64::MAX)
    });
    let mut mapping = BTreeMap::new();
    for source in pr_reviews {
        let old_number = source
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(review_number_from_name)
            .context("PR review has invalid file name")?;
        next = next.checked_add(1).context("review number overflow")?;
        let text = fs::read_to_string(&source)?;
        let rewritten = rewrite_review_number(&text, next)?;
        fs::write(stage.join(format!("rev-{next}.md")), rewritten)?;
        mapping.insert(old_number, next);
    }
    copy_pr_non_reviews(pr_path, stage)?;
    merge_context_files(branch_path, pr_path, stage)?;
    Ok(mapping)
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

fn copy_pr_non_reviews(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text == "context.md"
            || (entry.file_type()?.is_file()
                && review_number_from_name(&name_text).is_some())
        {
            continue;
        }
        let mut output = destination.join(&name);
        if output.exists() {
            output = destination.join(format!("pr-source-{name_text}"));
            if output.exists() {
                bail!(
                    "cannot preserve colliding PR entry {name_text}; \
                     destination {} also exists",
                    output.display()
                );
            }
        }
        copy_entry(&entry.path(), &output)?;
    }
    Ok(())
}

fn copy_entry(source: &Path, destination: &Path) -> Result<()> {
    if source.is_dir() {
        copy_directory(source, destination)
    } else {
        fs::copy(source, destination)?;
        Ok(())
    }
}

fn rewrite_review_number(text: &str, number: u64) -> Result<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let end = lines
        .iter()
        .skip(1)
        .position(|line| line == "---")
        .map(|index| index + 1)
        .context("review has no YAML front matter")?;
    let review = lines[..end]
        .iter()
        .position(|line| line.starts_with("review:"))
        .context("review YAML has no review field")?;
    lines[review] = format!("review: {number}");
    let mut output = lines.join("\n");
    if text.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

fn merge_context_files(
    branch_path: &Path,
    pr_path: &Path,
    stage: &Path,
) -> Result<()> {
    let branch = read_optional(branch_path.join("context.md"))?;
    let pr = read_optional(pr_path.join("context.md"))?;
    let content = match (branch, pr) {
        (Some(branch), Some(pr)) if branch == pr => branch,
        (Some(branch), Some(pr)) => combined_context(&branch, &pr),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => return Ok(()),
    };
    fs::write(stage.join("context.md"), content)?;
    Ok(())
}

fn combined_context(branch: &str, pr: &str) -> String {
    let mut output =
        String::from("# Combined review context\n\n## Branch source\n\n");
    output.push_str(branch);
    if !branch.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("\n## PR source\n\n");
    output.push_str(pr);
    if !pr.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn read_optional(path: PathBuf) -> Result<Option<String>> {
    match fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read {}", path.display())),
    }
}

fn cleanup_adoption_backups(
    home: &Path,
    slug: &str,
    numbers: impl IntoIterator<Item = u64>,
) -> Result<()> {
    let repo_home = home.join(slug);
    for number in numbers {
        for suffix in ["branch-backup", "pr-backup"] {
            let backup = repo_home.join(format!(".adopt-pr-{number}-{suffix}"));
            if backup.exists() {
                fs::remove_dir_all(backup)?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn target_output(
    home: &Path,
    slug: &str,
    discovery: Option<DiscoveryMetadata>,
    repo_path: &Path,
    target_name: &str,
    branch_name: &str,
    target: &Target,
    head_sha: &str,
) -> Result<TargetOutput> {
    let target_path = home.join(slug).join(&target.directory);
    fs::create_dir_all(&target_path)?;
    let context_path = target_path.join("context.md");
    if !context_path.exists() {
        fs::write(
            &context_path,
            "# Review context\n\nAdd product intent, constraints, references, and review emphasis here.\n",
        )?;
    }
    let reviews = review_paths(&target_path)?;
    let local_branch_sha =
        optional_exact_ref(repo_path, &format!("refs/heads/{branch_name}"));
    let origin_ref = discovery
        .as_ref()
        .map(|value| {
            format!("refs/remotes/{}/{}", value.selected_remote, branch_name)
        })
        .unwrap_or_else(|| format!("refs/remotes/origin/{branch_name}"));
    let origin_branch_sha = optional_exact_ref(repo_path, &origin_ref);
    let pr_head_sha = target.pull_request_head_sha.clone();
    let divergence = pr_head_sha
        .as_deref()
        .zip(local_branch_sha.as_deref())
        .is_some_and(|(github, local)| github != local);
    Ok(TargetOutput {
        repo: slug.to_owned(),
        discovery,
        repo_path: repo_path.to_owned(),
        target: target_name.to_owned(),
        target_path,
        context_path,
        base_ref: target.base_ref.clone(),
        base_sha: target.base_sha.clone(),
        head_sha: head_sha.to_owned(),
        head_authority: target_authority(target_name),
        local_branch_sha,
        origin_branch_sha,
        pr_head_sha,
        divergence,
        reviews,
    })
}

fn optional_exact_ref(worktree: &Path, reference: &str) -> Option<String> {
    let revision = format!("{reference}^{{commit}}");
    let output = Command::new("git")
        .args(["rev-parse", "--verify", &revision])
        .current_dir(worktree)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
}

fn review_template(
    target: &str,
    target_state: &Target,
    head_sha: &str,
    number: u64,
    label: Option<&str>,
) -> String {
    let label = label
        .map(|value| format!("label: {value}\n"))
        .unwrap_or_default();
    format!(
        "---\nreview: {number}\n{label}target: {target}\nbase_sha: {}\nhead_sha: {}\n---\n\n# Review: {target}\n\n## Scope\n\n## Summary\n\n## Findings\n\n- [ ] rev: P1 — Describe an actionable finding.\n\n## Recommendation\n",
        target_state.base_sha, head_sha
    )
}

fn review_paths(target_path: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(target_path)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .file_name()
                .to_str()
                .and_then(review_number_from_name)
                .is_some()
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn highest_review_number(target_path: &Path) -> Result<Option<u64>> {
    let mut highest = None;
    for entry in fs::read_dir(target_path)? {
        let entry = entry?;
        if let Some(number) =
            entry.file_name().to_str().and_then(review_number_from_name)
        {
            highest =
                Some(highest.map_or(number, |current: u64| current.max(number)));
        }
    }
    Ok(highest)
}

fn review_number_from_name(name: &str) -> Option<u64> {
    let number = name.strip_prefix("rev-")?.strip_suffix(".md")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = number.parse().ok()?;
    (number > 0).then_some(number)
}

fn target_status(
    home: &Path,
    slug: &str,
    target: String,
    state: &Target,
    discovery: Option<DiscoveryMetadata>,
) -> Result<TargetStatus> {
    let target_path = home.join(slug).join(&state.directory);
    let mut total = 0;
    let mut resolved = 0;
    let mut todo_findings = Vec::new();
    for review_path in review_paths(&target_path)? {
        for (index, line) in
            fs::read_to_string(&review_path)?.lines().enumerate()
        {
            if line.starts_with("- [x] rev:") {
                total += 1;
                resolved += 1;
            }
            if line.starts_with("- [ ] rev:") {
                total += 1;
                todo_findings.push(FindingOutput {
                    review_path: review_path.clone(),
                    line: index + 1,
                    text: line.to_owned(),
                });
            }
        }
    }
    Ok(TargetStatus {
        repo: slug.to_owned(),
        discovery,
        target,
        target_path,
        total,
        resolved,
        todo: todo_findings.len(),
        todo_findings,
    })
}

fn branch_directory(
    home: &Path,
    slug: &str,
    repo: &Repo,
    branch: &str,
) -> Result<String> {
    if let Some(target) = repo.branches.get(branch) {
        return Ok(target.directory.clone());
    }

    let canonical = format!("br-v2-{}", encode_component(branch));
    let legacy = format!("br-{}", sanitize_component(branch));
    if legacy == canonical
        || repo
            .branches
            .iter()
            .any(|(name, target)| name != branch && target.directory == legacy)
    {
        return Ok(canonical);
    }

    let legacy_path = home.join(slug).join(&legacy);
    if !legacy_path.is_dir() {
        return Ok(canonical);
    }
    let reviews = review_paths(&legacy_path)?;
    if reviews.is_empty() {
        return Ok(canonical);
    }

    let expected_target = format!("target: branch:{branch}");
    let belongs_to_branch = reviews.iter().all(|review| {
        fs::read_to_string(review)
            .map(|text| text.lines().any(|line| line == expected_target))
            .unwrap_or(false)
    });
    if belongs_to_branch {
        Ok(legacy)
    } else {
        Ok(canonical)
    }
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_')
            {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn git<const N: usize>(worktree: &Path, arguments: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(worktree)
        .output()
        .context("failed to invoke git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)
        .context("git output was not UTF-8")?
        .trim()
        .to_owned())
}

fn git_dynamic<'a>(
    worktree: &Path,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Result<String> {
    let arguments: Vec<_> = arguments.into_iter().collect();
    let output = Command::new("git")
        .args(&arguments)
        .current_dir(worktree)
        .output()
        .context("failed to invoke git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)
        .context("git output was not UTF-8")?
        .trim()
        .to_owned())
}

fn normalize_remote(input: &str) -> Result<String> {
    let input = input.trim().trim_end_matches(".git");
    let path = if let Some(value) = input.strip_prefix("git@") {
        value
            .split_once(':')
            .map(|(host, path)| format!("{host}/{path}"))
    } else if let Some(value) = input.strip_prefix("https://") {
        Some(value.to_owned())
    } else if let Some(value) = input.strip_prefix("ssh://git@") {
        value
            .split_once('/')
            .map(|(host, path)| format!("{host}/{path}"))
    } else {
        None
    }
    .context("origin must be a GitHub-style SSH or HTTPS URL")?;
    let components: Vec<_> = path.split('/').collect();
    if components.len() != 3 || components.iter().any(|value| value.is_empty()) {
        bail!("origin must have host/owner/repository form, got {input}");
    }
    Ok(path)
}

fn slug_from_remote(remote: &str) -> Result<String> {
    let components: Vec<_> = remote.split('/').collect();
    if components.len() != 3 {
        bail!("remote must have host/owner/repository form, got {remote}");
    }
    Ok(format!("{}__{}", components[1], components[2]).to_ascii_lowercase())
}

fn lock(home: &Path) -> Result<File> {
    let (file, lock_path) = open_lock_file(home)?;
    file.lock_exclusive()
        .with_context(|| format!("failed to lock {}", lock_path.display()))?;
    Ok(file)
}

fn try_lock(home: &Path) -> Result<File> {
    let (file, lock_path) = open_lock_file(home)?;
    for attempt in 0..LOCK_RETRY_ATTEMPTS {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && attempt + 1 < LOCK_RETRY_ATTEMPTS =>
            {
                thread::sleep(LOCK_RETRY_DELAY);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(anyhow!(
                    "ledger is busy; another gh-rev operation is in progress (lock {})",
                    lock_path.display()
                ));
            }
            Err(error) => {
                return Err(anyhow!(
                    "failed to lock {}: {error}",
                    lock_path.display()
                ));
            }
        }
    }
    unreachable!("lock retry loop has at least one attempt")
}

fn open_lock_file(home: &Path) -> Result<(File, PathBuf)> {
    fs::create_dir_all(home)
        .with_context(|| format!("failed to create {}", home.display()))?;
    let lock_path = home.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    Ok((file, lock_path))
}

fn read_state(home: &Path) -> Result<State> {
    let path = home.join(STATE_FILE);
    let state = match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            State::default()
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", path.display()));
        }
    };
    Ok(state)
}

fn write_state(home: &Path, state: &State) -> Result<()> {
    let path = home.join(STATE_FILE);
    let temporary =
        home.join(format!(".{STATE_FILE}.tmp-{}", std::process::id()));
    let serialized =
        toml::to_string_pretty(state).context("failed to serialize state")?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;
    file.write_all(serialized.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, &path).with_context(|| {
        format!("failed to atomically replace {}", path.display())
    })?;
    Ok(())
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_HOME_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn normalizes_common_github_remote_forms() {
        assert_eq!(
            normalize_remote("git@github.com:NibiruChain/sai-website.git")
                .unwrap(),
            "github.com/NibiruChain/sai-website"
        );
        assert_eq!(
            normalize_remote("https://github.com/NibiruChain/sai-website.git")
                .unwrap(),
            "github.com/NibiruChain/sai-website"
        );
    }

    #[test]
    fn derives_owner_repository_slug_without_host() {
        assert_eq!(
            slug_from_remote("github.com/NibiruChain/sai-website").unwrap(),
            "nibiruchain__sai-website"
        );
    }

    #[test]
    fn discovery_uses_selected_worktree_and_branch_remote() {
        let fixture = test_fixture("discovery");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        run_git(
            &fixture.worktree,
            [
                "config",
                &format!("branch.{}.remote", fixture.branch),
                "origin",
            ],
        );
        let discovery =
            discover_repository(Some(&fixture.worktree), None).unwrap();
        assert_eq!(discovery.slug, fixture.slug);
        assert_eq!(discovery.branch.as_deref(), Some(fixture.branch.as_str()));
        assert_eq!(discovery.remote_name, "origin");
        assert_eq!(discovery.source, "-C");
        assert_eq!(discovery.worktree_root, fixture.worktree);
        fixture.remove();
    }

    #[test]
    fn discovery_prefers_cwd_when_no_explicit_path() {
        let fixture = unregistered_fixture("discovery-cwd");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        let original = env::current_dir().unwrap();
        env::set_current_dir(&fixture.worktree).unwrap();
        let discovery = discover_repository(None, None).unwrap();
        env::set_current_dir(original).unwrap();
        assert_eq!(discovery.slug, fixture.slug);
        assert_eq!(discovery.source, "cwd");
        fixture.remove();
    }

    #[test]
    fn discover_fails_cleanly_from_non_git_path() {
        let path = temp_home("not-git");
        fs::create_dir_all(&path).unwrap();
        let error = discover_repository(Some(&path), None).unwrap_err();
        assert!(
            error.to_string().contains("not a git repository")
                || error.to_string().contains("fatal")
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn discovery_works_from_detached_head() {
        let fixture = unregistered_fixture("discovery-detached");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        let head = git(&fixture.worktree, ["rev-parse", "HEAD"]).unwrap();
        run_git(&fixture.worktree, ["checkout", "--detach", &head]);
        let discovery =
            discover_repository(Some(&fixture.worktree), None).unwrap();
        assert!(discovery.branch.is_none());
        assert_eq!(discovery.remote_name, "origin");
        fixture.remove();
    }

    #[test]
    fn discover_remote_precedence_upstream_before_push_default() {
        let fixture = unregistered_fixture("remote-precedence-upstream");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/origin.git",
            ],
        );
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "upstream",
                "git@github.com:example/upstream.git",
            ],
        );
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "mirror",
                "git@github.com:example/mirror.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        run_git(
            &fixture.worktree,
            [
                "config",
                &format!("branch.{}.remote", fixture.branch),
                "upstream",
            ],
        );
        run_git(
            &fixture.worktree,
            ["config", "remote.pushDefault", "mirror"],
        );
        let discovery =
            discover_repository(Some(&fixture.worktree), None).unwrap();
        assert_eq!(discovery.remote_name, "upstream");
        fixture.remove();
    }

    #[test]
    fn discover_remote_precedence_push_default_before_origin_and_sole() {
        let fixture = unregistered_fixture("remote-precedence-pushdefault");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/origin.git",
            ],
        );
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "mirror",
                "git@github.com:example/mirror.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        run_git(
            &fixture.worktree,
            ["config", "remote.pushDefault", "mirror"],
        );
        let discovery =
            discover_repository(Some(&fixture.worktree), None).unwrap();
        assert_eq!(discovery.remote_name, "mirror");
        run_git(
            &fixture.worktree,
            ["config", "--unset", "remote.pushDefault"],
        );
        run_git(&fixture.worktree, ["remote", "remove", "origin"]);
        let discovery =
            discover_repository(Some(&fixture.worktree), None).unwrap();
        assert_eq!(discovery.remote_name, "mirror");
        fixture.remove();
    }

    #[test]
    fn discover_remote_selection_rejects_ambiguous_remote_sets() {
        let fixture = unregistered_fixture("remote-ambiguous");
        run_git(
            &fixture.worktree,
            ["remote", "add", "first", "git@github.com:example/first.git"],
        );
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "second",
                "git@github.com:example/second.git",
            ],
        );
        let error =
            discover_repository(Some(&fixture.worktree), None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("could not select a Git remote"));
        assert!(message.contains("pass --remote <name>"));
        assert!(message.contains("first"));
        assert!(message.contains("second"));
        fixture.remove();
    }

    #[test]
    fn explicit_remote_override_ignores_unrelated_remote_config() {
        let fixture = unregistered_fixture("remote-override");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/origin.git",
            ],
        );
        run_git(
            &fixture.worktree,
            ["remote", "add", "alternate", "noremote"],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        run_git(
            &fixture.worktree,
            [
                "config",
                &format!("branch.{}.remote", fixture.branch),
                "alternate",
            ],
        );
        let discovery =
            discover_repository(Some(&fixture.worktree), Some("origin"))
                .unwrap();
        assert_eq!(discovery.remote_name, "origin");
        fixture.remove();
    }

    #[test]
    fn open_discovers_a_worktree_without_prior_registration() {
        let fixture = unregistered_fixture("open-discovery");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);

        open_with_head_with_discovered_pr_fetch(
            &fixture.home,
            None,
            Some(&fixture.worktree),
            None,
            None,
            None,
            "main",
            None,
            |_, _| Ok(None),
        )
        .unwrap();
        open_with_head_with_discovered_pr_fetch(
            &fixture.home,
            None,
            Some(&fixture.worktree),
            None,
            None,
            None,
            "main",
            None,
            |_, _| Ok(None),
        )
        .unwrap();
        let state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get(&fixture.slug).unwrap();
        assert_eq!(repo.path, fixture.worktree);
        assert!(repo.branches.contains_key(&fixture.branch));
        assert_eq!(state.repos.len(), 1);
        fixture.remove();
    }

    #[test]
    fn open_from_discovered_worktree_adopts_matching_pr_history() {
        let fixture = unregistered_fixture("open-discovery-pr");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        let head =
            git(&fixture.worktree, ["rev-parse", &fixture.branch]).unwrap();
        let metadata = pr_metadata(&fixture, 1153, &head);

        open_with_head_with_discovered_pr_fetch(
            &fixture.home,
            None,
            Some(&fixture.worktree),
            None,
            None,
            None,
            "main",
            None,
            |discovery, branch| {
                assert_eq!(discovery.remote, "github.com/example/reviews");
                assert_eq!(branch, Some(fixture.branch.as_str()));
                Ok(Some((discovery.remote.clone(), metadata)))
            },
        )
        .unwrap();

        let state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get(&fixture.slug).unwrap();
        assert_eq!(
            alias_branch(repo.pull_requests.get(&1153).unwrap(), 1153).unwrap(),
            fixture.branch
        );
        assert_eq!(repo.branches[&fixture.branch].directory, "pr-1153");
        assert!(fixture.home.join(&fixture.slug).join("pr-1153").is_dir());
        fixture.remove();
    }

    #[test]
    fn discovered_pr_lookup_rejects_multiple_exact_matches() {
        let fixture = unregistered_fixture("discovery-pr-ambiguous");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        let discovery =
            discover_repository(Some(&fixture.worktree), None).unwrap();
        let head =
            git(&fixture.worktree, ["rev-parse", &fixture.branch]).unwrap();
        let first = pr_metadata(&fixture, 1153, &head);
        let second = pr_metadata(&fixture, 1154, &head);

        let error = fetch_discovered_pr_with(
            &discovery,
            Some(&fixture.branch),
            |_, _| Ok(vec![first, second]),
        )
        .unwrap_err();
        assert!(error.to_string().contains("multiple open pull requests"));
        fixture.remove();
    }

    #[test]
    fn next_discovers_a_worktree_without_prior_registration() {
        let fixture = unregistered_fixture("next-discovery");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        next_with_head_with_discovered_pr_fetch(
            &fixture.home,
            None,
            Some(&fixture.worktree),
            None,
            None,
            None,
            None,
            None,
            |_, _| Ok(None),
        )
        .unwrap();
        let state = read_state(&fixture.home).unwrap();
        assert!(state.repos.contains_key(&fixture.slug));
        assert!(
            state
                .repos
                .get(&fixture.slug)
                .unwrap()
                .branches
                .contains_key(&fixture.branch)
        );
        fixture.remove();
    }

    #[test]
    fn context_discovers_a_worktree_without_prior_registration() {
        let fixture = unregistered_fixture("context-discovery");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        context_with_discovered_pr_fetch(
            &fixture.home,
            None,
            Some(&fixture.worktree),
            None,
            None,
            None,
            |_, _| Ok(None),
        )
        .unwrap();
        let state = read_state(&fixture.home).unwrap();
        assert!(state.repos.contains_key(&fixture.slug));
        assert!(
            state
                .repos
                .get(&fixture.slug)
                .unwrap()
                .branches
                .contains_key(&fixture.branch)
        );
        fixture.remove();
    }

    #[test]
    fn status_discovers_a_worktree_without_prior_registration() {
        let fixture = unregistered_fixture("status-discovery");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        status(
            &fixture.home,
            None,
            None,
            None,
            None,
            None,
            Some(&fixture.worktree),
        )
        .unwrap();
        let state = read_state(&fixture.home).unwrap();
        assert!(state.repos.contains_key(&fixture.slug));
        fixture.remove();
    }

    #[test]
    fn sync_discovers_a_worktree_without_prior_registration() {
        let fixture = unregistered_fixture("sync-discovery");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        sync_with(&fixture.home, None, Some(&fixture.worktree), None, |_| {
            Ok(Vec::new())
        })
        .unwrap();
        let state = read_state(&fixture.home).unwrap();
        assert!(state.repos.contains_key(&fixture.slug));
        fixture.remove();
    }

    #[test]
    fn explicit_repo_argument_uses_existing_registration() {
        let fixture = test_fixture("explicit-repo-compatible");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        open_with_head(
            &fixture.home,
            Some(&fixture.slug),
            None,
            None,
            Some(fixture.branch.clone()),
            None,
            "main",
            None,
        )
        .unwrap();
        assert!(
            read_state(&fixture.home)
                .unwrap()
                .repos
                .contains_key(&fixture.slug)
        );
        fixture.remove();
    }

    #[test]
    fn state_round_trips_through_atomic_writer() {
        let home = temp_home("state");
        let mut state = State::default();
        state.repos.insert(
            "nibiruchain__sai-website".to_owned(),
            Repo {
                path: PathBuf::from("/tmp/sai-website"),
                remote: "github.com/NibiruChain/sai-website".to_owned(),
                branches: BTreeMap::new(),
                pull_requests: BTreeMap::new(),
            },
        );
        fs::create_dir_all(&home).unwrap();
        write_state(&home, &state).unwrap();
        let read = read_state(&home).unwrap();
        assert_eq!(read.repos.len(), 1);
        assert!(
            !home
                .join(format!(".{STATE_FILE}.tmp-{}", std::process::id()))
                .exists()
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn open_repairs_missing_target_state_then_next_allocates_after_artifacts() {
        let fixture = test_fixture("open-missing-state");
        write_review(&fixture.target_path, 1);
        write_review(&fixture.target_path, 2);

        open(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            "main",
        )
        .unwrap();
        assert_eq!(branch_target(&fixture).next_review_number, 3);

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            Some("follow-up".to_owned()),
        )
        .unwrap();
        assert!(fixture.target_path.join("rev-3.md").is_file());
        assert!(fixture.target_path.join("rev-1.md").is_file());
        assert!(fixture.target_path.join("rev-2.md").is_file());
        assert_eq!(branch_target(&fixture).next_review_number, 4);
        fixture.remove();
    }

    #[test]
    fn next_directly_recovers_a_missing_target_state() {
        let fixture = test_fixture("next-missing-state");
        write_review(&fixture.target_path, 1);
        write_review(&fixture.target_path, 2);

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        assert!(fixture.target_path.join("rev-3.md").is_file());
        assert_eq!(branch_target(&fixture).next_review_number, 4);
        fixture.remove();
    }

    #[test]
    fn direct_next_allocation_starts_at_one_when_no_reviews_exist() {
        let fixture = test_fixture("next-no-reviews");

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        assert!(fixture.target_path.join("rev-1.md").is_file());
        assert_eq!(branch_target(&fixture).next_review_number, 2);
        fixture.remove();
    }

    #[test]
    fn stale_counter_is_advanced_past_the_highest_review_artifact() {
        let fixture = test_fixture("stale-counter");
        write_review(&fixture.target_path, 1);
        write_review(&fixture.target_path, 5);
        insert_branch_target(&fixture, 2);

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        assert!(fixture.target_path.join("rev-6.md").is_file());
        assert_eq!(branch_target(&fixture).next_review_number, 7);
        fixture.remove();
    }

    #[test]
    fn numbering_gaps_allocate_after_the_highest_valid_review() {
        let fixture = test_fixture("gaps");
        write_review(&fixture.target_path, 1);
        write_review(&fixture.target_path, 3);

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        assert!(fixture.target_path.join("rev-4.md").is_file());
        assert!(!fixture.target_path.join("rev-2.md").exists());
        fixture.remove();
    }

    #[test]
    fn many_sparse_review_files_reconcile_to_next_review_number() {
        let fixture = test_fixture("many-reviews");
        for number in [9, 1, 12, 3, 5, 7, 11, 2, 4, 6, 8, 10].into_iter() {
            write_review(&fixture.target_path, number);
        }

        // Add common noise that should be ignored by parsing logic.
        fs::write(fixture.target_path.join("rev-12.md.bak"), "noise\n").unwrap();
        fs::write(fixture.target_path.join("rev-x.md"), "noise\n").unwrap();
        fs::write(fixture.target_path.join("review-99.md"), "noise\n").unwrap();
        fs::write(fixture.target_path.join("rev-0.md"), "noise\n").unwrap();

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        assert!(fixture.target_path.join("rev-13.md").is_file());
        assert!(!fixture.target_path.join("rev-14.md").exists());
        assert_eq!(branch_target(&fixture).next_review_number, 14);
        fixture.remove();
    }

    #[test]
    fn unrelated_filenames_do_not_affect_review_allocation() {
        let fixture = test_fixture("unrelated-files");
        write_review(&fixture.target_path, 1);
        fs::write(fixture.target_path.join("rev-2.md.bak"), "").unwrap();
        fs::write(fixture.target_path.join("rev-x.md"), "").unwrap();
        fs::write(fixture.target_path.join("review-99.md"), "").unwrap();
        fs::write(fixture.target_path.join("rev-0.md"), "").unwrap();

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        assert!(fixture.target_path.join("rev-2.md").is_file());
        fixture.remove();
    }

    #[test]
    fn occupied_review_directory_advances_allocation() {
        let fixture = test_fixture("occupied-directory");
        fs::create_dir(fixture.target_path.join("rev-1.md")).unwrap();

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        assert!(fixture.target_path.join("rev-2.md").is_file());
        assert_eq!(branch_target(&fixture).next_review_number, 3);
        fixture.remove();
    }

    #[test]
    fn colliding_legacy_branch_names_use_distinct_directories() {
        let fixture = test_fixture("branch-collision");
        let other_branch = "feature-reconcile".to_owned();
        run_git(&fixture.worktree, ["branch", &other_branch]);

        open(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            "main",
        )
        .unwrap();
        open(
            &fixture.home,
            &fixture.slug,
            Some(other_branch.clone()),
            None,
            "main",
        )
        .unwrap();

        let mut state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get_mut(&fixture.slug).unwrap();
        let first = repo.branches.remove(&fixture.branch).unwrap();
        let second = repo.branches.remove(&other_branch).unwrap();
        assert_ne!(first.directory, second.directory);
        assert!(first.directory.contains("%2F"));
        fixture.remove();
    }

    #[test]
    fn legacy_directory_is_adopted_only_for_its_recorded_branch() {
        let fixture = test_fixture("legacy-directory");
        let legacy_path = fixture
            .home
            .join(&fixture.slug)
            .join(format!("br-{}", sanitize_component(&fixture.branch)));
        fs::create_dir_all(&legacy_path).unwrap();
        fs::write(
            legacy_path.join("rev-1.md"),
            format!("---\nreview: 1\ntarget: branch:{}\n---\n", fixture.branch),
        )
        .unwrap();

        open(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            "main",
        )
        .unwrap();

        assert_eq!(
            branch_target(&fixture).directory,
            legacy_path.file_name().unwrap().to_str().unwrap()
        );
        fixture.remove();
    }

    #[test]
    fn encoded_name_does_not_steal_sanitized_legacy_directory() {
        let fixture = test_fixture("encoded-sanitized-owner");
        let encoded_branch = "feature-reconcile".to_owned();
        run_git(&fixture.worktree, ["branch", &encoded_branch]);
        let legacy_path = fixture
            .home
            .join(&fixture.slug)
            .join("br-feature-reconcile");
        fs::create_dir_all(&legacy_path).unwrap();
        fs::write(
            legacy_path.join("rev-1.md"),
            format!("---\nreview: 1\ntarget: branch:{}\n---\n", fixture.branch),
        )
        .unwrap();

        open(
            &fixture.home,
            &fixture.slug,
            Some(encoded_branch.clone()),
            None,
            "main",
        )
        .unwrap();
        open(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            "main",
        )
        .unwrap();

        let state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get(&fixture.slug).unwrap();
        assert_eq!(
            repo.branches.get(&fixture.branch).unwrap().directory,
            "br-feature-reconcile"
        );
        assert_eq!(
            repo.branches.get(&encoded_branch).unwrap().directory,
            "br-v2-feature-reconcile"
        );
        fixture.remove();
    }

    #[test]
    fn review_uses_named_branch_sha_when_worktree_is_elsewhere() {
        let fixture = test_fixture("branch-head");
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        fs::write(fixture.worktree.join("branch.txt"), "first\n").unwrap();
        run_git(&fixture.worktree, ["add", "branch.txt"]);
        commit_test_change(&fixture.worktree, "first branch commit");
        let first_head =
            git(&fixture.worktree, ["rev-parse", &fixture.branch]).unwrap();
        run_git(&fixture.worktree, ["checkout", "main"]);

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        let first_review =
            fs::read_to_string(fixture.target_path.join("rev-1.md")).unwrap();
        assert!(first_review.contains(&format!("head_sha: {first_head}")));
        assert_eq!(branch_target(&fixture).last_reviewed_head_sha, first_head);

        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        fs::write(fixture.worktree.join("branch.txt"), "second\n").unwrap();
        run_git(&fixture.worktree, ["add", "branch.txt"]);
        commit_test_change(&fixture.worktree, "second branch commit");
        let second_head =
            git(&fixture.worktree, ["rev-parse", &fixture.branch]).unwrap();
        run_git(&fixture.worktree, ["checkout", "main"]);

        next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();

        let second_review =
            fs::read_to_string(fixture.target_path.join("rev-2.md")).unwrap();
        assert!(second_review.contains(&format!("head_sha: {second_head}")));
        assert_eq!(branch_target(&fixture).last_reviewed_head_sha, second_head);
        fixture.remove();
    }

    #[test]
    fn parses_status_repo_and_todo_forms() {
        assert_eq!(parse_status_positionals(None, None).unwrap(), (false, None));
        assert_eq!(
            parse_status_positionals(Some("example__reviews".into()), None)
                .unwrap(),
            (false, Some("example__reviews".into()))
        );
        assert_eq!(
            parse_status_positionals(
                Some("todo".into()),
                Some("example__reviews".into())
            )
            .unwrap(),
            (true, Some("example__reviews".into()))
        );
        assert!(
            parse_status_positionals(
                Some("invalid".into()),
                Some("example__reviews".into())
            )
            .is_err()
        );
    }

    #[test]
    fn repeated_registration_preserves_review_targets() {
        let fixture = test_fixture("repeat-register");
        run_git(
            &fixture.worktree,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:example/reviews.git",
            ],
        );
        insert_branch_target(&fixture, 4);

        register(&fixture.home, &fixture.worktree).unwrap();

        let state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get(&fixture.slug).unwrap();
        assert_eq!(repo.branches.len(), 1);
        assert_eq!(
            repo.branches
                .get(&fixture.branch)
                .unwrap()
                .next_review_number,
            4
        );
        fixture.remove();
    }

    #[test]
    fn version_flag_parses_without_subcommand() {
        let cli = Cli::parse_from(["gh-rev", "--version"]);
        assert!(cli.version);
        assert!(cli.command.is_none());
    }

    #[test]
    fn version_output_is_json_with_expected_fields() {
        let output = version_output();
        let json = serde_json::to_string_pretty(&output).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value["version"].as_str().unwrap(),
            APP_VERSION,
            "version should reflect build metadata"
        );
        assert_eq!(
            value["commit"].as_str().map(|value| !value.is_empty()),
            Some(true)
        );
        assert!(
            value["dirty"].as_bool().is_some() || value["dirty"].is_null(),
            "dirty must be boolean or null when Git metadata is unavailable"
        );
        if APP_GIT_COMMIT != "unknown" {
            assert_eq!(
                APP_GIT_COMMIT,
                git(
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    ["rev-parse", "HEAD"]
                )
                .unwrap()
            );
        }
    }

    #[test]
    fn exact_branch_resolution_ignores_same_named_tag() {
        let fixture = test_fixture("branch-tag");
        run_git(&fixture.worktree, ["tag", &fixture.branch]);
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        fs::write(fixture.worktree.join("branch-only.txt"), "branch\n").unwrap();
        run_git(&fixture.worktree, ["add", "branch-only.txt"]);
        commit_test_change(&fixture.worktree, "advance branch");
        let expected = git(&fixture.worktree, ["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            exact_branch_sha(&fixture.worktree, &fixture.branch).unwrap(),
            expected
        );
        fixture.remove();
    }

    #[test]
    fn branch_only_pr_adoption_renames_directory_and_aliases_target() {
        let fixture = test_fixture("branch-only-adoption");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        insert_branch_target(&fixture, 2);
        let metadata = pr_metadata(&fixture, 1153, "github-head");
        let mut state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get_mut(&fixture.slug).unwrap();

        apply_pr_metadata(
            &fixture.home,
            &fixture.slug,
            repo,
            &metadata,
            None,
            None,
        )
        .unwrap();

        let canonical = fixture.home.join(&fixture.slug).join("pr-1153");
        assert!(canonical.join("rev-1.md").is_file());
        assert!(!fixture.target_path.exists());
        assert_eq!(
            alias_branch(repo.pull_requests.get(&1153).unwrap(), 1153).unwrap(),
            fixture.branch
        );
        assert_eq!(
            repo.branches.get(&fixture.branch).unwrap().directory,
            "pr-1153"
        );
        fixture.remove();
    }

    #[test]
    fn dual_history_merge_preserves_branch_numbers_and_appends_pr_reviews() {
        let fixture = test_fixture("dual-adoption");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        write_review_yaml(&fixture.target_path, 3, "branch:evidence");
        let branch_attachment = fixture.target_path.join("attachments");
        fs::create_dir(&branch_attachment).unwrap();
        fs::write(branch_attachment.join("branch.txt"), "branch\n").unwrap();
        fs::write(fixture.target_path.join("context.md"), "branch context\n")
            .unwrap();
        insert_branch_target(&fixture, 4);
        let pr_path = fixture.home.join(&fixture.slug).join("pr-1153");
        fs::create_dir_all(&pr_path).unwrap();
        write_review_yaml(&pr_path, 1, "pr:1153");
        write_review_yaml(&pr_path, 2, "pr:1153");
        let pr_attachment = pr_path.join("pr-attachments");
        fs::create_dir(&pr_attachment).unwrap();
        fs::write(pr_attachment.join("pr.txt"), "pr\n").unwrap();
        fs::write(pr_path.join("context.md"), "PR context\n").unwrap();
        let metadata = pr_metadata(&fixture, 1153, "07");
        let mut state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get_mut(&fixture.slug).unwrap();

        let (_, _, merged) = apply_pr_metadata(
            &fixture.home,
            &fixture.slug,
            repo,
            &metadata,
            None,
            None,
        )
        .unwrap();
        let (_, _, repeated) = apply_pr_metadata(
            &fixture.home,
            &fixture.slug,
            repo,
            &metadata,
            None,
            None,
        )
        .unwrap();

        assert_eq!(merged.kind, AdoptionKind::Merged);
        assert_eq!(merged.review_mapping.get(&1), Some(&4));
        assert_eq!(merged.review_mapping.get(&2), Some(&5));
        assert_eq!(repeated.kind, AdoptionKind::Unchanged);
        for number in [1, 3, 4, 5] {
            assert!(pr_path.join(format!("rev-{number}.md")).is_file());
        }
        assert!(!pr_path.join("rev-2.md").exists());
        assert!(
            fs::read_to_string(pr_path.join("rev-4.md"))
                .unwrap()
                .contains("review: 4")
        );
        let target = repo.branches.get(&fixture.branch).unwrap();
        assert_eq!(target.next_review_number, 6);
        let context = fs::read_to_string(pr_path.join("context.md")).unwrap();
        assert!(context.contains("## Branch source"));
        assert!(context.contains("## PR source"));
        assert!(pr_path.join("attachments/branch.txt").is_file());
        assert!(pr_path.join("pr-attachments/pr.txt").is_file());
        fixture.remove();
    }

    #[test]
    fn pr_matching_rejects_forks_and_unregistered_branches() {
        let fixture = test_fixture("pr-matching");
        insert_branch_target(&fixture, 1);
        let state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get(&fixture.slug).unwrap();
        let exact = pr_metadata(&fixture, 7, "head");
        assert_eq!(matching_branch(repo, &exact).unwrap(), fixture.branch);
        let mut fork = exact.clone();
        fork.head_repository_owner.login = "fork-owner".to_owned();
        assert!(matching_branch(repo, &fork).is_err());
        let mut zero = exact;
        zero.head_ref_name = "missing".to_owned();
        assert!(matching_branch(repo, &zero).is_err());
        fixture.remove();
    }

    #[test]
    fn adopted_pr_divergence_requires_explicit_local_branch_policy() {
        let fixture = test_fixture("divergence-1153");
        insert_branch_target(&fixture, 372);
        let metadata = pr_metadata(&fixture, 1153, "07");
        let mut state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get_mut(&fixture.slug).unwrap();
        apply_pr_metadata(
            &fixture.home,
            &fixture.slug,
            repo,
            &metadata,
            None,
            None,
        )
        .unwrap();
        write_state(&fixture.home, &state).unwrap();

        assert!(
            open_with_head(
                &fixture.home,
                Some(&fixture.slug),
                None,
                None,
                Some(fixture.branch.clone()),
                None,
                "main",
                None,
            )
            .is_err()
        );
        open_with_head(
            &fixture.home,
            Some(&fixture.slug),
            None,
            None,
            Some(fixture.branch.clone()),
            None,
            "main",
            Some(HeadPolicy::Local),
        )
        .unwrap();
        fixture.remove();
    }

    #[test]
    fn next_rejects_branch_head_movement_after_open() {
        let fixture = test_fixture("pending-movement");
        open(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            "main",
        )
        .unwrap();
        run_git(&fixture.worktree, ["checkout", &fixture.branch]);
        fs::write(fixture.worktree.join("moved.txt"), "moved\n").unwrap();
        run_git(&fixture.worktree, ["add", "moved.txt"]);
        commit_test_change(&fixture.worktree, "move selected ref");

        let result = next(
            &fixture.home,
            &fixture.slug,
            Some(fixture.branch.clone()),
            None,
            None,
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("reopen to re-resolve")
        );
        assert!(!fixture.target_path.join("rev-1.md").exists());
        fixture.remove();
    }

    #[test]
    fn github_failure_still_persists_local_counter_repairs() {
        let fixture = test_fixture("offline-repair");
        write_review(&fixture.target_path, 5);
        insert_branch_target(&fixture, 2);

        let result = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Err(anyhow::anyhow!("gh unavailable"))
        });

        assert_eq!(
            result.unwrap_err().to_string(),
            "github: unavailable: gh unavailable"
        );
        assert_eq!(branch_target(&fixture).next_review_number, 6);
        fixture.remove();
    }

    #[test]
    fn sync_fetches_github_metadata_without_holding_the_ledger_lock() {
        let fixture = test_fixture("fetch-outside-lock");
        insert_branch_target(&fixture, 1);
        let mut acquired = false;

        sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            let probe = try_lock(&fixture.home)?;
            acquired = true;
            drop(probe);
            Ok(Vec::new())
        })
        .unwrap();

        assert!(acquired);
        fixture.remove();
    }

    #[test]
    fn targeted_pr_fetches_metadata_without_holding_the_ledger_lock() {
        let fixture = test_fixture("targeted-fetch-outside-lock");
        let metadata = pr_metadata(&fixture, 1153, "head");
        let mut acquired = false;

        let output = fetch_targeted_pr_without_lock_with(
            &fixture.home,
            &fixture.slug,
            Some(1153),
            |_, number| {
                let probe = try_lock(&fixture.home)?;
                acquired = true;
                drop(probe);
                assert_eq!(number, 1153);
                Ok(metadata)
            },
        )
        .unwrap();

        assert!(acquired);
        assert_eq!(output.unwrap().1.number, 1153);
        fixture.remove();
    }

    #[test]
    fn sync_reports_live_lock_contention_without_running_fetch() {
        let fixture = test_fixture("sync-contention");
        let held = lock(&fixture.home).unwrap();
        let mut fetched = false;

        let error = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            fetched = true;
            Ok(Vec::new())
        })
        .unwrap_err();

        assert!(error.to_string().contains("ledger is busy"));
        assert!(!fetched);
        drop(held);
        fixture.remove();
    }

    #[test]
    fn sync_discards_abandoned_stage_before_adoption() {
        let fixture = test_fixture("recover-stage");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        insert_branch_target(&fixture, 1);
        let canonical = fixture.home.join(&fixture.slug).join("pr-1153");
        fs::create_dir(&canonical).unwrap();
        write_review_yaml(&canonical, 1, "pr:1153");
        let stage = fixture
            .home
            .join(&fixture.slug)
            .join(".adopt-pr-1153-stage-999");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("partial"), "partial\n").unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let output = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap();

        assert_eq!(
            output.recovered,
            vec![RecoveryOutput {
                pull_request: 1153,
                action: "discarded abandoned staging".to_owned(),
            }]
        );
        assert!(!stage.exists());
        assert_eq!(output.merged.get(&1153).unwrap().get(&1), Some(&2));
        fixture.remove();
    }

    #[test]
    fn sync_preserves_pr_history_after_crash_between_backup_renames() {
        let fixture = test_fixture("recover-mid-merge");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        insert_branch_target(&fixture, 2);
        let repo_home = fixture.home.join(&fixture.slug);
        let canonical = repo_home.join("pr-1153");
        fs::create_dir(&canonical).unwrap();
        write_review_yaml(&canonical, 1, "pr:1153");
        let stage = repo_home.join(".adopt-pr-1153-stage-999");
        fs::create_dir(&stage).unwrap();
        prepare_merged_directory(&fixture.target_path, &canonical, &stage)
            .unwrap();
        let branch_backup = repo_home.join(".adopt-pr-1153-branch-backup");
        fs::rename(&fixture.target_path, &branch_backup).unwrap();
        let original_pr =
            fs::read_to_string(canonical.join("rev-1.md")).unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let output = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap();

        assert_eq!(
            output.recovered,
            vec![RecoveryOutput {
                pull_request: 1153,
                action: "rolled back unpersisted adoption".to_owned(),
            }]
        );
        assert_eq!(
            fs::read_to_string(canonical.join("rev-2.md")).unwrap(),
            rewrite_review_number(&original_pr, 2).unwrap()
        );
        assert!(canonical.join("rev-1.md").is_file());
        assert!(!branch_backup.exists());
        fixture.remove();
    }

    #[test]
    fn sync_rolls_back_unpersisted_adoption_before_retrying() {
        let fixture = test_fixture("recover-rollback");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        insert_branch_target(&fixture, 2);
        let canonical = fixture.home.join(&fixture.slug).join("pr-1153");
        backup_and_copy_branch_directory(
            &fixture.home.join(&fixture.slug),
            &fixture.target_path,
            &canonical,
            1153,
        )
        .unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let output = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap();

        assert_eq!(
            output.recovered,
            vec![RecoveryOutput {
                pull_request: 1153,
                action: "rolled back unpersisted adoption".to_owned(),
            }]
        );
        assert_eq!(output.adopted, vec![1153]);
        assert!(canonical.join("rev-1.md").is_file());
        assert!(
            !fixture
                .home
                .join(&fixture.slug)
                .join(".adopt-pr-1153-branch-backup")
                .exists()
        );
        fixture.remove();
    }

    #[test]
    fn sync_completes_rollback_after_sources_were_restored() {
        let fixture = test_fixture("recover-complete-rollback");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        insert_branch_target(&fixture, 2);
        let repo_home = fixture.home.join(&fixture.slug);
        let canonical = repo_home.join("pr-1153");
        fs::create_dir(&canonical).unwrap();
        write_review_yaml(&canonical, 1, "pr:1153");
        let pr_backup = repo_home.join(".adopt-pr-1153-pr-backup");
        copy_directory(&canonical, &pr_backup).unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let output = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap();

        assert_eq!(
            output.recovered,
            vec![RecoveryOutput {
                pull_request: 1153,
                action: "completed interrupted rollback".to_owned(),
            }]
        );
        assert!(!pr_backup.exists());
        assert_eq!(output.merged.get(&1153).unwrap().get(&1), Some(&2));
        assert!(canonical.join("rev-1.md").is_file());
        assert!(canonical.join("rev-2.md").is_file());
        fixture.remove();
    }

    #[test]
    fn sync_finalizes_persisted_adoption_cleanup() {
        let fixture = test_fixture("recover-finalize");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        insert_branch_target(&fixture, 2);
        let metadata = pr_metadata(&fixture, 1153, "head");
        let mut state = read_state(&fixture.home).unwrap();
        apply_pr_metadata(
            &fixture.home,
            &fixture.slug,
            state.repos.get_mut(&fixture.slug).unwrap(),
            &metadata,
            None,
            None,
        )
        .unwrap();
        write_state(&fixture.home, &state).unwrap();
        let backup = fixture
            .home
            .join(&fixture.slug)
            .join(".adopt-pr-1153-branch-backup");
        assert!(backup.is_dir());

        let output = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata.clone()])
        })
        .unwrap();

        assert_eq!(
            output.recovered,
            vec![RecoveryOutput {
                pull_request: 1153,
                action: "finalized persisted adoption".to_owned(),
            }]
        );
        assert!(!backup.exists());
        assert_eq!(output.unchanged, vec![1153]);
        fixture.remove();
    }

    #[test]
    fn sync_refuses_ambiguous_interrupted_adoption() {
        let fixture = test_fixture("recover-ambiguous");
        insert_branch_target(&fixture, 1);
        let pr_backup = fixture
            .home
            .join(&fixture.slug)
            .join(".adopt-pr-1153-pr-backup");
        fs::create_dir(&pr_backup).unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let error = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap_err();

        assert!(error.to_string().contains("ambiguous"));
        assert!(pr_backup.is_dir());
        fixture.remove();
    }

    #[test]
    fn sync_preserves_stage_when_a_merge_source_is_missing() {
        let fixture = test_fixture("recover-stage-missing-source");
        insert_branch_target(&fixture, 1);
        let stage = fixture
            .home
            .join(&fixture.slug)
            .join(".adopt-pr-1153-stage-999");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("only-pr-copy"), "evidence\n").unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let error = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap_err();

        assert!(error.to_string().contains("not both authoritative"));
        assert!(stage.join("only-pr-copy").is_file());
        fixture.remove();
    }

    #[test]
    fn sync_preserves_mid_merge_artifacts_when_pr_source_is_missing() {
        let fixture = test_fixture("recover-mid-merge-missing-pr");
        insert_branch_target(&fixture, 1);
        let repo_home = fixture.home.join(&fixture.slug);
        let stage = repo_home.join(".adopt-pr-1153-stage-999");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("only-pr-copy"), "evidence\n").unwrap();
        let branch_backup = repo_home.join(".adopt-pr-1153-branch-backup");
        fs::rename(&fixture.target_path, &branch_backup).unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let error = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap_err();

        assert!(error.to_string().contains("canonical PR source"));
        assert!(stage.join("only-pr-copy").is_file());
        assert!(branch_backup.is_dir());
        assert!(!fixture.target_path.exists());
        fixture.remove();
    }

    #[test]
    fn sync_refuses_unrecognized_recovery_artifact() {
        let fixture = test_fixture("recover-unrecognized");
        insert_branch_target(&fixture, 1);
        let artifact = fixture
            .home
            .join(&fixture.slug)
            .join(".adopt-pr-1153-mystery");
        fs::create_dir(&artifact).unwrap();

        let error =
            sync_with_fetch(&fixture.home, &fixture.slug, |_| Ok(Vec::new()))
                .unwrap_err();

        assert!(error.to_string().contains("unrecognized"));
        assert!(artifact.is_dir());
        fixture.remove();
    }

    #[test]
    fn sync_refuses_non_directory_recovery_artifact() {
        let fixture = test_fixture("recover-file");
        insert_branch_target(&fixture, 1);
        let artifact = fixture
            .home
            .join(&fixture.slug)
            .join(".adopt-pr-1153-branch-backup");
        fs::write(&artifact, "not a directory\n").unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let error = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap_err();

        assert!(error.to_string().contains("not a directory"));
        assert!(artifact.is_file());
        fixture.remove();
    }

    #[test]
    fn sync_refuses_malformed_stage_name() {
        let fixture = test_fixture("recover-stage-name");
        insert_branch_target(&fixture, 1);
        let artifact = fixture
            .home
            .join(&fixture.slug)
            .join(".adopt-pr-1153-stage-not-a-pid");
        fs::create_dir(&artifact).unwrap();

        let error =
            sync_with_fetch(&fixture.home, &fixture.slug, |_| Ok(Vec::new()))
                .unwrap_err();

        assert!(error.to_string().contains("unrecognized"));
        assert!(artifact.is_dir());
        fixture.remove();
    }

    #[test]
    fn targeted_pr_creates_missing_branch_owned_target() {
        let fixture = test_fixture("targeted-new-pr");
        let metadata = pr_metadata(&fixture, 1153, "github-head");
        let mut state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get_mut(&fixture.slug).unwrap();
        assert!(repo.branches.is_empty());

        let (branch, head, outcome) = apply_targeted_pr_metadata(
            &fixture.home,
            &fixture.slug,
            repo,
            &metadata,
        )
        .unwrap();

        assert_eq!(branch, fixture.branch);
        assert_eq!(head, "github-head");
        assert_eq!(outcome.kind, AdoptionKind::Adopted);
        let target = repo.branches.get(&fixture.branch).unwrap();
        assert_eq!(target.directory, "pr-1153");
        assert_eq!(
            alias_branch(repo.pull_requests.get(&1153).unwrap(), 1153).unwrap(),
            fixture.branch
        );
        assert!(
            fixture
                .home
                .join(&fixture.slug)
                .join(".adopt-pr-1153-branch-backup")
                .is_dir()
        );
        fixture.remove();
    }

    #[test]
    fn ambiguous_sync_persists_repairs_and_reports_skip() {
        let fixture = test_fixture("ambiguous-repair");
        write_review(&fixture.target_path, 5);
        insert_branch_target(&fixture, 2);
        let first = pr_metadata(&fixture, 10, "head-a");
        let second = pr_metadata(&fixture, 11, "head-b");

        let output = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![first, second])
        })
        .unwrap();

        assert_eq!(branch_target(&fixture).next_review_number, 6);
        assert_eq!(output.local_repaired.len(), 1);
        assert_eq!(
            output.skipped_ambiguous.get(&fixture.branch),
            Some(&vec![10, 11])
        );
        assert!(output.adopted.is_empty());
        fixture.remove();
    }

    #[test]
    fn sync_migrates_legacy_pr_target_and_merges_its_history() {
        let fixture = test_fixture("legacy-pr-target");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        insert_branch_target(&fixture, 2);
        let pr_path = fixture.home.join(&fixture.slug).join("pr-1153");
        fs::create_dir_all(&pr_path).unwrap();
        write_review_yaml(&pr_path, 1, "pr:1153");
        let mut state = read_state(&fixture.home).unwrap();
        state
            .repos
            .get_mut(&fixture.slug)
            .unwrap()
            .pull_requests
            .insert(
                1153,
                PrAlias::Legacy(Box::new(new_target(
                    "pr-1153".to_owned(),
                    "main",
                    "base",
                    "legacy-head",
                ))),
            );
        write_state(&fixture.home, &state).unwrap();

        let metadata = pr_metadata(&fixture, 1153, "github-head");
        let output = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap();

        assert_eq!(output.merged.get(&1153).unwrap().get(&1), Some(&2));
        let state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get(&fixture.slug).unwrap();
        assert_eq!(
            alias_branch(repo.pull_requests.get(&1153).unwrap(), 1153).unwrap(),
            fixture.branch
        );
        assert!(pr_path.join("rev-1.md").is_file());
        assert!(pr_path.join("rev-2.md").is_file());
        fixture.remove();
    }

    #[test]
    fn sync_reports_known_branch_with_zero_matching_prs() {
        let fixture = test_fixture("zero-match");
        insert_branch_target(&fixture, 1);

        let output =
            sync_with_fetch(&fixture.home, &fixture.slug, |_| Ok(Vec::new()))
                .unwrap();

        assert_eq!(output.skipped_zero, vec![fixture.branch.clone()]);
        assert!(output.adopted.is_empty());
        fixture.remove();
    }

    #[test]
    fn sync_does_not_adopt_deleted_local_branch() {
        let fixture = test_fixture("deleted-branch");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        insert_branch_target(&fixture, 2);
        run_git(&fixture.worktree, ["branch", "-D", &fixture.branch]);
        let metadata = pr_metadata(&fixture, 1153, "github-head");

        let result = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        });

        assert!(result.is_err());
        assert!(fixture.target_path.join("rev-1.md").is_file());
        assert!(!fixture.home.join(&fixture.slug).join("pr-1153").exists());
        fixture.remove();
    }

    #[test]
    fn sync_preserves_and_reports_unknown_backup() {
        let fixture = test_fixture("scoped-cleanup");
        insert_branch_target(&fixture, 1);
        let unrelated = fixture
            .home
            .join(&fixture.slug)
            .join(".adopt-pr-999-branch-backup");
        fs::create_dir_all(&unrelated).unwrap();
        let metadata = pr_metadata(&fixture, 1153, "head");

        let error = sync_with_fetch(&fixture.home, &fixture.slug, |_| {
            Ok(vec![metadata])
        })
        .unwrap_err();

        assert!(error.to_string().contains("artifacts were preserved"));
        assert!(unrelated.is_dir());
        assert!(!fixture.home.join(&fixture.slug).join("pr-1153").exists());
        fixture.remove();
    }

    #[test]
    fn pending_snapshot_rejects_authority_change_with_same_shas() {
        let fixture = test_fixture("authority-change");
        let sha = exact_branch_sha(&fixture.worktree, &fixture.branch).unwrap();
        let base = exact_branch_sha(&fixture.worktree, "main").unwrap();
        let mut target = new_target("unused".to_owned(), "main", &base, &sha);
        target.pending = Some(ReviewSnapshot {
            head_sha: sha.clone(),
            base_sha: base,
            authority: "branch".to_owned(),
        });

        let error = validate_pending_snapshot(&target, "pr", &sha).unwrap_err();

        assert!(error.to_string().contains("reopen to re-resolve"));
        fixture.remove();
    }

    #[test]
    fn combined_context_preserves_markdown_significant_whitespace() {
        let branch = "    indented code\nline with break  \n";
        let pr = "\n    PR code\n";

        let combined = combined_context(branch, pr);

        assert!(combined.contains(branch));
        assert!(combined.contains(pr));
    }

    #[test]
    fn target_output_includes_authority_and_head_diagnostics() {
        let fixture = test_fixture("output-diagnostics");
        insert_branch_target(&fixture, 1);
        let metadata = pr_metadata(&fixture, 1153, "github-head");
        let mut state = read_state(&fixture.home).unwrap();
        let repo = state.repos.get_mut(&fixture.slug).unwrap();
        apply_pr_metadata(
            &fixture.home,
            &fixture.slug,
            repo,
            &metadata,
            None,
            None,
        )
        .unwrap();
        let target = repo.branches.get(&fixture.branch).unwrap();

        let output = target_output(
            &fixture.home,
            &fixture.slug,
            None,
            &fixture.worktree,
            "pr:1153",
            &fixture.branch,
            target,
            "github-head",
        )
        .unwrap();

        assert_eq!(output.head_authority, "pr");
        assert!(output.local_branch_sha.is_some());
        assert_eq!(output.pr_head_sha.as_deref(), Some("github-head"));
        assert!(output.divergence);
        assert!(output.origin_branch_sha.is_none());
        fixture.remove();
    }

    #[test]
    fn local_head_policy_is_rejected_for_pr_selector() {
        assert!(
            validate_head_policy(None, Some(1153), Some(HeadPolicy::Local))
                .is_err()
        );
    }

    #[test]
    fn status_counts_branch_and_pr_alias_once() {
        let fixture = test_fixture("status-dedupe");
        write_review_yaml(&fixture.target_path, 1, "branch:evidence");
        fs::write(
            fixture.target_path.join("rev-2.md"),
            "- [ ] rev: P1 — shared finding\n",
        )
        .unwrap();
        insert_branch_target(&fixture, 3);
        let metadata = pr_metadata(&fixture, 1153, "head");
        let mut state = read_state(&fixture.home).unwrap();
        apply_pr_metadata(
            &fixture.home,
            &fixture.slug,
            state.repos.get_mut(&fixture.slug).unwrap(),
            &metadata,
            None,
            None,
        )
        .unwrap();
        state.repos.insert(
            "aaa__without-pr".to_owned(),
            Repo {
                path: fixture.worktree.clone(),
                remote: "github.com/example/without-pr".to_owned(),
                branches: BTreeMap::new(),
                pull_requests: BTreeMap::new(),
            },
        );
        let all = collect_status_targets(
            &fixture.home,
            &state,
            Some(&fixture.slug),
            None,
            None,
            None,
        )
        .unwrap();
        let by_branch = collect_status_targets(
            &fixture.home,
            &state,
            Some(&fixture.slug),
            Some(fixture.branch.clone()),
            None,
            None,
        )
        .unwrap();
        let by_pr = collect_status_targets(
            &fixture.home,
            &state,
            Some(&fixture.slug),
            None,
            Some(1153),
            None,
        )
        .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].todo, 1);
        assert_eq!(by_branch[0].target_path, by_pr[0].target_path);
        let global_by_pr = collect_status_targets(
            &fixture.home,
            &state,
            None,
            None,
            Some(1153),
            None,
        )
        .unwrap();
        assert_eq!(global_by_pr.len(), 1);
        fixture.remove();
    }

    struct TestFixture {
        home: PathBuf,
        worktree: PathBuf,
        slug: String,
        branch: String,
        target_path: PathBuf,
    }

    impl TestFixture {
        fn remove(self) {
            fs::remove_dir_all(self.home).unwrap();
            fs::remove_dir_all(self.worktree).unwrap();
        }
    }

    fn test_fixture(label: &str) -> TestFixture {
        let home = temp_home(label);
        let worktree = temp_home(&format!("{label}-worktree"));
        let slug = "example__reviews".to_owned();
        let branch = "feature/reconcile".to_owned();
        fs::create_dir_all(&worktree).unwrap();
        run_git(&worktree, ["init", "--initial-branch=main"]);
        fs::write(worktree.join("README.md"), "test\n").unwrap();
        run_git(&worktree, ["add", "README.md"]);
        run_git(
            &worktree,
            [
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "initial",
            ],
        );
        run_git(&worktree, ["branch", &branch]);

        let target_path = home
            .join(&slug)
            .join(format!("br-v2-{}", encode_component(&branch)));
        fs::create_dir_all(&target_path).unwrap();
        let mut state = State::default();
        state.repos.insert(
            slug.clone(),
            Repo {
                path: worktree.clone(),
                remote: "github.com/example/reviews".to_owned(),
                branches: BTreeMap::new(),
                pull_requests: BTreeMap::new(),
            },
        );
        fs::create_dir_all(&home).unwrap();
        write_state(&home, &state).unwrap();
        TestFixture {
            home,
            worktree,
            slug,
            branch,
            target_path,
        }
    }

    fn unregistered_fixture(label: &str) -> TestFixture {
        let home = temp_home(label);
        let worktree = temp_home(&format!("{label}-worktree"));
        let slug = "example__reviews".to_owned();
        let branch = "feature/reconcile".to_owned();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        run_git(&worktree, ["init", "--initial-branch=main"]);
        fs::write(worktree.join("README.md"), "test\n").unwrap();
        run_git(&worktree, ["add", "README.md"]);
        run_git(
            &worktree,
            [
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "initial",
            ],
        );
        run_git(&worktree, ["branch", &branch]);
        let target_path = home
            .join(&slug)
            .join(format!("br-v2-{}", encode_component(&branch)));
        TestFixture {
            home,
            worktree,
            slug,
            branch,
            target_path,
        }
    }

    fn write_review(target_path: &Path, number: u64) {
        fs::write(target_path.join(format!("rev-{number}.md")), "existing\n")
            .unwrap();
    }

    fn write_review_yaml(target_path: &Path, number: u64, target: &str) {
        fs::write(
            target_path.join(format!("rev-{number}.md")),
            format!(
                "---\nreview: {number}\ntarget: {target}\n\
                 base_sha: base\nhead_sha: head\n---\n"
            ),
        )
        .unwrap();
    }

    fn pr_metadata(
        fixture: &TestFixture,
        number: u64,
        head_sha: &str,
    ) -> PrMetadata {
        PrMetadata {
            number,
            state: "OPEN".to_owned(),
            head_ref_name: fixture.branch.clone(),
            head_ref_oid: head_sha.to_owned(),
            head_repository_owner: RepositoryOwner {
                login: "example".to_owned(),
            },
            head_repository: RepositoryName {
                name: "reviews".to_owned(),
            },
            base_ref_name: "main".to_owned(),
            base_ref_oid: git(&fixture.worktree, ["rev-parse", "main"]).unwrap(),
        }
    }

    fn insert_branch_target(fixture: &TestFixture, next_review_number: u64) {
        let mut state = read_state(&fixture.home).unwrap();
        let mut target = new_target(
            fixture
                .target_path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            "main",
            &git(&fixture.worktree, ["rev-parse", "main"]).unwrap(),
            &git(&fixture.worktree, ["rev-parse", &fixture.branch]).unwrap(),
        );
        target.next_review_number = next_review_number;
        state
            .repos
            .get_mut(&fixture.slug)
            .unwrap()
            .branches
            .insert(fixture.branch.clone(), target);
        write_state(&fixture.home, &state).unwrap();
    }

    fn branch_target(fixture: &TestFixture) -> Target {
        read_state(&fixture.home)
            .unwrap()
            .repos
            .remove(&fixture.slug)
            .unwrap()
            .branches
            .remove(&fixture.branch)
            .unwrap()
    }

    fn run_git<const N: usize>(worktree: &Path, arguments: [&str; N]) {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(worktree)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit_test_change(worktree: &Path, message: &str) {
        run_git(
            worktree,
            [
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                message,
            ],
        );
    }

    fn temp_home(label: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "gh-rev-test-{label}-{}-{}",
            std::process::id(),
            TEMP_HOME_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
