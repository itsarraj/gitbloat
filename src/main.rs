use std::path::PathBuf;

use clap::Parser;
use gitbloat::{format::format_bytes, gitobjects, parse};

#[derive(Parser)]
#[command(
    name = "gitbloat",
    about = "What's bloating this git repo, including files since deleted but still in history"
)]
struct Cli {
    #[arg(default_value = ".")]
    repo: PathBuf,

    #[arg(long, default_value_t = 20)]
    top: usize,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let raw = gitobjects::batch_check_all_objects(&cli.repo)?;
    let objects = parse::parse_batch_check(&raw);
    let stats = parse::aggregate_by_path(&objects);

    let total: u64 = stats.iter().map(|s| s.total_size).sum();
    println!(
        "{} distinct paths, {} across all history\n",
        stats.len(),
        format_bytes(total)
    );

    println!("{:>10}  {:>10}  {:>4}  path", "total", "peak", "vers");
    for entry in stats.iter().take(cli.top) {
        println!(
            "{:>10}  {:>10}  {:>4}  {}",
            format_bytes(entry.total_size),
            format_bytes(entry.peak_size),
            entry.version_count,
            entry.path
        );
    }

    Ok(())
}
