//! Scratch check: one real `generate_report` call through
//! `CODETRIAL_GEMINI_REST_BASE`. Not part of the gate.
//!
//! `REPORT_PROMPT_FILE` swaps in another prompt and `REPORT_PROBLEM` names the
//! problem it was written for, which is what the report is validated against:
//! the published title it must not name, among other things. Both default to
//! the Two Sum golden prompt.
use codetrial::agent::find_problem;
use codetrial::gemini::generate_report;

#[tokio::test]
#[ignore = "needs a generateContent server at CODETRIAL_GEMINI_REST_BASE"]
async fn local_report() {
    let prompt = match std::env::var("REPORT_PROMPT_FILE") {
        Ok(path) => std::fs::read_to_string(path).unwrap(),
        Err(_) => {
            let golden: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string("tests/golden/prompts.json").unwrap(),
            )
            .unwrap();
            golden["report"].as_str().unwrap().to_string()
        }
    };

    // `find_problem` rather than `get_problem`, which opens the default for a
    // name it does not know: a typo would validate against Two Sum and pass a
    // report that names the real problem.
    let id = std::env::var("REPORT_PROBLEM").unwrap_or_else(|_| "two-sum".to_string());
    let problem =
        find_problem(&id).unwrap_or_else(|| panic!("REPORT_PROBLEM names no problem: {id}"));
    let started = std::time::Instant::now();
    let report = generate_report("local", "local", &prompt, problem)
        .await
        .expect("report");
    println!("elapsed {:.1}s", started.elapsed().as_secs_f64());
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
