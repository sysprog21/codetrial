//! The defense task (docs/mycpu-defense.md): questions about the student's
//! own ca2026-mycpu work, asked over their checkout, with every prediction,
//! edit and run recorded for the instructors to read before the live defense.

mod checkout;
mod select;

pub use checkout::{Checkout, Restore, Stamp, Worktree, safe_name, valid_student_id};
pub use select::{Added, added_lines, choose, context, graded, worth_asking};

#[cfg(test)]
#[path = "../../tests/unit/tasks/defense.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/tasks/defense_checkout.rs"]
mod checkout_tests;
