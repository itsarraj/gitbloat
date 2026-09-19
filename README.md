# gitbloat

What's bloating this git repo — including files since deleted but still
sitting in history. GitHub's own `git-sizer` covers part of this
read-only, without a ranked "biggest offenders" view; nothing gives you
the equivalent of `dust`/`ncdu` specifically over git history.

## Usage

```bash
gitbloat                    # current directory
gitbloat /path/to/repo --top 30
```

## How the accounting works

Built on `git rev-list --objects --all | git cat-file --batch-check`,
which lists every object reachable from any ref, deduplicated by SHA
across all of history — so an unchanged file that survives 500 commits
untouched is counted **once**, not 500 times, and a file that was edited
5 times shows as 5 distinct versions with a real summed size. Deleting a
file doesn't remove its blob from history — the object is still
reachable from an old commit, and this tool reports it exactly like any
other path, which is the entire point: `git rm` doesn't shrink your
`.git` directory, and this shows you why.

## Status: built, verified with hand-built repos, then genuinely deadlocked on this workspace's own real history — and got fixed because of it

- **9 unit tests** (`cargo test --lib`): parsing `batch-check` output
  (blob lines kept, commit/tree lines correctly skipped, malformed lines
  non-fatal), aggregation (two distinct blob SHAs at the same path summed
  correctly, one SHA referenced once *not* double-counted, descending
  sort by total size), and — against **real, hand-built git repositories**,
  not fixtures — the full `rev-list | cat-file` pipeline correctly
  summing two genuinely different versions of a file, confirming a
  **deleted** file's blob still shows up in the accounting (the actual
  point of the tool), and confirming an unrelated unchanged file across
  two commits reports as exactly one version.
- **Then run against this workspace's own real, multi-year git repository
  — and it deadlocked.** The first version wrote all of `git rev-list`'s
  output to `git cat-file`'s stdin in one blocking call, then read
  `cat-file`'s stdout afterward. That's fine for the small hand-built test
  repos (a handful of objects fit comfortably in a pipe buffer) and
  deadlocks on any repo with enough objects to fill one (~64KB on Linux):
  `cat-file` blocks writing output nobody's reading yet, which stops it
  reading more input, which blocks this process's write right back. None
  of the unit tests could have caught this — they were never big enough
  to. Fixed by writing on a separate thread so both ends of the pipe
  drain concurrently (the standard fix for this exact deadlock shape),
  then **re-verified against the same real repository**: completed in
  well under the retest's 30-second bound and produced a genuinely
  useful, correct-looking report — 5,114 distinct historical paths,
  56.2 MiB total, correctly surfacing real offenders (a large committed
  JSON eval dataset, PNG assets, lockfiles — including one lockfile
  showing exactly 2 versions with the right summed size, meaning it was
  regenerated once in this repo's real history).

**Not done / deliberately deferred**: filtering by path prefix or commit
range (currently always walks full history on all refs); a
`--delete-from-history` action (this tool only *reports* bloat — actually
rewriting history to remove it needs `git filter-repo` or the BFG, tools
that already do that one job well, not reimplemented here).
