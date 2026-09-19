use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectInfo {
    pub oid: String,
    pub size: u64,
    pub path: String,
}

/// Parses `git cat-file --batch-check='%(objectname) %(objecttype)
/// %(objectsize) %(rest)'` output, keeping only `blob` entries with a
/// non-empty path (`%(rest)` is empty for the commit/tree lines
/// `git rev-list --objects --all` also emits, which aren't file content
/// and aren't what this tool is answering "what's bloating this repo"
/// about).
pub fn parse_batch_check(output: &str) -> Vec<ObjectInfo> {
    let mut result = Vec::new();
    for line in output.lines() {
        let mut parts = line.splitn(4, ' ');
        let (Some(oid), Some(otype), Some(size_str)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if otype != "blob" {
            continue;
        }
        let Ok(size) = size_str.parse::<u64>() else {
            continue;
        };
        let path = parts.next().unwrap_or("").trim().to_string();
        if path.is_empty() {
            continue;
        }
        result.push(ObjectInfo {
            oid: oid.to_string(),
            size,
            path,
        });
    }
    result
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathStats {
    pub path: String,
    /// Sum of every *distinct* blob ever stored at this path — since
    /// `git rev-list --objects` already deduplicates identical content by
    /// SHA, an unchanged file re-appearing across many commits is not
    /// double-counted here; only genuinely different versions add to this.
    pub total_size: u64,
    pub peak_size: u64,
    pub version_count: usize,
}

/// Aggregates per-object sizes by path, sorted by `total_size` descending
/// — the actual "what's bloating this repo" ranking.
pub fn aggregate_by_path(objects: &[ObjectInfo]) -> Vec<PathStats> {
    let mut by_path: HashMap<&str, (u64, u64, usize)> = HashMap::new();
    for obj in objects {
        let entry = by_path.entry(obj.path.as_str()).or_insert((0, 0, 0));
        entry.0 += obj.size;
        entry.1 = entry.1.max(obj.size);
        entry.2 += 1;
    }
    let mut result: Vec<PathStats> = by_path
        .into_iter()
        .map(|(path, (total, peak, count))| PathStats {
            path: path.to_string(),
            total_size: total,
            peak_size: peak,
            version_count: count,
        })
        .collect();
    result.sort_by(|a, b| {
        b.total_size
            .cmp(&a.total_size)
            .then_with(|| a.path.cmp(&b.path))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blob_lines_and_skips_commit_and_tree_lines() {
        let output = "\
abc123 commit 250
def456 tree 60
789xyz blob 1000 src/big_file.bin
";
        let objects = parse_batch_check(output);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].path, "src/big_file.bin");
        assert_eq!(objects[0].size, 1000);
    }

    #[test]
    fn distinct_blob_shas_at_the_same_path_are_separate_versions() {
        let output = "\
aaa111 blob 100 config.yaml
bbb222 blob 5000 config.yaml
";
        let objects = parse_batch_check(output);
        let stats = aggregate_by_path(&objects);
        assert_eq!(stats.len(), 1);
        assert_eq!(
            stats[0].total_size, 5100,
            "two genuinely different versions must both count toward total size"
        );
        assert_eq!(stats[0].peak_size, 5000);
        assert_eq!(stats[0].version_count, 2);
    }

    #[test]
    fn identical_object_referenced_once_is_not_double_counted() {
        // git rev-list --objects --all only ever lists a given SHA once,
        // regardless of how many commits reference it — this input
        // reflects that (one line, not one per referencing commit).
        let output = "same1sha blob 2000 unchanged.txt\n";
        let objects = parse_batch_check(output);
        let stats = aggregate_by_path(&objects);
        assert_eq!(stats[0].total_size, 2000);
        assert_eq!(stats[0].version_count, 1);
    }

    #[test]
    fn results_are_sorted_by_total_size_descending() {
        let output = "\
a blob 100 small.txt
b blob 9000 huge.bin
c blob 500 medium.txt
";
        let objects = parse_batch_check(output);
        let stats = aggregate_by_path(&objects);
        assert_eq!(stats[0].path, "huge.bin");
        assert_eq!(stats[1].path, "medium.txt");
        assert_eq!(stats[2].path, "small.txt");
    }

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        let output = "garbage\nnot enough fields here\n";
        assert_eq!(parse_batch_check(output), vec![]);
    }
}
