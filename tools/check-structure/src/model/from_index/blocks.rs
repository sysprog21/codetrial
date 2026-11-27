//! Items written as blocks in one file, such as inline modules and functions,
//! and which of them holds a place in the file.

use std::cmp::Reverse;
use std::collections::HashMap;

use super::lines;
use crate::scip::Range;

pub struct Blocks<T> {
    blocks: Vec<(Range, T)>,
}

impl<T: Copy> Blocks<T> {
    /// A macro that writes several items gives each its own call as the block,
    /// so a block that items share says where none of them is: it is left out.
    pub fn new(blocks: Vec<(Range, T)>) -> Blocks<T> {
        let mut shares: HashMap<Range, usize> = HashMap::new();
        for (block, _) in &blocks {
            *shares.entry(*block).or_default() += 1;
        }
        let blocks = blocks
            .into_iter()
            .filter(|(block, _)| shares[block] == 1)
            .collect();
        Blocks { blocks }
    }

    /// The smallest block around `range`, other than `range` itself.
    pub fn innermost(&self, range: Range) -> Option<T> {
        self.blocks
            .iter()
            .filter(|(block, _)| *block != range && block.holds(range))
            .min_by_key(|(block, _)| (lines(*block), Reverse(block.start)))
            .map(|&(_, item)| item)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Range, T)> + '_ {
        self.blocks.iter().copied()
    }
}
