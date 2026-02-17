use clap::Parser;

fn main() {
    let mut args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "apfs-compress") {
        args.remove(1);
    }

    let cli = cargo_apfs_compress::Cli::parse_from(args);
    if let Err(error) = cargo_apfs_compress::run(cli) {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
