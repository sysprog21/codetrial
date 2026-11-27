use lib::Shown;
use lib::detail::Kept;
use lib::inner::Hidden;

fn main() {
    let _ = (Hidden, Shown, size_of::<Kept>());
}
