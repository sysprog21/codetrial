//! Whether the interviewer behaves the way the live prompt asks, and the rules
//! that decide it.
//!
//! Two halves, the same split `tests/recording_integration.rs` makes. The rules
//! are plain functions and run in every `cargo test`: what counts as naming the
//! published problem, as volunteering a limit, as answering one. The run itself
//! needs a Gemini key and costs requests, so it is `#[ignore]` and runs through
//! `scripts/interview-behavior-check.sh`:
//!
//! ```text
//! GOOGLE_API_KEY=... cargo test --test interview_behavior -- --ignored
//! ```
//!
//! The live interviewer speaks through a native-audio model this harness cannot
//! drive, so it holds a text model to the same instructions, the same greeting
//! and the same tools, answered by the production dispatch. That is a proxy: it
//! catches a prompt that fails to say what it means, not a voice model that
//! reads a correct prompt differently.

use codetrial::agent::{
    InterviewGrounding, InterviewLoop, InterviewProfile, Problem, RuntimeState,
    build_instructions_for_plan, find_problem, get_problem, greeting, log_hint_text,
    names_published_problem,
};
use codetrial::gemini::{GeminiFunctionCall, live_tool_declarations};
use codetrial::livekit::execute_tool_call;
use codetrial::runtime::TOOL_LOG_HINT;
use serde_json::{Value, json};

#[path = "common/words.rs"]
mod words;
use words::{names_title, shared_run, words};

/// The published title, by the rule the generator applies, or the site.
fn names_source(problem: &Problem, reply: &str) -> bool {
    // "LeetCode" splits at its case change like any identifier.
    let spoken = words(reply);
    spoken.iter().any(|word| word == "leetcode")
        || spoken
            .windows(2)
            .any(|pair| pair[0] == "leet" && pair[1] == "code")
        || names_published_problem(problem.title, reply)
}

/// The limits a candidate has to ask for, as they could be said: `10^4` also
/// as `10000`. Only figures that identify a limit, so the zero in `0 <= amount`
/// does not make every "zero" a leak.
fn limit_figures(problem: &Problem) -> Vec<String> {
    let mut figures = Vec::new();
    for line in problem.variant().constraints {
        for token in line.split(|character: char| !(character.is_ascii_digit() || character == '^'))
        {
            if let Some((base, exponent)) = token.split_once('^') {
                figures.push(token.to_string());
                if let (Ok(base), Ok(exponent)) = (base.parse::<u64>(), exponent.parse::<u32>())
                    && let Some(value) = base.checked_pow(exponent)
                {
                    figures.push(value.to_string());

                    // `2^31 - 1` is said as 2147483647 as often as it is
                    // written out, and matching only 2147483648 fails the right
                    // answer.
                    let after = line.split(token).nth(1).unwrap_or_default().trim_start();
                    if after.starts_with("- 1") || after.starts_with("-1") {
                        figures.push((value - 1).to_string());
                    }
                }
            } else if token.parse::<u64>().is_ok_and(|value| value >= 100) {
                figures.push(token.to_string());
            }
        }
    }
    figures.sort();
    figures.dedup();
    figures
}

/// The first limit figure a reply states, with thousands separators ignored.
fn states_limit(problem: &Problem, reply: &str) -> Option<String> {
    let reply = reply.replace(',', "");
    limit_figures(problem)
        .into_iter()
        .find(|figure| reply.contains(figure.as_str()))
}

/// Terms that name a technique, each matched as whole words with its common
/// inflections, so "sorted" counts and "settle" is not "set".
///
/// Specific phrases rather than the short words inside them. The authored rungs
/// say "set aside", "water depth", "kept prefix" and "a playlist queue" as
/// plain
/// English, and a bare "set" or "depth" here would fail a faithful paraphrase
/// of the very clue a reply was served.
const TECHNIQUE_TERMS: &[&str] = &[
    "sort",
    "pointer",
    "hash",
    "hashmap",
    "hashset",
    "dictionary",
    "heap",
    "priority queue",
    "stack",
    "queue",
    "dynamic programming",
    "memoization",
    "memoize",
    "binary search",
    "sliding window",
    "prefix sum",
    "greedy",
    "recursion",
    "recursive",
    "backtracking",
    "breadth-first",
    "depth-first",
    "bfs",
    "dfs",
    "trie",
    "union-find",
    "topological",
    "xor",
    "bitmask",
];

/// Whether `text` uses `term` as whole words, allowing a trailing inflection
/// on its last word.
fn uses(text: &str, term: &str) -> bool {
    let (text, term) = (words(text), words(term));
    let last = term.len() - 1;
    text.windows(term.len()).enumerate().any(|(at, window)| {
        let inflected = window[..last] == term[..last]
            && ["", "s", "es", "ed", "ing"]
                .iter()
                .any(|suffix| window[last] == format!("{}{suffix}", term[last]));
        // "What sort of ordering" is English, not the technique.
        let idiom = term == ["sort"] && text.get(at + 1).is_some_and(|next| next == "of");
        inflected && !idiom
    })
}

/// The technique terms a reply names that neither the clue it was served nor
/// the scenario brief already uses. The brief is on the candidate's screen, so
/// a word it says (a playlist "queue", a mine shaft's "stack" of levels) is the
/// scenario's vocabulary rather than a hint.
fn named_beyond(reply: &str, allowed: &str) -> Vec<&'static str> {
    TECHNIQUE_TERMS
        .iter()
        .copied()
        .filter(|term| uses(reply, term) && !uses(allowed, term))
        .collect()
}

#[test]
fn the_rules_catch_a_named_source_and_a_volunteered_limit() {
    let problem = get_problem(Some("3sum"));
    assert!(names_source(problem, "Sure, this is basically 3 Sum."));
    assert!(names_title(problem.title, "Sure, this is basically 3 Sum."));
    assert!(!names_source(problem, "Those 3 sums all cancel out."));
    assert!(names_source(
        get_problem(Some("lru-cache")),
        "So this is the classic LRUCache?"
    ));
    assert!(names_source(problem, "It is on LeetCode, yes."));
    assert!(!names_source(
        problem,
        "Let us stay with the ledger version."
    ));

    let figures = limit_figures(problem);
    assert!(figures.contains(&"3000".to_string()), "{figures:?}");
    assert!(figures.contains(&"100000".to_string()), "{figures:?}");
    assert_eq!(
        states_limit(problem, "Up to 3,000 adjustments."),
        Some("3000".to_string())
    );
    assert_eq!(states_limit(problem, "Could be a handful or zero."), None);
    assert_eq!(
        shared_run("sort the array first", "Sort the array, fix one"),
        3
    );

    // The replies the first live run gave to the second and third requests,
    // held to the clue each was served.
    let [_, ordering, _] = problem.variant().hints else {
        panic!("three rungs");
    };
    assert_eq!(
        named_beyond(
            "If you sorted the list of adjustments first, how might that help?",
            ordering
        ),
        vec!["sort"]
    );
    assert_eq!(
        named_beyond(
            "With a sorted list, could you use two pointers from each end?",
            ordering
        ),
        vec!["sort", "pointer"]
    );
    assert!(named_beyond("What ordering would help here?", ordering).is_empty());
    assert!(named_beyond("What sort of ordering would help here?", ordering).is_empty());
    assert_eq!(
        named_beyond("Would you sort them first?", ordering),
        vec!["sort"]
    );
    assert!(
        limit_figures(get_problem(Some("coin-change"))).contains(&"2147483647".to_string()),
        "a limit written as 2^31 - 1 is also its value"
    );
    // Plain English the rungs themselves use.
    for ordinary in [
        "Is there a subset you could skip?",
        "Which pole can you set aside, and which side can you settle first?",
        "What do you need to hold in memory about the water depth so far?",
        "Since the kept prefix is in order, which earlier position matters?",
        "Could you map out what each step changes?",
    ] {
        assert!(named_beyond(ordinary, "any clue").is_empty(), "{ordinary}");
    }
    assert_eq!(
        named_beyond("Would a hash map of seen values help?", "any clue"),
        vec!["hash"]
    );
    assert_eq!(
        named_beyond("Try a depth-first walk.", "any clue"),
        vec!["depth-first"]
    );
    // A word the scenario itself uses is not a hint when the reply repeats it.
    let brief = "The music player keeps the upcoming queue as a chain of tracks.";
    assert!(named_beyond("Flip part of the queue by hand.", brief).is_empty());
    assert_eq!(
        named_beyond("Flip part of the queue by hand.", "any clue"),
        vec!["queue"]
    );
}

struct Conversation {
    client: reqwest::Client,
    key: String,
    model: String,
    instructions: String,
    contents: Vec<Value>,
    state: RuntimeState,
}

struct Turn {
    reply: String,
    hint_calls: Vec<bool>,
    /// What `log_hint` returned this turn, the clue the reply is held to.
    served: String,
}

impl Conversation {
    /// One request, waiting out a rate limit rather than failing on it: a free
    /// key allows fifteen a minute and one scripted problem takes about ten.
    async fn generate(&self) -> Value {
        for _ in 0..8 {
            let response: Value = self
                .client
                .post(format!(
                    "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                    self.model
                ))
                .header("x-goog-api-key", &self.key)
                .json(&json!({
                    "systemInstruction": { "parts": [{ "text": self.instructions }] },
                    "contents": self.contents,
                    "tools": [{ "functionDeclarations": live_tool_declarations() }],
                    "generationConfig": { "temperature": 0.7 },
                }))
                .send()
                .await
                .expect("Gemini is reachable")
                .json()
                .await
                .expect("Gemini answers JSON");
            if response["error"]["code"] != 429 {
                return response;
            }
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
        panic!("still rate limited after waiting");
    }

    async fn say(&mut self, text: &str) -> Turn {
        self.contents
            .push(json!({ "role": "user", "parts": [{ "text": text }] }));
        let mut hint_calls = Vec::new();
        let mut served = String::new();
        for _ in 0..6 {
            let response = self.generate().await;
            let content = response["candidates"][0]["content"].clone();
            assert!(content.is_object(), "no candidate: {response}");
            self.contents.push(content.clone());
            let parts = content["parts"].as_array().cloned().unwrap_or_default();
            let calls = parts
                .iter()
                .filter_map(|part| part.get("functionCall"))
                .collect::<Vec<_>>();
            if calls.is_empty() {
                let reply = parts
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .collect::<String>();
                return Turn {
                    reply,
                    hint_calls,
                    served,
                };
            }
            let answers = calls
                .iter()
                .map(|call| {
                    let call = GeminiFunctionCall {
                        id: String::new(),
                        name: call["name"].as_str().unwrap_or_default().to_string(),
                        args: call.get("args").cloned().unwrap_or_else(|| json!({})),
                    };
                    let response = execute_tool_call(&mut self.state, &call);
                    if call.name == TOOL_LOG_HINT {
                        // Read off what production answered rather than the
                        // flag, so a missing flag counts the way dispatch
                        // counts it: anything but the bare record was a
                        // requested hint.
                        let result = response["result"].as_str().unwrap_or_default();
                        hint_calls.push(result != log_hint_text(self.state.hints_used));
                        served.push_str(result);
                    }
                    println!("    tool {} {} -> {}", call.name, call.args, response);
                    json!({ "functionResponse": { "name": call.name, "response": response } })
                })
                .collect::<Vec<_>>();
            self.contents
                .push(json!({ "role": "user", "parts": answers }));
        }
        panic!("the model kept calling tools without replying");
    }
}

/// The key the binary would use: the config file the script names over the
/// environment, read by the application's own parser so quoting and spacing
/// the binary accepts are accepted here too. A missing file leaves the
/// environment to answer, the way a key exported for a one-off run does.
fn gemini_key() -> String {
    let from_file = std::env::var("CODETRIAL_ENV")
        .ok()
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_file())
        .and_then(|path| {
            codetrial::config::read_config_file(&path)
                .unwrap_or_else(|error| panic!("{error}"))
                .into_iter()
                .rev()
                .find(|(name, _)| name == "GOOGLE_API_KEY")
                .map(|(_, value)| value)
        });
    from_file
        .or_else(|| std::env::var("GOOGLE_API_KEY").ok())
        .filter(|key| !key.is_empty())
        .expect("GOOGLE_API_KEY is set in the config file or the environment")
}

/// Every selector resolved before any request is made. `get_problem` opens the
/// default for a name it does not know, which is right for a link and wrong
/// here: a typo would script the default and report green for a problem that
/// was never exercised.
fn selected_problems(ids: &str) -> Vec<&'static Problem> {
    let ids = ids.split(',').map(str::trim);
    let unknown = ids
        .clone()
        .filter(|id| find_problem(id).is_none())
        .collect::<Vec<_>>();
    assert!(
        unknown.is_empty(),
        "BEHAVIOR_PROBLEMS names no problem for {unknown:?}"
    );
    ids.filter_map(find_problem).collect()
}

#[test]
fn a_behaviour_selector_must_name_a_problem() {
    // A page name selects its problem too; read from the bank, not written
    // here.
    let page = get_problem(Some("two-sum")).variant().page;
    let selected = selected_problems(&format!("3sum, {page}"));
    assert_eq!(selected[0].id, "3sum");
    assert_eq!(selected[1].id, "two-sum");
    for bad in ["3sum,coin-chnage", "3sum,", ""] {
        assert!(
            std::panic::catch_unwind(|| selected_problems(bad)).is_err(),
            "{bad:?} was accepted"
        );
    }
}

/// One scripted candidate per problem, and every rule the prompt states that a
/// transcript can show. Failures are collected rather than asserted one at a
/// time, so a run reports everything it saw.
#[tokio::test]
#[ignore = "needs GOOGLE_API_KEY and makes Gemini requests; run scripts/interview-behavior-check.sh"]
async fn live_interviewer_poses_the_variant_and_serves_hints_in_order() {
    let key = gemini_key();
    let model = std::env::var("GEMINI_BEHAVIOR_MODEL")
        .unwrap_or_else(|_| codetrial::config::DEFAULT_GEMINI_REPORT_MODEL.to_string());
    let ids = std::env::var("BEHAVIOR_PROBLEMS")
        .unwrap_or_else(|_| "3sum,coin-change,two-sum".to_string());
    let problems = selected_problems(&ids);
    let mut failures = Vec::new();

    for problem in problems {
        let mut conversation = Conversation {
            client: reqwest::Client::new(),
            key: key.clone(),
            model: model.clone(),
            instructions: build_instructions_for_plan(
                problem,
                45,
                &InterviewProfile::default(),
                &InterviewGrounding::default(),
                InterviewLoop::CodingBehavioral,
            ),
            contents: Vec::new(),
            state: RuntimeState::for_problem(problem),
        };
        let mut fail = |what: String| failures.push(format!("{}: {what}", problem.id));
        let brief = problem.variant().brief_text();
        let [_, _, key_step] = problem.variant().hints else {
            panic!("three rungs");
        };

        let opening = conversation.say(&greeting(problem)).await;
        println!("[{}] Jim: {}", problem.id, opening.reply);
        if names_source(problem, &opening.reply) {
            fail(format!("the greeting names the source: {}", opening.reply));
        }
        if let Some(figure) = states_limit(problem, &opening.reply) {
            fail(format!("the greeting volunteers the limit {figure}"));
        }

        let language = conversation.say("I'll use Python.").await;
        println!("[{}] Jim: {}", problem.id, language.reply);
        if let Some(figure) = states_limit(problem, &language.reply) {
            fail(format!("volunteered the limit {figure} before being asked"));
        }

        let guess = conversation
            .say(&format!(
                "Quick question first: is this basically the LeetCode problem {}?",
                problem.title
            ))
            .await;
        println!("[{}] Jim: {}", problem.id, guess.reply);
        if names_source(problem, &guess.reply) {
            fail(format!("confirmed or repeated the source: {}", guess.reply));
        }

        let limit = conversation
            .say("What is the largest input I should plan for?")
            .await;
        println!("[{}] Jim: {}", problem.id, limit.reply);
        if states_limit(problem, &limit.reply).is_none() {
            fail(format!("did not answer the size question: {}", limit.reply));
        }

        for (asked, rungs) in [
            ("I'm honestly stuck on how to start. Can I get a hint?", 1),
            ("Thanks. Could I get another hint?", 2),
        ] {
            let hint = conversation.say(asked).await;
            println!("[{}] Jim: {}", problem.id, hint.reply);
            if !hint.hint_calls.contains(&true) {
                fail(format!("answered {asked:?} without log_hint requested"));
            }
            if conversation.state.hint_rungs_given != rungs {
                fail(format!(
                    "expected {rungs} rungs served, found {}",
                    conversation.state.hint_rungs_given
                ));
            }
            if shared_run(&hint.reply, problem.optimal) >= 5 {
                fail(format!(
                    "a hint recites the optimal approach: {}",
                    hint.reply
                ));
            }
            let beyond = named_beyond(&hint.reply, &format!("{} {brief}", hint.served));
            if !beyond.is_empty() {
                fail(format!(
                    "a hint names {beyond:?} beyond its clue: {}",
                    hint.reply
                ));
            }
        }

        // No approach has been offered, so the key step stays withheld unless
        // the model recorded an approach on the candidate's behalf, which is
        // its own failure.
        let third = conversation
            .say("Still not seeing it. One more hint?")
            .await;
        println!("[{}] Jim: {}", problem.id, third.reply);
        if conversation.state.hint_rungs_given > 2 {
            fail("served the key step before the candidate stated an approach".to_string());
        }
        if shared_run(&third.reply, key_step) >= 6 {
            fail(format!("spoke the withheld rung: {}", third.reply));
        }
        let beyond = named_beyond(&third.reply, &format!("{} {brief}", third.served));
        if !beyond.is_empty() {
            fail(format!(
                "the withheld request names {beyond:?} instead: {}",
                third.reply
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} behaviour failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
