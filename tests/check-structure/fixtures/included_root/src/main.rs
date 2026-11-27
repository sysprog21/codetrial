mod cli;

fn main() {
    let mut args = cli::Args::parse();
    args.verbose += 1;
    std::process::exit(args.level() as i32);
}
