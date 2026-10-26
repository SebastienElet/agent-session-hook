use std::path::{Path, PathBuf};

pub struct Fixture {
    temporary: tempfile::TempDir,
    repository: PathBuf,
}

impl Fixture {
    pub fn new() -> std::io::Result<Self> {
        let temporary = tempfile::tempdir()?;
        let repository = temporary.path().join("repository");
        std::fs::create_dir(&repository)?;
        Ok(Self {
            temporary,
            repository,
        })
    }

    pub fn home(&self) -> &Path {
        self.temporary.path()
    }

    pub fn repository(&self) -> &Path {
        &self.repository
    }

    pub fn write_home(&self, path: &str, text: &str) -> std::io::Result<()> {
        let path = self.home().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)
    }
}
