//! The order that puts the members of a circle in layers with the fewest ties
//! against it. A circle has no order to read its members in, so any order has
//! ties that go against it: a member written lower using one written higher.
//! The ties of the best order are the ones to cut, and the rest is the layering
//! the code can be moved to.

/// Up to this many members the best order is found exactly, over subsets of
/// the members. Past it the order is built greedily, which is no worse than
/// the count of ties it leaves but may not be the fewest.
const MOST_EXACT: usize = 20;

/// A tie of one member, `user`, by `weight` items, to another it `used`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub user: usize,
    pub used: usize,
    pub weight: usize,
}

/// What an order costs: how many ties go against it, then by how many items.
type Cost = (usize, usize);

/// The members `0..members` from the lowest layer up, and the indices in
/// `edges` of the ties that go against that order. The fewest ties go against
/// it, then the fewest items among them. Ties between such orders go to the
/// one the search reaches first, so the same circle always answers the same.
pub fn layers(members: usize, edges: &[Edge]) -> (Vec<usize>, Vec<usize>) {
    let order = if members <= MOST_EXACT {
        exact(members, edges)
    } else {
        greedy(members, edges)
    };
    let mut layer = vec![0; members];
    for (at, &member) in order.iter().enumerate() {
        layer[member] = at;
    }
    let against = edges
        .iter()
        .enumerate()
        .filter(|(_, edge)| layer[edge.user] < layer[edge.used])
        .map(|(index, _)| index)
        .collect();
    (order, against)
}

/// What putting `member` on top of the members in `below` adds: the ties it
/// has to members not yet placed, which end up above it.
fn added(member: usize, below: u32, edges: &[Edge]) -> Cost {
    edges
        .iter()
        .filter(|edge| edge.user == member && edge.used != member && below & (1 << edge.used) == 0)
        .fold((0, 0), |(count, items), edge| {
            (count + 1, items + edge.weight)
        })
}

fn exact(members: usize, edges: &[Edge]) -> Vec<usize> {
    let all = (1u32 << members) - 1;

    // The best cost of ordering exactly the members in a subset as the lowest
    // layers, and the member that was placed last.
    let mut best: Vec<Option<(Cost, usize)>> = vec![None; 1 << members];
    best[0] = Some(((0, 0), 0));
    for below in 0..=all {
        let Some((cost, _)) = best[below as usize] else {
            continue;
        };
        for member in (0..members).filter(|member| below & (1 << member) == 0) {
            let step = added(member, below | (1 << member), edges);
            let total = (cost.0 + step.0, cost.1 + step.1);
            let next = (below | (1 << member)) as usize;

            // Strictly better, so the first order reached stays on a tie.
            if best[next].is_none_or(|(known, _)| total < known) {
                best[next] = Some((total, member));
            }
        }
    }
    let mut order = Vec::with_capacity(members);
    let mut below = all;
    while below != 0 {
        let (_, member) = best[below as usize].expect("every subset is reached");
        order.push(member);
        below &= !(1 << member);
    }
    order.reverse();
    order
}

fn greedy(members: usize, edges: &[Edge]) -> Vec<usize> {
    let mut placed = vec![false; members];
    let mut order = Vec::with_capacity(members);
    while order.len() < members {
        let cost = |member: usize| {
            edges
                .iter()
                .filter(|edge| edge.user == member && edge.used != member && !placed[edge.used])
                .fold((0, 0), |(count, items), edge| {
                    (count + 1, items + edge.weight)
                })
        };
        let next = (0..members)
            .filter(|&member| !placed[member])
            .min_by_key(|&member| (cost(member), member))
            .expect("a member is left");
        placed[next] = true;
        order.push(next);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(user: usize, used: usize, weight: usize) -> Edge {
        Edge { user, used, weight }
    }

    #[test]
    fn a_chain_with_one_tie_back_has_that_tie_against_it() {
        // a uses b, b uses c, c uses a: cutting the cheapest tie (c uses a)
        // leaves c lowest, then b, then a on top.
        let edges = [edge(0, 1, 3), edge(1, 2, 2), edge(2, 0, 1)];
        let (order, against) = layers(3, &edges);
        assert_eq!(order, vec![2, 1, 0]);
        assert_eq!(against, [2]);
    }

    #[test]
    fn a_circle_always_has_a_tie_against_any_order() {
        let edges = [edge(0, 1, 1), edge(1, 0, 1)];
        let (order, against) = layers(2, &edges);
        assert_eq!(order.len(), 2);
        assert_eq!(against.len(), 1);
    }

    #[test]
    fn the_exact_order_is_the_best_of_every_order() {
        // A small deterministic generator, so a failure names its graph.
        let mut seed = 0x2545_f491_u64;
        let mut next = move |most: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % most
        };
        for _ in 0..200 {
            let members = 2 + next(5);
            let edges: Vec<Edge> = (0..next(12) + 1)
                .map(|_| edge(next(members), next(members), 1 + next(4)))
                .filter(|edge| edge.user != edge.used)
                .collect();
            let (order, against) = layers(members, &edges);
            let cost_of = |order: &[usize]| -> Cost {
                let mut layer = vec![0; members];
                for (at, &member) in order.iter().enumerate() {
                    layer[member] = at;
                }
                edges
                    .iter()
                    .filter(|e| layer[e.user] < layer[e.used])
                    .fold((0, 0), |(c, w), e| (c + 1, w + e.weight))
            };
            let mut best = cost_of(&order);
            assert_eq!(
                best,
                (
                    against.len(),
                    against.iter().map(|&at| edges[at].weight).sum()
                )
            );
            let mut permutation: Vec<usize> = (0..members).collect();
            permute(&mut permutation, 0, &mut |candidate| {
                assert!(
                    best <= cost_of(candidate),
                    "{members} members, {edges:?}: {order:?} loses to {candidate:?}"
                );
                best = best.min(cost_of(candidate));
            });
        }
    }

    fn permute(items: &mut Vec<usize>, from: usize, visit: &mut impl FnMut(&[usize])) {
        if from == items.len() {
            visit(items);
            return;
        }
        for at in from..items.len() {
            items.swap(from, at);
            permute(items, from + 1, visit);
            items.swap(from, at);
        }
    }

    #[test]
    fn past_the_exact_limit_the_order_is_still_complete() {
        let members = MOST_EXACT + 3;
        let edges: Vec<Edge> = (0..members)
            .map(|member| edge(member, (member + 1) % members, 1))
            .collect();
        let (order, against) = layers(members, &edges);
        let mut seen = order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..members).collect::<Vec<_>>());
        assert_eq!(against.len(), 1, "a ring is cut once");
    }
}
