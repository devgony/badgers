use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Args;

/// Provision everything GitHub Actions needs to use GCS with badgers:
/// bucket, workload identity pool/provider, service account, IAM bindings.
/// Every step is idempotent; existing resources are left untouched.
#[derive(Args, Debug)]
pub struct SetupGcsArgs {
    /// GCP project ID (the only required input)
    #[arg(long)]
    pub project: String,

    /// GitHub repository slug "owner/name" (default: parsed from `origin`)
    #[arg(long)]
    pub repo: Option<String>,

    /// Bucket name (default: "{project}-badgers-coverage")
    #[arg(long)]
    pub bucket: Option<String>,

    /// Bucket location
    #[arg(long, default_value = "asia-northeast3")]
    pub location: String,

    /// Workload identity pool ID, shared across repositories
    #[arg(long, default_value = "github-actions")]
    pub pool: String,

    /// OIDC provider ID (default: "gh-{repo name}", one per repository)
    #[arg(long)]
    pub provider: Option<String>,

    /// Service account name (default: "badgers-{repo name}")
    #[arg(long)]
    pub service_account: Option<String>,

    /// Immutable GitHub repository ID (default: resolved via `gh api`)
    #[arg(long)]
    pub repository_id: Option<u64>,

    /// Print mutating commands instead of executing them
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(args: &SetupGcsArgs) -> Result<()> {
    let exec = Exec {
        dry_run: args.dry_run,
    };
    let plan = Plan::resolve(args, &exec)?;
    plan.print_header();

    exec.mutate(
        "enable required APIs",
        "gcloud",
        &[
            "services",
            "enable",
            "iam.googleapis.com",
            "iamcredentials.googleapis.com",
            "sts.googleapis.com",
            "cloudresourcemanager.googleapis.com",
            "storage.googleapis.com",
            "--project",
            &plan.project,
        ],
    )?;

    ensure_bucket(&exec, &plan)?;
    ensure_pool(&exec, &plan)?;
    ensure_provider(&exec, &plan)?;
    ensure_service_account(&exec, &plan)?;
    bind_iam(&exec, &plan)?;

    plan.print_workflow_snippet();
    Ok(())
}

struct Plan {
    project: String,
    project_number: String,
    repo: String,
    repository_id: String,
    bucket: String,
    location: String,
    pool: String,
    provider: String,
    sa_email: String,
    sa_name: String,
}

impl Plan {
    fn resolve(args: &SetupGcsArgs, exec: &Exec) -> Result<Self> {
        let repo = match &args.repo {
            Some(repo) => repo.clone(),
            None => detect_repo()?,
        };
        let repo_name = repo
            .split('/')
            .nth(1)
            .with_context(|| format!("repo must be owner/name, got {repo:?}"))?;

        let project_number = exec
            .read(
                "gcloud",
                &[
                    "projects",
                    "describe",
                    &args.project,
                    "--format=value(projectNumber)",
                ],
            )
            .context("resolving project number (is gcloud authenticated?)")?;

        let repository_id = match args.repository_id {
            Some(id) => id.to_string(),
            None => exec
                .read("gh", &["api", &format!("repos/{repo}"), "--jq", ".id"])
                .context("resolving repository id (install gh or pass --repository-id)")?,
        };

        let sa_name = args
            .service_account
            .clone()
            .unwrap_or_else(|| sanitize(&format!("badgers-{repo_name}"), 30));
        Ok(Self {
            project_number,
            repository_id,
            bucket: args
                .bucket
                .clone()
                .unwrap_or_else(|| format!("{}-badgers-coverage", args.project)),
            location: args.location.clone(),
            pool: args.pool.clone(),
            provider: args
                .provider
                .clone()
                .unwrap_or_else(|| sanitize(&format!("gh-{repo_name}"), 32)),
            sa_email: format!("{sa_name}@{}.iam.gserviceaccount.com", args.project),
            sa_name,
            repo,
            project: args.project.clone(),
        })
    }

    fn principal_set(&self) -> String {
        format!(
            "principalSet://iam.googleapis.com/projects/{}/locations/global/workloadIdentityPools/{}/attribute.repository_id/{}",
            self.project_number, self.pool, self.repository_id
        )
    }

    fn provider_resource(&self) -> String {
        format!(
            "projects/{}/locations/global/workloadIdentityPools/{}/providers/{}",
            self.project_number, self.pool, self.provider
        )
    }

    fn attribute_condition(&self) -> String {
        format!("assertion.repository_id == '{}'", self.repository_id)
    }

    fn print_header(&self) {
        println!("project:        {} ({})", self.project, self.project_number);
        println!("repository:     {} (id {})", self.repo, self.repository_id);
        println!("bucket:         gs://{} ({})", self.bucket, self.location);
        println!("pool/provider:  {}/{}", self.pool, self.provider);
        println!("service acct:   {}", self.sa_email);
        println!();
    }

    fn print_workflow_snippet(&self) {
        println!();
        println!("Setup complete. Add this to your workflow before the badgers step:");
        println!();
        println!("    permissions:");
        println!("      contents: read");
        println!("      id-token: write");
        println!("      pull-requests: write");
        println!();
        println!("      - uses: google-github-actions/auth@v3");
        println!("        with:");
        println!("          project_id: {}", self.project);
        println!(
            "          workload_identity_provider: {}",
            self.provider_resource()
        );
        println!("          service_account: {}", self.sa_email);
        println!();
        println!("      - uses: devgony/badgers@main");
        println!("        with:");
        println!("          gcs-bucket: {}", self.bucket);
    }
}

fn ensure_bucket(exec: &Exec, plan: &Plan) -> Result<()> {
    let bucket_url = format!("gs://{}", plan.bucket);
    if exec.exists(
        "gcloud",
        &[
            "storage",
            "buckets",
            "describe",
            &bucket_url,
            "--format=value(name)",
        ],
    ) {
        println!("bucket already exists: {bucket_url}");
        return Ok(());
    }
    exec.mutate(
        "create private bucket",
        "gcloud",
        &[
            "storage",
            "buckets",
            "create",
            &bucket_url,
            "--project",
            &plan.project,
            "--location",
            &plan.location,
            "--uniform-bucket-level-access",
            "--public-access-prevention",
        ],
    )
}

fn ensure_pool(exec: &Exec, plan: &Plan) -> Result<()> {
    if exec.exists(
        "gcloud",
        &[
            "iam",
            "workload-identity-pools",
            "describe",
            &plan.pool,
            "--project",
            &plan.project,
            "--location=global",
            "--format=value(name)",
        ],
    ) {
        println!("workload identity pool already exists: {}", plan.pool);
        return Ok(());
    }
    exec.mutate(
        "create workload identity pool",
        "gcloud",
        &[
            "iam",
            "workload-identity-pools",
            "create",
            &plan.pool,
            "--project",
            &plan.project,
            "--location=global",
            "--display-name=GitHub Actions",
        ],
    )
}

fn ensure_provider(exec: &Exec, plan: &Plan) -> Result<()> {
    if let Ok(existing) = exec.read(
        "gcloud",
        &[
            "iam",
            "workload-identity-pools",
            "providers",
            "describe",
            &plan.provider,
            "--project",
            &plan.project,
            "--location=global",
            &format!("--workload-identity-pool={}", plan.pool),
            "--format=value(attributeCondition)",
        ],
    ) {
        if existing.trim() == plan.attribute_condition() {
            println!("oidc provider already exists: {}", plan.provider);
        } else {
            println!(
                "::warning::provider {} exists with a different attribute condition\n  existing: {}\n  expected: {}\n  (fix manually or pass --provider to create a new one)",
                plan.provider,
                existing.trim(),
                plan.attribute_condition()
            );
        }
        return Ok(());
    }
    exec.mutate(
        "create github oidc provider",
        "gcloud",
        &[
            "iam",
            "workload-identity-pools",
            "providers",
            "create-oidc",
            &plan.provider,
            "--project",
            &plan.project,
            "--location=global",
            &format!("--workload-identity-pool={}", plan.pool),
            &format!("--display-name=GitHub {}", truncate(&plan.repo, 22)),
            "--issuer-uri=https://token.actions.githubusercontent.com",
            "--attribute-mapping=google.subject=assertion.sub,attribute.repository_id=assertion.repository_id,attribute.repository=assertion.repository,attribute.ref=assertion.ref",
            &format!("--attribute-condition={}", plan.attribute_condition()),
        ],
    )
}

fn ensure_service_account(exec: &Exec, plan: &Plan) -> Result<()> {
    if exec.exists(
        "gcloud",
        &[
            "iam",
            "service-accounts",
            "describe",
            &plan.sa_email,
            "--project",
            &plan.project,
            "--format=value(email)",
        ],
    ) {
        println!("service account already exists: {}", plan.sa_email);
        return Ok(());
    }
    exec.mutate(
        "create service account",
        "gcloud",
        &[
            "iam",
            "service-accounts",
            "create",
            &plan.sa_name,
            "--project",
            &plan.project,
            &format!(
                "--display-name=Badgers coverage ({})",
                truncate(&plan.repo, 60)
            ),
        ],
    )
}

fn bind_iam(exec: &Exec, plan: &Plan) -> Result<()> {
    exec.mutate(
        "allow the github repo to impersonate the service account",
        "gcloud",
        &[
            "iam",
            "service-accounts",
            "add-iam-policy-binding",
            &plan.sa_email,
            "--project",
            &plan.project,
            "--role=roles/iam.workloadIdentityUser",
            &format!("--member={}", plan.principal_set()),
        ],
    )?;
    exec.mutate(
        "grant object access on the bucket",
        "gcloud",
        &[
            "storage",
            "buckets",
            "add-iam-policy-binding",
            &format!("gs://{}", plan.bucket),
            &format!("--member=serviceAccount:{}", plan.sa_email),
            "--role=roles/storage.objectUser",
        ],
    )
}

struct Exec {
    dry_run: bool,
}

impl Exec {
    fn read(&self, program: &str, args: &[&str]) -> Result<String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("running {program} (is it installed?)"))?;
        if !output.status.success() {
            bail!(
                "{program} {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn exists(&self, program: &str, args: &[&str]) -> bool {
        self.read(program, args).is_ok()
    }

    fn mutate(&self, label: &str, program: &str, args: &[&str]) -> Result<()> {
        if self.dry_run {
            println!("[dry-run] {label}:\n  {program} {}", args.join(" "));
            return Ok(());
        }
        println!("{label}...");
        self.read(program, args).map(|out| {
            if !out.is_empty() {
                println!("{out}");
            }
        })
    }
}

fn detect_repo() -> Result<String> {
    let url = Exec { dry_run: false }
        .read("git", &["config", "--get", "remote.origin.url"])
        .context("detecting repo from git remote (or pass --repo owner/name)")?;
    parse_repo_url(&url).with_context(|| format!("cannot parse owner/name from remote {url:?}"))
}

fn parse_repo_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(".git");
    let tail = trimmed
        .rsplit_once(':')
        .map(|(_, t)| t)
        .unwrap_or(trimmed)
        .trim_start_matches('/');
    let mut segments = tail.rsplit('/');
    let name = segments.next()?;
    let owner = segments.next()?;
    if name.is_empty() || owner.is_empty() || owner.contains('@') {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

fn sanitize(name: &str, max_len: usize) -> String {
    let cleaned: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    cleaned
        .trim_matches('-')
        .chars()
        .take(max_len)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

fn truncate(s: &str, max_len: usize) -> &str {
    &s[..s.len().min(max_len)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_remote_urls() {
        for url in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo",
            "ssh://git@github.com/owner/repo.git",
        ] {
            assert_eq!(parse_repo_url(url).as_deref(), Some("owner/repo"), "{url}");
        }
        assert_eq!(parse_repo_url("not-a-url"), None);
    }

    #[test]
    fn sanitizes_resource_names() {
        assert_eq!(sanitize("badgers-My_Repo.py", 30), "badgers-my-repo-py");
        assert_eq!(
            sanitize("badgers-timetree-planner-agent", 20),
            "badgers-timetree-pla"
        );
        assert_eq!(sanitize("---x---", 30), "x");
    }
}
