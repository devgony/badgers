use anyhow::Context;
use badge_rs_storage::{GcsBackend, Keys, LocalBackend, StorageBackend};
use clap::Args;

pub const TOKEN_ENV: &str = "BADGERS_GCS_TOKEN";

#[derive(Args)]
pub struct StorageOpts {
    /// GCS bucket name (requires an OAuth2 access token in $BADGERS_GCS_TOKEN)
    #[arg(long, conflicts_with = "local_dir")]
    pub bucket: Option<String>,

    /// Local directory backend (testing / local development)
    #[arg(long, required_unless_present = "bucket")]
    pub local_dir: Option<std::path::PathBuf>,

    /// Object key prefix inside the bucket
    #[arg(long, default_value = "badgers")]
    pub prefix: String,

    /// Repository slug, e.g. "owner/repo"
    #[arg(long)]
    pub repo: String,
}

impl StorageOpts {
    pub fn backend(&self) -> anyhow::Result<Box<dyn StorageBackend>> {
        if let Some(bucket) = &self.bucket {
            let token = std::env::var(TOKEN_ENV).with_context(|| {
                format!(
                    "{TOKEN_ENV} must hold a GCS access token \
                     (e.g. from `gcloud auth print-access-token`)"
                )
            })?;
            Ok(Box::new(GcsBackend::new(bucket.clone(), token)))
        } else {
            let dir = self
                .local_dir
                .as_ref()
                .expect("clap enforces bucket|local-dir");
            Ok(Box::new(LocalBackend::new(dir.clone())))
        }
    }

    pub fn keys(&self) -> Keys {
        Keys::new(&self.prefix, &self.repo)
    }
}
