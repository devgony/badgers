use badge_rs_storage::{Keys, LocalBackend};
use clap::Args;

#[derive(Args)]
pub struct StorageOpts {
    /// Storage directory, e.g. a checkout of the coverage storage branch
    #[arg(long)]
    pub local_dir: std::path::PathBuf,

    /// Object key prefix inside the storage directory
    #[arg(long, default_value = "badgers")]
    pub prefix: String,

    /// Repository slug, e.g. "owner/repo"
    #[arg(long)]
    pub repo: String,
}

impl StorageOpts {
    pub fn backend(&self) -> LocalBackend {
        LocalBackend::new(self.local_dir.clone())
    }

    pub fn keys(&self) -> Keys {
        Keys::new(&self.prefix, &self.repo)
    }
}
