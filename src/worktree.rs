use crate::{Error, failure};
use std::path::PathBuf;

pub(crate) fn resolve(worktree: PathBuf) -> Result<PathBuf, Error> {
    if !worktree.is_absolute() || !worktree.is_dir() {
        return Err(failure("worktree must be an existing absolute directory"));
    }
    let worktree = worktree
        .canonicalize()
        .map_err(|_| failure("worktree is unavailable"))?;
    for directory in worktree.ancestors() {
        match std::fs::metadata(directory.join(".git")) {
            Ok(metadata) if metadata.is_dir() || metadata.is_file() => {
                return Ok(directory.to_path_buf());
            }
            Ok(_) => return Err(failure("invalid Git worktree marker")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(failure("Git worktree marker is unavailable")),
        }
    }
    Err(failure("cwd is not inside a Git worktree"))
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use std::process::Command;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn git(directory: &std::path::Path, args: &[&str]) -> TestResult {
        let status = Command::new("git")
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            )
            .env("HOME", directory)
            .current_dir(directory)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()?;
        assert!(status.success());
        Ok(())
    }

    #[test]
    fn nested_and_symlinked_cwd_resolve_to_git_root() -> TestResult {
        let temporary = tempfile::tempdir()?;
        git(temporary.path(), &["init", "--quiet"])?;
        let nested = temporary.path().join("a/b");
        std::fs::create_dir_all(&nested)?;
        let alias = temporary.path().join("alias");
        std::os::unix::fs::symlink(&nested, &alias)?;
        assert_eq!(resolve(alias)?, temporary.path().canonicalize()?);
        Ok(())
    }

    #[test]
    fn linked_worktree_and_submodule_use_their_own_roots() -> TestResult {
        let temporary = tempfile::tempdir()?;
        let repository = temporary.path().join("repository");
        std::fs::create_dir(&repository)?;
        git(&repository, &["init", "--quiet"])?;
        git(
            &repository,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        )?;
        git(
            &repository,
            &["worktree", "add", "--quiet", "--detach", "../linked"],
        )?;
        git(
            &repository,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                "--",
                "../linked",
                "child",
            ],
        )?;
        for root in [temporary.path().join("linked"), repository.join("child")] {
            assert!(root.join(".git").is_file());
            let nested = root.join("nested");
            std::fs::create_dir(&nested)?;
            assert_eq!(resolve(nested)?, root.canonicalize()?);
        }
        Ok(())
    }

    #[test]
    fn missing_relative_and_non_repository_paths_fail_explicitly() -> TestResult {
        let temporary = tempfile::tempdir()?;
        for path in [
            std::path::PathBuf::from("relative"),
            temporary.path().join("missing"),
        ] {
            assert_eq!(
                resolve(path)
                    .err()
                    .ok_or("invalid path accepted")?
                    .to_string(),
                "session-end: worktree must be an existing absolute directory"
            );
        }
        assert_eq!(
            resolve(temporary.path().to_path_buf())
                .err()
                .ok_or("non-repository accepted")?
                .to_string(),
            "session-end: cwd is not inside a Git worktree"
        );
        Ok(())
    }

    #[test]
    fn invalid_and_unreadable_git_markers_fail_explicitly() -> TestResult {
        let temporary = tempfile::tempdir()?;
        let marker = temporary.path().join(".git");
        let socket = std::os::unix::net::UnixListener::bind(&marker)?;
        assert_eq!(
            resolve(temporary.path().to_path_buf())
                .err()
                .ok_or("socket accepted")?
                .to_string(),
            "session-end: invalid Git worktree marker"
        );
        drop(socket);
        std::fs::remove_file(&marker)?;
        std::os::unix::fs::symlink(".git", &marker)?;
        assert_eq!(
            resolve(temporary.path().to_path_buf())
                .err()
                .ok_or("symlink loop accepted")?
                .to_string(),
            "session-end: Git worktree marker is unavailable"
        );
        Ok(())
    }

    #[test]
    fn git_fixture_commands_ignore_ambient_repository_routing() -> TestResult {
        let temporary = tempfile::tempdir()?;
        let sentinel = temporary.path().join("sentinel");
        std::fs::write(&sentinel, "unchanged")?;
        let output = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "worktree::tests::nested_and_symlinked_cwd_resolve_to_git_root",
            ])
            .env("GIT_DIR", &sentinel)
            .env("GIT_WORK_TREE", &sentinel)
            .env("GIT_COMMON_DIR", &sentinel)
            .output()?;
        assert_eq!(std::fs::read_to_string(&sentinel)?, "unchanged");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    }
}
