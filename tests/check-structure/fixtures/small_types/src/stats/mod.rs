mod rate;
mod tally;
mod window;

pub use rate::Rate;
pub use tally::Tally;
pub use window::Window;

pub fn percent(part: u64, whole: u64) -> u64 {
    (part * 100).checked_div(whole).unwrap_or(0)
}
