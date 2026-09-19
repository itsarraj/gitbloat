use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use anyhow::{Context, Result};

/// Runs the real two-command pipeline this whole tool is built on: `git
/// rev-list --objects --all` lists every reachable object (deduplicated
/// by SHA across all of history, which is exactly the property that
/// makes `parse::aggregate_by_path` correct without extra work), piped
/// into `git cat-file --batch-check` to get each one's size without
/// reading its full content. Shells out to the real `git` binary rather
/// than `git2`/`libgit2` — same reasoning as `leakscan`/`commitguard`
/// elsewhere in this workspace: this only ever runs where `git` itself is
/// already guaranteed to exist.
pub fn batch_check_all_objects(repo_dir: &Path) -> Result<String> {
    let rev_list = Command::new("git")
        .args(["rev-list", "--objects", "--all"])
        .current_dir(repo_dir)
        .output()
        .context("running git rev-list")?;
    if !rev_list.status.success() {
        anyhow::bail!(
            "git rev-list failed: {}",
            String::from_utf8_lossy(&rev_list.stderr)
        );
    }

    let mut cat_file = Command::new("git")
        .args([
            "cat-file",
            "--batch-check=%(objectname) %(objecttype) %(objectsize) %(rest)",
        ])
        .current_dir(repo_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("spawning git cat-file")?;

    // The naive version of this — write all of rev-list's stdout to
    // cat-file's stdin, *then* read cat-file's stdout — deadlocks on any
    // repo with enough objects to fill a pipe buffer (~64KB on Linux):
    // cat-file blocks writing output nobody's reading yet, which stops it
    // reading more input, which blocks this process's write in turn.
    // Caught live against this workspace's own real, multi-year git
    // history — none of the small hand-built test repos below ever had
    // enough objects to fill a pipe buffer, so they never exposed this.
    // Fixed by writing on a separate thread so both ends drain
    // concurrently, the standard fix for this exact deadlock shape.
    let mut stdin = cat_file.stdin.take().context("getting cat-file stdin")?;
    let rev_list_stdout = rev_list.stdout;
    let writer = thread::spawn(move || stdin.write_all(&rev_list_stdout));

    let output = cat_file
        .wait_with_output()
        .context("waiting for git cat-file")?;
    writer
        .join()
        .expect("writer thread panicked")
        .context("writing object list to cat-file")?;
    if !output.status.success() {
        anyhow::bail!(
            "git cat-file failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{aggregate_by_path, parse_batch_check};
    use std::fs;

    fn temp_repo(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gitbloat-test-{}-{}", std::process::id(), name));
        fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        dir
    }

    fn commit_file(dir: &Path, filename: &str, content: &[u8], message: &str) {
        fs::write(dir.join(filename), content).unwrap();
        Command::new("git")
            .args(["add", filename])
            .current_dir(dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", message])
            .current_dir(dir)
            .status()
            .unwrap();
    }

    #[test]
    fn real_repo_pipeline_correctly_sums_two_distinct_versions_of_one_file() {
        let repo = temp_repo("two-versions");
        commit_file(
            &repo,
            "data.bin",
            &vec![0u8; 1000],
            "add data.bin, 1000 bytes",
        );
        commit_file(
            &repo,
            "data.bin",
            &vec![0u8; 5000],
            "grow data.bin to 5000 bytes",
        );

        let raw = batch_check_all_objects(&repo).unwrap();
        let objects = parse_batch_check(&raw);
        let stats = aggregate_by_path(&objects);

        let data_bin = stats.iter().find(|s| s.path == "data.bin").unwrap();
        assert_eq!(
            data_bin.version_count, 2,
            "two genuinely different blob contents were committed"
        );
        assert_eq!(
            data_bin.total_size, 6000,
            "1000 + 5000 — both distinct versions counted"
        );
        assert_eq!(data_bin.peak_size, 5000);

        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn deleting_a_file_does_not_remove_it_from_the_history_accounting() {
        let repo = temp_repo("deleted-file");
        commit_file(
            &repo,
            "big_temp_file.bin",
            &vec![1u8; 8000],
            "add a big file",
        );
        Command::new("git")
            .args(["rm", "-q", "big_temp_file.bin"])
            .current_dir(&repo)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "remove the big file"])
            .current_dir(&repo)
            .status()
            .unwrap();

        let raw = batch_check_all_objects(&repo).unwrap();
        let objects = parse_batch_check(&raw);
        let stats = aggregate_by_path(&objects);

        let found = stats.iter().find(|s| s.path == "big_temp_file.bin");
        assert!(found.is_some(), "a deleted file's blob is still reachable from history and must still be reported — that's the entire point of this tool");
        assert_eq!(found.unwrap().total_size, 8000);

        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn unrelated_unchanged_files_are_not_double_counted_across_commits() {
        let repo = temp_repo("unchanged");
        commit_file(&repo, "readme.txt", b"same content forever", "first commit");
        commit_file(
            &repo,
            "other.txt",
            b"a different file entirely",
            "second commit, readme.txt untouched",
        );

        let raw = batch_check_all_objects(&repo).unwrap();
        let objects = parse_batch_check(&raw);
        let stats = aggregate_by_path(&objects);

        let readme = stats.iter().find(|s| s.path == "readme.txt").unwrap();
        assert_eq!(
            readme.version_count, 1,
            "readme.txt's content never changed, so it must appear as exactly one version, not two"
        );

        fs::remove_dir_all(&repo).ok();
    }
}
