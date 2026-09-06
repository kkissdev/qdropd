//! Generate man pages and shell completions from the CLI definition.
//!
//!   cargo run -p qdrop --example gen-artifacts -- packaging
//!
//! writes `packaging/man/qdrop.1` and `packaging/completions/qdrop.{bash,zsh,fish}`.

#[path = "../src/cli.rs"]
#[allow(dead_code)] // only the clap derive metadata is used here
mod cli;

use std::path::PathBuf;

use clap::CommandFactory;
use clap_complete::Shell;

fn main() -> std::io::Result<()> {
    let out: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "packaging".into())
        .into();

    let mut cmd = cli::Cli::command();
    cmd.set_bin_name("qdrop");

    let man_dir = out.join("man");
    std::fs::create_dir_all(&man_dir)?;
    let mut buf = Vec::new();
    clap_mangen::Man::new(cmd.clone()).render(&mut buf)?;
    std::fs::write(man_dir.join("qdrop.1"), &buf)?;

    // Sub-command man pages.
    for sub in cmd.get_subcommands() {
        let name = sub.get_name().to_string();
        if name == "help" {
            continue;
        }
        let mut b = Vec::new();
        clap_mangen::Man::new(sub.clone().bin_name(format!("qdrop {name}"))).render(&mut b)?;
        std::fs::write(man_dir.join(format!("qdrop-{name}.1")), &b)?;
    }

    let comp_dir = out.join("completions");
    std::fs::create_dir_all(&comp_dir)?;
    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
        clap_complete::generate_to(shell, &mut cmd, "qdrop", &comp_dir)?;
    }

    println!(
        "wrote man pages to {} and completions to {}",
        man_dir.display(),
        comp_dir.display()
    );
    Ok(())
}
