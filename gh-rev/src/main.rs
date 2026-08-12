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

const STATE_FILE: &str = "state.toml";
const LOCK_FILE: &str = ".gh-rev.lock";

#[derive(Parser)]
#[command(about = "Persistent local review ledger")]
struct Cli {
    /// Override the default ~/gh workspace root.
    #[arg(long, global = true, env = "GH_REV_HOME")]
    home: Option<PathBuf>,

    #[command(subcommand)]
    command: CommandName,
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
struct State {
    #[serde(default)]
    repos: BTreeMap<String, Repo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Repo {
    path: PathBuf,
    remote: String,
    #[serde(default)]
    branches: BTreeMap<String, Target>,
    #[serde(default)]
    pull_requests: BTreeMap<u64, Target>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Target {
    directory: String,
    base_ref: String,
    base_sha: String,
    last_reviewed_head_sha: String,
    next_review_number: u64,
}

#[derive(Serialize)]
struct RegisterOutput<'a> {
    slug: &'a str,
    path: &'a Path,
    remote: &'a str,
    state_path: PathBuf,
}

#[derive(Serialize)]
struct PathsOutput {
    home: PathBuf,
    state_path: PathBuf,
}

#[derive(Serialize)]
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
struct NextOutput {
    #[serde(flatten)]
    target: TargetOutput,
    review_path: PathBuf,
}

#[derive(Serialize)]
struct FindingOutput {
    review_path: PathBuf,
    line: usize,
    text: String,
}

#[derive(Serialize)]
struct StatusOutput {
    targets: Vec<TargetStatus>,
    total: usize,
    resolved: usize,
    todo: usize,
}

#[derive(Serialize)]
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
    let home = review_home(cli.home)?;
    match cli.command {
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
    state.repos.insert(
        slug.clone(),
        Repo {
            path: path.clone(),
            remote: remote.clone(),
            branches: BTreeMap::new(),
            pull_requests: BTreeMap::new(),
        },
    );
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
    let (target_name, target) =
        resolve_or_create_target(repo, branch, pr, base)?;
    report_reconciled_target(home, slug, &target_name, target)?;
    let output = target_output(home, slug, &repo_path, &target_name, target)?;
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
    let (target_name, target) =
        resolve_or_create_target(repo, branch, pr, "main")?;
    report_reconciled_target(home, slug, &target_name, target)?;
    let number = target.next_review_number;
    target.next_review_number = target
        .next_review_number
        .checked_add(1)
        .context("review number overflow; operator repair is required")?;
    let target_path = home.join(slug).join(&target.directory);
    fs::create_dir_all(&target_path)?;
    let review_path = target_path.join(format!("rev-{number}.md"));
    if review_path.exists() {
        bail!("review already exists: {}", review_path.display());
    }
    fs::write(
        &review_path,
        review_template(&target_name, target, number, label.as_deref()),
    )
    .with_context(|| format!("failed to create {}", review_path.display()))?;
    let output = NextOutput {
        target: target_output(home, slug, &repo_path, &target_name, target)?,
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
    let (target_name, target) =
        resolve_or_create_target(repo, branch, pr, "main")?;
    report_reconciled_target(home, slug, &target_name, target)?;
    let output = target_output(home, slug, &repo_path, &target_name, target)?;
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
    filter: Option<String>,
    repo_filter: Option<String>,
    branch: Option<String>,
    pr: Option<u64>,
) -> Result<()> {
    if filter.as_deref().is_some_and(|value| value != "todo") {
        bail!("status filter must be `todo`");
    }
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
    if filter.as_deref() == Some("todo") {
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

fn resolve_or_create_target<'a>(
    repo: &'a mut Repo,
    branch: Option<String>,
    pr: Option<u64>,
    base: &str,
) -> Result<(String, &'a mut Target)> {
    match (branch, pr) {
        (Some(branch), None) => {
            let head_sha = git(&repo.path, ["rev-parse", &branch])?;
            let base_sha = git(&repo.path, ["rev-parse", base])?;
            let sanitized = sanitize_component(&branch);
            let target =
                repo.branches
                    .entry(branch.clone())
                    .or_insert_with(|| Target {
                        directory: format!("br-{sanitized}"),
                        base_ref: base.to_owned(),
                        base_sha: base_sha.clone(),
                        last_reviewed_head_sha: head_sha,
                        next_review_number: 1,
                    });
            Ok((format!("branch:{branch}"), target))
        }
        (None, Some(number)) => {
            let target = repo.pull_requests.get_mut(&number).with_context(|| format!("PR {number} is unknown; run sync or open the branch first"))?;
            Ok((format!("pr:{number}"), target))
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
    let head_sha = git(repo_path, ["rev-parse", "HEAD"])?;
    Ok(TargetOutput {
        repo: slug.to_owned(),
        repo_path: repo_path.to_owned(),
        target: target_name.to_owned(),
        target_path,
        context_path,
        base_ref: target.base_ref.clone(),
        base_sha: target.base_sha.clone(),
        head_sha,
        reviews,
    })
}

fn review_template(
    target: &str,
    target_state: &Target,
    number: u64,
    label: Option<&str>,
) -> String {
    let label = label
        .map(|value| format!("label: {value}\n"))
        .unwrap_or_default();
    format!(
        "---\nreview: {number}\n{label}target: {target}\nbase_sha: {}\nhead_sha: {}\n---\n\n# Review: {target}\n\n## Scope\n\n## Summary\n\n## Findings\n\n- [ ] rev: P1 — Describe an actionable finding.\n\n## Recommendation\n",
        target_state.base_sha, target_state.last_reviewed_head_sha
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
        if !entry.file_type()?.is_file() {
            continue;
        }
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
    fn unrelated_filenames_do_not_affect_review_allocation() {
        let fixture = test_fixture("unrelated-files");
        write_review(&fixture.target_path, 1);
        fs::write(fixture.target_path.join("rev-2.md.bak"), "").unwrap();
        fs::write(fixture.target_path.join("rev-x.md"), "").unwrap();
        fs::write(fixture.target_path.join("review-99.md"), "").unwrap();
        fs::write(fixture.target_path.join("rev-0.md"), "").unwrap();
        fs::create_dir(fixture.target_path.join("rev-99.md")).unwrap();

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
            .join(format!("br-{}", sanitize_component(&branch)));
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

    fn temp_home(label: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "gh-rev-test-{label}-{}-{}",
            std::process::id(),
            TEMP_HOME_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
