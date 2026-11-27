//! The starter cut held against every starter the problem pages serve: the
//! rule that tells an edited starter comment line from the candidate's own
//! sentence is a heuristic, so it is checked on each real line rather than on
//! a few chosen ones.

use super::*;

/// Words no starter comment line holds, for the words a candidate adds.
const ADDED: [&str; 3] = ["quietly", "zebra", "meanwhile"];

/// Each starter the pages serve, as its language and its comment lines in
/// `quote_words` spelling, the form `without_starter` is given.
fn starters() -> Vec<(String, Vec<String>)> {
    let mut starters = Vec::new();
    for entry in std::fs::read_dir("web/problems").unwrap() {
        let page = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        let page = serde_json::from_str::<serde_json::Value>(&page).unwrap();
        for (language, code) in page["starterCode"].as_object().unwrap() {
            let given = typed_comments(language, code.as_str().unwrap())
                .lines()
                .map(quote_words)
                .filter(|words| !words.is_empty())
                .collect::<Vec<_>>();
            starters.push((language.clone(), given));
        }
    }
    assert!(starters.len() > 100, "the problem pages were read");
    starters
}

/// `line` with each word of `added` put before the word at its index.
fn inserted(line: &str, added: &[(usize, &str)]) -> String {
    let mut words = line.split(' ').map(str::to_string).collect::<Vec<_>>();
    for (at, word) in added.iter().rev() {
        words.insert(*at, (*word).to_string());
    }
    words.join(" ")
}

/// What survives the cut: the words left as the candidate's.
fn kept(line: &str, given: &[String]) -> Vec<String> {
    without_starter(line, given)
        .split(' ')
        .filter(|word| *word != "|")
        .map(str::to_string)
        .collect()
}

/// Every starter comment line left as the starter wrote it is cut whole, a
/// short one such as "@return {number}" included; with a word or two inserted
/// into one of three words or more, only the inserted words are left.
#[test]
fn every_starter_line_is_cut_unchanged_and_with_words_inserted() {
    let mut wrong = Vec::new();
    for (language, given) in starters() {
        for line in &given {
            if !kept(line, &given).is_empty() {
                wrong.push(format!("{language} unchanged: {line}"));
            }
            let n = line.split(' ').count();
            if n < MIN_STARTER_WORDS_KEPT {
                continue;
            }
            let edits = [
                vec![(n / 2, ADDED[0])],
                vec![(1, ADDED[0]), (n - 1, ADDED[1])],
                vec![(n / 2, ADDED[0]), (n / 2, ADDED[1])],
                vec![(0, ADDED[2]), (n, ADDED[1])],
            ];
            for edit in edits {
                let edited = inserted(line, &edit);
                let mut added = edit.iter().map(|(_, word)| *word).collect::<Vec<_>>();
                let mut left = kept(&edited, &given);
                added.sort_unstable();
                left.sort_unstable();
                if left != added {
                    wrong.push(format!("{language} edited: {edited} kept {left:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A sentence that reuses a starter line's words with the candidate's own
/// between them is theirs: each line of three words or more, its words in
/// order with two added words after each, keeps every word.
#[test]
fn no_starter_line_cuts_a_sentence_spreading_its_words() {
    let mut wrong = Vec::new();
    for (language, given) in starters() {
        for line in given.iter().filter(|line| line.split(' ').count() >= 3) {
            let spread = line
                .split(' ')
                .map(|word| format!("{word} {} {}", ADDED[0], ADDED[1]))
                .collect::<Vec<_>>()
                .join(" ");
            if kept(&spread, &given).len() != spread.split(' ').count() {
                wrong.push(format!("{language}: {spread}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Restatements a candidate might type beside the starter's prose comment
/// lines, each reusing some of a line's words in its order. None is cut.
#[test]
fn a_restatement_reusing_a_starter_lines_words_is_kept() {
    let sentences = [
        "For a binary tree, return the label of each node seen from the left",
        "for a node with no children the answer is zero",
        "the input is a singly linked list, and for each node I keep the list",
        "the returned array holds every index, and the caller must free it",
        "I return an array of groups, and both the array and columnSizes are malloced by me",
        "I return an array of host groups, each of size k",
        "the sizes of the groups are returned too, one size per group array",
        "I do not return anything, I modify the board in place",
        "I do not return anything and change the root in place",
        "I will think about it as I go along",
        "each CommandIndex object is called with insert and search, as such calls arrive",
        "the OffsetMonitor class keeps each offset it is given",
    ];
    let mut wrong = Vec::new();
    for (language, given) in starters() {
        for sentence in sentences.map(quote_words) {
            let left = kept(&sentence, &given).join(" ");
            if left != sentence {
                wrong.push(format!("{language}: {sentence} kept {left}"));
            }
        }
    }
    wrong.dedup();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A known limit: the kept words need only be most of the stretch they span,
/// so a sentence that keeps four of the five words of "Definition for a
/// TileTree node." with three of its own among them is cut as that line
/// edited, and only its other words are the candidate's.
#[test]
fn a_sentence_keeping_most_of_a_short_starter_line_is_cut() {
    let (_, given) = starters()
        .into_iter()
        .find(|(_, given)| {
            given
                .iter()
                .any(|line| line == "definition for a tiletree node")
        })
        .expect("a TileTree starter");
    let sentence = quote_words("for a tiletree that is one node, return its value");
    assert_eq!(
        kept(&sentence, &given).join(" "),
        "that is one return its value"
    );
}

/// Where a starter line could be read at two places, the choice settles which
/// of the candidate's words stay: the earliest stretch keeping the most of it
/// is cut, and inside a stretch, of two equally long runs, the one that keeps
/// the line's earlier word. A line `x y x` in `x y x y x` is cut from the
/// first word, and `p q r s t u` read in `p q s r t u` keeps `s` as the
/// starter's and leaves `r`.
#[test]
fn a_starter_line_readable_at_two_places_is_cut_at_the_first() {
    let cut = |words: &str, line: &str| without_starter(words, &[line.to_string()]);
    assert_eq!(cut("x y x y x", "x y x"), "| | | y x");
    assert_eq!(cut("p q s r t u", "p q r s t u"), "| | | r | |");
}

/// A starter line typed twice is cut twice, and one typed once with the
/// candidate's words after it leaves those words: the cut goes on past its
/// first find and stops once nothing more is the starter's.
#[test]
fn a_starter_line_typed_twice_is_cut_twice() {
    let cut = |words: &str, line: &str| without_starter(words, &[line.to_string()]);
    assert_eq!(cut("a b c d a b c d", "a b c d"), "| | | | | | | |");
    assert_eq!(cut("a b c d e f", "a b c d"), "| | | | e f");
}

/// The run of a line's words a diff would keep, not the first equal word each
/// time: `a b` in `b a b` keeps the last two, where taking the leading `b`
/// would keep one. Of two runs equally long, the one starting earlier in the
/// text is kept. Each table entry and each stop of the walk is needed, and the
/// walk ends whichever of the two runs out first.
#[test]
fn kept_in_order_keeps_a_longest_run_rather_than_the_first_match() {
    let kept = |line: &str, text: &str| {
        let line = line.split(' ').collect::<Vec<_>>();
        let text = text.split(' ').collect::<Vec<_>>();
        kept_in_order(&line, &text)
    };
    assert_eq!(kept("a b", "b a b"), [1, 2]);
    assert_eq!(kept("x a b", "a b"), [0, 1]);
    assert_eq!(kept("a b c", "a c b c"), [0, 2, 3]);
    assert_eq!(kept("b a c", "a b c"), [0, 2]);
    assert_eq!(kept("a", "a b"), [0]);
    assert_eq!(kept("a", "a a"), [0]);
    assert_eq!(kept("a a b", "b a"), [0]);
    assert_eq!(kept("a b", "a"), [0]);
    assert_eq!(kept("a", "b"), [] as [usize; 0]);
}
