pub struct Args {
    pub verbose: u32,
    quiet: bool,
}

impl Args {
    pub fn parse() -> Args {
        Args {
            verbose: 0,
            quiet: false,
        }
    }

    pub fn level(&self) -> u32 {
        if self.quiet { 0 } else { self.verbose + 1 }
    }
}
