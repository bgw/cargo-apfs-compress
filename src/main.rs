use clap::Parser;

fn main() {
    let cli = cargo_apfs_compress::Cli::parse();
    if let Err(error) = cargo_apfs_compress::run(cli) {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
