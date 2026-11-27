//! Which of its package's targets (library, binary, test...) each file is in.
//! SCIP's symbols name a package, not a target, so this follows each target's
//! module declarations from its root file.

use std::collections::HashMap;

use super::is_under;
use crate::model::{NodeId, Tree};
use crate::workspace::TargetKind;

pub struct Unit<'a> {
    pub path: &'a str,
    pub module: NodeId,
    /// The file's module and the modules written inline in it.
    pub defines: Vec<NodeId>,
    /// Every module the file names.
    pub names: Vec<NodeId>,
}

/// For each unit, the first target (libraries first) whose module tree holds
/// it.
pub fn assign(
    units: &[Unit],
    roots: &[(TargetKind, &str)],
    tree: &Tree,
) -> Vec<Option<TargetKind>> {
    let mut by_module: HashMap<NodeId, Vec<usize>> = HashMap::new();
    let mut by_path = HashMap::new();
    for (index, unit) in units.iter().enumerate() {
        by_module.entry(unit.module).or_default().push(index);
        by_path.insert(unit.path, index);
    }
    let mut roots = roots.to_vec();
    roots.sort();
    let mut kinds = vec![None; units.len()];
    for (kind, root) in roots {
        let Some(&start) = by_path.get(root) else {
            continue;
        };
        if kinds[start].is_some() {
            continue;
        }

        // A target's modules live under its root's directory: this keeps a test
        // crate's `mod util;` apart from the library's.
        let directory = root.rsplit_once('/').map_or("", |(directory, _)| directory);
        kinds[start] = Some(kind);
        let mut queue = vec![start];
        while let Some(unit) = queue.pop() {
            let unit = &units[unit];
            for &named in &unit.names {
                let declared_here = tree
                    .parent(named)
                    .is_some_and(|parent| unit.defines.contains(&parent));
                if !declared_here {
                    continue;
                }
                for &candidate in by_module.get(&named).into_iter().flatten() {
                    if kinds[candidate].is_none() && is_under(units[candidate].path, directory) {
                        kinds[candidate] = Some(kind);
                        queue.push(candidate);
                    }
                }
            }
        }
    }
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeKind;
    use crate::scip::with_slashes;

    #[test]
    fn a_test_crate_keeps_its_own_modules_and_the_library_keeps_its() {
        let mut tree = Tree::new();
        let package = tree.child(tree.root(), NodeKind::Package, "p");
        let util = tree.child(package, NodeKind::Module, "util");
        let unit = |path, module, names: Vec<NodeId>| Unit {
            path,
            module,
            defines: vec![module],
            names,
        };
        let units = [
            unit("src/lib.rs", package, vec![util]),
            unit("src/util.rs", util, vec![]),
            unit("tests/it.rs", package, vec![util]),
            unit("tests/util.rs", util, vec![]),
            unit("src/main.rs", package, vec![]),
        ];
        let roots = [
            (TargetKind::Test, "tests/it.rs"),
            (TargetKind::Binary, "src/main.rs"),
            (TargetKind::Library, "src/lib.rs"),
        ];
        let kinds = assign(&units, &roots, &tree);
        assert_eq!(
            kinds,
            [
                Some(TargetKind::Library),
                Some(TargetKind::Library),
                Some(TargetKind::Test),
                Some(TargetKind::Test),
                Some(TargetKind::Binary),
            ]
        );
    }

    #[test]
    fn a_test_crate_holds_its_files_whichever_separator_the_index_writes() {
        let mut tree = Tree::new();
        let package = tree.child(tree.root(), NodeKind::Package, "p");
        let util = tree.child(package, NodeKind::Module, "util");
        // Cargo's paths, `/`-separated on every platform.
        let roots = [(TargetKind::Test, "tests/it.rs")];
        for separator in ['/', '\\'] {
            let [it, util_rs] = [
                format!("tests{separator}it.rs"),
                format!("tests{separator}util.rs"),
            ]
            .map(|written| with_slashes(&written, separator).into_owned());
            let units = [
                Unit {
                    path: &it,
                    module: package,
                    defines: vec![package],
                    names: vec![util],
                },
                Unit {
                    path: &util_rs,
                    module: util,
                    defines: vec![util],
                    names: vec![],
                },
            ];
            assert_eq!(
                assign(&units, &roots, &tree),
                [Some(TargetKind::Test), Some(TargetKind::Test)]
            );
        }
    }
}
