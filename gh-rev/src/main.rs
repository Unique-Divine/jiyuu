use std::{
    collections::BTreeMap,
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
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

    #[command(subcommand)]
    command: Option<CommandName>,
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
        repo: String,
        #[arg(long, conflicts_with = "pr")]
        branch: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long, default_value = "main")]
        base: String,
    },
    /// Allocate the next review template for a branch or pull-request target.
    Next {
        repo: String,
        #[arg(long, conflicts_with = "pr")]
        branch: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long)]
        label: Option<String>,
    },
    /// Print or create the human-authored context file for a target.
    Context {
        repo: String,
        #[arg(long, conflicts_with = "pr")]
        branch: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
    },
    /// Summarize review findings; use `status todo` to list unresolved findings.
    Status {
        filter: Option<String>,
        repo: Option<String>,
        #[arg(long, conflicts_with = "pr")]
        branch: Option<String>,
        #[arg(long)]
        pr: Option<u64>,
    },
    /// Display the persistent workspace root and state-file path.
    Paths,
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

#[derive(Debug, Serialize, Deserialize)]
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
    pull_requests: BTreeMap<u64, Target>,
}

#[derive(Debug, Serialize, Deserialize)]
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
    repo_path: PathBuf,
    target: String,
    target_path: PathBuf,
    context_path: PathBuf,
    base_ref: String,
    base_sha: String,
    head_sha: String,
    reviews: Vec<PathBuf>,
}

#[derive(Serialize)]
/// JSON response from allocation, extending target context with the new file.
struct NextOutput {
    #[serde(flatten)]
    target: TargetOutput,
    review_path: PathBuf,
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
    target: String,
    target_path: PathBuf,
    total: usize,
    resolved: usize,
    todo: usize,
    todo_findings: Vec<FindingOutput>,
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
    let command = cli.command.with_context(|| "missing command")?;
    match command {
        CommandName::Register { path } => register(&home, &path),
        CommandName::Open {
            repo,
            branch,
            pr,
            base,
        } => open(&home, &repo, branch, pr, &base),
        CommandName::Next {
            repo,
            branch,
            pr,
            label,
        } => next(&home, &repo, branch, pr, label),
        CommandName::Context { repo, branch, pr } => {
            context(&home, &repo, branch, pr)
        }
        CommandName::Status {
            filter,
            repo,
            branch,
            pr,
        } => status(&home, filter, repo, branch, pr),
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

fn open(
    home: &Path,
    slug: &str,
    branch: Option<String>,
    pr: Option<u64>,
    base: &str,
) -> Result<()> {
    let _lock = lock(home)?;
    let mut state = read_state(home)?;
    let repo = state.repos.get_mut(slug).with_context(|| {
        format!("unknown repository {slug}; run register first")
    })?;
    let repo_path = repo.path.clone();
    let (target_name, head_sha, target) =
        resolve_or_create_target(home, slug, repo, branch, pr, base)?;
    report_reconciled_target(home, slug, &target_name, target)?;
    let output =
        target_output(home, slug, &repo_path, &target_name, target, &head_sha)?;
    write_state(home, &state)?;
    print_json(&output)
}

fn next(
    home: &Path,
    slug: &str,
    branch: Option<String>,
    pr: Option<u64>,
    label: Option<String>,
) -> Result<()> {
    let _lock = lock(home)?;
    let mut state = read_state(home)?;
    let repo = state.repos.get_mut(slug).with_context(|| {
        format!("unknown repository {slug}; run register first")
    })?;
    let repo_path = repo.path.clone();
    let (target_name, head_sha, target) =
        resolve_or_create_target(home, slug, repo, branch, pr, "main")?;
    report_reconciled_target(home, slug, &target_name, target)?;
    let number = target.next_review_number;
    target.next_review_number = target
        .next_review_number
        .checked_add(1)
        .context("review number overflow; operator repair is required")?;
    let target_path = home.join(slug).join(&target.directory);
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
    let output = NextOutput {
        target: target_output(
            home,
            slug,
            &repo_path,
            &target_name,
            target,
            &head_sha,
        )?,
        review_path,
    };
    write_state(home, &state)?;
    print_json(&output)
}

fn context(
    home: &Path,
    slug: &str,
    branch: Option<String>,
    pr: Option<u64>,
) -> Result<()> {
    let _lock = lock(home)?;
    let mut state = read_state(home)?;
    let repo = state.repos.get_mut(slug).with_context(|| {
        format!("unknown repository {slug}; run register first")
    })?;
    let repo_path = repo.path.clone();
    let (target_name, head_sha, target) =
        resolve_or_create_target(home, slug, repo, branch, pr, "main")?;
    report_reconciled_target(home, slug, &target_name, target)?;
    let output =
        target_output(home, slug, &repo_path, &target_name, target, &head_sha)?;
    write_state(home, &state)?;
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
    branch: Option<String>,
    pr: Option<u64>,
) -> Result<()> {
    let (todo_only, repo_filter) = parse_status_positionals(first, second)?;
    if branch.is_some() && pr.is_some() {
        bail!("use either --branch or --pr");
    }
    let state = read_state(home)?;
    let mut targets = Vec::new();
    for (slug, repo) in &state.repos {
        if repo_filter.as_deref().is_some_and(|value| value != slug) {
            continue;
        }
        for (branch_name, target) in &repo.branches {
            if pr.is_none()
                && branch.as_deref().is_none_or(|value| value == branch_name)
            {
                targets.push(target_status(
                    home,
                    slug,
                    format!("branch:{branch_name}"),
                    target,
                )?);
            }
        }
        for (number, target) in &repo.pull_requests {
            if branch.is_none() && pr.is_none_or(|value| value == *number) {
                targets.push(target_status(
                    home,
                    slug,
                    format!("pr:{number}"),
                    target,
                )?);
            }
        }
    }
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

fn resolve_or_create_target<'a>(
    home: &Path,
    slug: &str,
    repo: &'a mut Repo,
    branch: Option<String>,
    pr: Option<u64>,
    base: &str,
) -> Result<(String, String, &'a mut Target)> {
    match (branch, pr) {
        (Some(branch), None) => {
            let head_sha = git(&repo.path, ["rev-parse", &branch])?;
            let base_sha = git(&repo.path, ["rev-parse", base])?;
            let directory = branch_directory(home, slug, repo, &branch)?;
            let target =
                repo.branches
                    .entry(branch.clone())
                    .or_insert_with(|| Target {
                        directory,
                        base_ref: base.to_owned(),
                        base_sha: base_sha.clone(),
                        last_reviewed_head_sha: head_sha.clone(),
                        next_review_number: 1,
                    });
            Ok((format!("branch:{branch}"), head_sha, target))
        }
        (None, Some(number)) => {
            let target = repo.pull_requests.get_mut(&number).with_context(|| format!("PR {number} is unknown; run sync or open the branch first"))?;
            Ok((
                format!("pr:{number}"),
                target.last_reviewed_head_sha.clone(),
                target,
            ))
        }
        _ => bail!("supply exactly one of --branch or --pr"),
    }
}

fn target_output(
    home: &Path,
    slug: &str,
    repo_path: &Path,
    target_name: &str,
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
    Ok(TargetOutput {
        repo: slug.to_owned(),
        repo_path: repo_path.to_owned(),
        target: target_name.to_owned(),
        target_path,
        context_path,
        base_ref: target.base_ref.clone(),
        base_sha: target.base_sha.clone(),
        head_sha: head_sha.to_owned(),
        reviews,
    })
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

    let canonical = format!("br-{}", encode_component(branch));
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
    file.lock_exclusive()
        .with_context(|| format!("failed to lock {}", lock_path.display()))?;
    Ok(file)
}

fn read_state(home: &Path) -> Result<State> {
    let path = home.join(STATE_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(State::default())
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to read {}", path.display())),
    }
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
            .join(format!("br-{}", encode_component(&branch)));
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

    fn write_review(target_path: &Path, number: u64) {
        fs::write(target_path.join(format!("rev-{number}.md")), "existing\n")
            .unwrap();
    }

    fn insert_branch_target(fixture: &TestFixture, next_review_number: u64) {
        let mut state = read_state(&fixture.home).unwrap();
        state.repos.get_mut(&fixture.slug).unwrap().branches.insert(
            fixture.branch.clone(),
            Target {
                directory: fixture
                    .target_path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                base_ref: "main".to_owned(),
                base_sha: git(&fixture.worktree, ["rev-parse", "main"]).unwrap(),
                last_reviewed_head_sha: git(
                    &fixture.worktree,
                    ["rev-parse", &fixture.branch],
                )
                .unwrap(),
                next_review_number,
            },
        );
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
