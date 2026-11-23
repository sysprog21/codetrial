//! One defense attempt: the plan, the private worktree, and every prediction,
//! run and claim, in the order they happened. Its methods block on builds and
//! runs; the web layer calls them off the async runtime, one at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::checkout::{Checkout, Worktree, safe_name};
use super::obs::{Table, ended_by_itself, first_divergence_in, read, retire_digest, retire_window};
use super::runner::{Ended, run};
use super::{Config, CourseTask, LineItem, Plan, edit_matches};

/// The widest window of cycles a simulation run shows.
pub const MAX_WINDOW: u64 = 256;
/// How far a claimed cycle may be from the first divergence and still count.
pub const CYCLE_SLACK: u64 = 8;
/// The largest retire log CodeTrial reads back.
const MAX_LOG_BYTES: u64 = 256 * 1024 * 1024;
/// The cycles one simulation may run: the harness has no limit of its own, and
/// at about a hundred bytes a cycle this keeps its retire log within what
/// CodeTrial reads back.
pub const MAX_CYCLES: u64 = 2_000_000;
/// The runs one line item may have: enough to recover from a mistake, few
/// enough that the result stays within its size limit.
pub const MAX_RUNS_PER_ITEM: usize = 6;

/// Why a request of the page was refused, as its error code.
type Refusal = &'static str;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prediction {
    pub at: u64,
    /// The spec classes the student says will fail.
    pub fails: Vec<String>,
    /// False when the student says the changed sources will not compile, so
    /// no spec runs at all.
    pub compiles: bool,
    pub how: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Outcome {
    /// The specs ran; these failed.
    Tests { failed: Vec<String> },
    /// The changed sources did not compile, so no spec ran.
    CompileError,
    /// The tools failed: a timeout, or a command that could not start.
    TelemetryGap { reason: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecRun {
    pub item: String,
    pub started_at: u64,
    pub ended_at: u64,
    pub edited: String,
    pub specs: Vec<String>,
    pub prediction: Prediction,
    pub outcome: Outcome,
    /// The prediction named exactly the specs that failed. `None` for a run
    /// that says nothing about the change.
    pub matched: Option<bool>,
    /// A match that shows something: a failure it predicted, or every spec
    /// of the task run when it predicted none.
    pub conclusive: bool,
    /// ScalaTest's own lines of the run: each test's result, the failures
    /// and the totals, or the compiler's errors; what the page shows.
    pub summary: String,
}

/// The lines of an sbt run a learner reads: test results, failures with
/// their indented details, totals and compile errors with the lines that
/// explain them, at most `SUMMARY_LINES` of them and `SUMMARY_BYTES`.
pub fn summary_of(tail: &str) -> String {
    const SUMMARY_LINES: usize = 40;
    const SUMMARY_BYTES: usize = 4096;
    fn body(line: &str) -> Option<&str> {
        line.strip_prefix("[info]")
            .or_else(|| line.strip_prefix("[error]"))
            .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
    }
    let wanted = |line: &str| {
        let text = body(line).unwrap_or(line).trim();
        text.starts_with("- ")
            || text.ends_with(':') && text.split_whitespace().count() == 1
            || text.starts_with("Tests: ")
            || text.starts_with("*** ")
            || text.starts_with("All tests passed")
            || text.contains("Compilation failed")
            || line.starts_with("[error]") && text.contains(".scala:")
    };

    // A failure's message and a compile error's explanation follow it on lines
    // indented under it.
    let opens_details = |line: &str| {
        line.contains("*** FAILED ***") || line.starts_with("[error]") && line.contains(".scala:")
    };
    let mut kept: Vec<&str> = Vec::new();
    let mut details = false;
    for line in tail.lines() {
        let indented = body(line).is_some_and(|text| text.starts_with(char::is_whitespace));
        if details && indented {
            kept.push(line);
        } else if wanted(line) {
            kept.push(line);
            details = opens_details(line);
        } else {
            details = false;
        }
    }
    kept.drain(..kept.len().saturating_sub(SUMMARY_LINES));
    let mut summary = String::new();
    for line in kept.iter().rev() {
        // A line over the whole limit by itself is left out, not allowed to
        // empty the summary of every line before it.
        if line.len() + 1 > SUMMARY_BYTES {
            continue;
        }
        if summary.len() + line.len() + 1 > SUMMARY_BYTES {
            break;
        }
        summary.insert_str(0, &format!("{line}\n"));
    }
    summary
}

/// The two models of the bug item, in the order they are built.
const MODELS: [Model; 2] = [Model::Clean, Model::Bug];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Model {
    Clean,
    Bug,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimRun {
    pub at: u64,
    pub model: Model,
    pub program: String,
    pub first: u64,
    pub count: u64,
    pub exit: Option<i32>,
    pub retire: Vec<(u64, String)>,
    pub table: Option<Table>,
    pub error: Option<String>,
}

/// What the student said of the bug, as they said it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stated {
    pub at: u64,
    pub program: String,
    pub cycle: u64,
    pub signal: String,
    pub rule: String,
    pub exercise: String,
}

/// A claim and the checks CodeTrial made of it, which only the record of the
/// marks reads: neither the page nor the result the student keeps shows them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    #[serde(flatten)]
    pub stated: Stated,
    /// The bug model's first differing retire record on that program.
    pub first_divergence: Option<u64>,
    pub cycle_close: Option<bool>,
    pub exercise_matches: Option<bool>,
}

/// What the page is shown of a line item: never the bug's mutant or rule.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BugBrief {
    pub task: String,
    pub variant: Option<String>,
    pub programs: Vec<String>,
    pub built: bool,
    pub build_error: Option<String>,
}

/// The build tools an attempt runs, by the name it starts them with.
#[derive(Debug, Clone)]
pub struct Tools {
    pub sbt: String,
    pub make: String,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            sbt: "sbt".to_owned(),
            make: "make".to_owned(),
        }
    }
}

pub struct Attempt {
    tools: Tools,
    config: Config,
    checkout: Checkout,
    plan: Plan,
    worktree: Option<Worktree>,
    predictions: BTreeMap<String, Prediction>,
    runs: Vec<SpecRun>,
    models: BTreeMap<Model, PathBuf>,
    build_error: Option<String>,
    sims: Vec<SimRun>,
    /// The retire records of the last whole run of each model on each
    /// program, by cycle and digest: all a divergence check compares.
    logs: BTreeMap<(Model, String), Vec<(u64, u64)>>,
    claim: Option<Claim>,
    /// Each line item's spec classes, read once from the stamped commit.
    specs: BTreeMap<String, Vec<String>>,
    /// The programs built for the bug item's task when the attempt began.
    programs: Vec<String>,
    /// Their files as they were then: both models run these, whatever the
    /// student rebuilds later. None when the copy could not be made.
    program_files: Option<Scratch>,
    /// When the learner opened each item, by id (`bug` for the bug item):
    /// an item's overview comes first, and nothing acts on it before.
    begun: BTreeMap<String, u64>,
}

impl Attempt {
    pub fn new(config: Config, checkout: Checkout, plan: Plan, tools: Tools) -> Self {
        let mut by_dir: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let specs = plan
            .lines
            .iter()
            .map(|item| {
                let specs = config
                    .task_of(&item.file)
                    .map(|task| {
                        by_dir
                            .entry(&task.dir)
                            .or_insert_with(|| checkout.specs(&plan.stamp.head, &task.dir))
                            .clone()
                    })
                    .unwrap_or_default();
                (item.id.clone(), specs)
            })
            .collect();
        let arch = config
            .tasks
            .iter()
            .find(|task| task.dir == plan.bug.task)
            .map(|task| task.arch.clone());
        let built = arch
            .as_deref()
            .map(|arch| checkout.root().join("build/sw/mycpu").join(arch));
        let mut programs = arch
            .as_deref()
            .map(|arch| checkout.programs(arch))
            .unwrap_or_default();

        // The names and the files are taken together, now: a program is the one
        // the attempt began with, and only a regular file is copied, so a link
        // cannot lead the copy out of the checkout.
        let program_files = built.and_then(|from| {
            let scratch = scratch_dir().ok()?;
            programs.retain(|name| {
                let source = from.join(name);
                std::fs::symlink_metadata(&source).is_ok_and(|meta| meta.is_file())
                    && std::fs::copy(&source, scratch.0.join(name)).is_ok()
            });
            Some(scratch)
        });
        if program_files.is_none() {
            programs.clear();
        }
        Self {
            specs,
            programs,
            program_files,
            tools,
            config,
            checkout,
            plan,
            worktree: None,
            predictions: BTreeMap::new(),
            runs: Vec::new(),
            models: BTreeMap::new(),
            build_error: None,
            sims: Vec::new(),
            logs: BTreeMap::new(),
            claim: None,
            begun: BTreeMap::new(),
        }
    }

    /// Opens `item` once its overview has been read; true the first time.
    pub fn begin(&mut self, item: &str, now: u64) -> Result<bool, Refusal> {
        if item != "bug" {
            self.item(item)?;
        }
        if self.begun.contains_key(item) {
            return Ok(false);
        }
        self.begun.insert(item.to_owned(), now);
        Ok(true)
    }

    /// When the learner opened `item`, if they have.
    pub fn begun_at(&self, item: &str) -> Option<u64> {
        self.begun.get(item).copied()
    }

    /// Refuses a request on an item the learner has not opened.
    fn opened(&self, item: &str) -> Result<(), Refusal> {
        if self.begun.contains_key(item) {
            Ok(())
        } else {
            Err("item_not_begun")
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The hints the package lets Jim give in this attempt.
    pub fn hints_allowed(&self) -> u64 {
        self.config.hints
    }

    pub fn runs(&self) -> &[SpecRun] {
        &self.runs
    }

    pub fn sims(&self) -> &[SimRun] {
        &self.sims
    }

    pub fn claim(&self) -> Option<&Claim> {
        self.claim.as_ref()
    }

    pub fn prediction(&self, item: &str) -> Option<&Prediction> {
        self.predictions.get(item)
    }

    fn item(&self, id: &str) -> Result<&LineItem, Refusal> {
        self.plan
            .lines
            .iter()
            .find(|l| l.id == id)
            .ok_or("item_unknown")
    }

    fn bug_task(&self) -> Result<&CourseTask, Refusal> {
        self.config
            .tasks
            .iter()
            .find(|t| t.dir == self.plan.bug.task)
            .ok_or("package_invalid")
    }

    /// The spec classes the student may predict and run for a line item.
    pub fn specs_for(&self, item: &str) -> Result<Vec<String>, Refusal> {
        self.specs.get(item).cloned().ok_or("item_unknown")
    }

    pub fn bug_brief(&self) -> BugBrief {
        BugBrief {
            task: self.plan.bug.task.clone(),
            variant: self.plan.bug.variant.clone(),
            programs: self.programs.clone(),
            built: self.models.len() == 2,
            build_error: self.build_error.clone(),
        }
    }

    /// Record what the student predicts before a run of `item`. A later
    /// prediction replaces an earlier one until a run has started from it.
    pub fn predict(
        &mut self,
        item: &str,
        fails: Vec<String>,
        how: &str,
        now: u64,
    ) -> Result<(), Refusal> {
        let specs = self.specs_for(item)?;
        self.opened(item)?;
        if fails.iter().any(|f| !specs.contains(f)) || fails.len() > specs.len() {
            return Err("spec_unknown");
        }
        if !super::super::bounded_text(how, 1024) {
            return Err("prediction_invalid");
        }
        let fails: BTreeSet<String> = fails.into_iter().collect();
        self.record_prediction(item, fails.into_iter().collect(), true, how, now);
        Ok(())
    }

    /// Record a prediction that the change will not compile.
    pub fn predict_no_compile(&mut self, item: &str, how: &str, now: u64) -> Result<(), Refusal> {
        self.item(item)?;
        self.opened(item)?;
        if !super::super::bounded_text(how, 1024) {
            return Err("prediction_invalid");
        }
        self.record_prediction(item, Vec::new(), false, how, now);
        Ok(())
    }

    fn record_prediction(
        &mut self,
        item: &str,
        fails: Vec<String>,
        compiles: bool,
        how: &str,
        now: u64,
    ) {
        self.predictions.insert(
            item.to_owned(),
            Prediction {
                at: now,
                fails,
                compiles,
                how: how.trim().to_owned(),
            },
        );
    }

    /// Check what a run of `item` needs before it starts: the edit is the
    /// change asked for, a prediction is recorded, the specs are the task's.
    pub fn check_run(&self, item: &str, edited: &str, specs: &[String]) -> Result<(), Refusal> {
        let line = self.item(item)?;
        self.opened(item)?;
        if !edit_matches(line, edited) {
            return Err("edit_mismatch");
        }
        let Some(prediction) = self.predictions.get(item) else {
            return Err("prediction_required");
        };
        let known = self.specs_for(item)?;
        if specs.is_empty()
            || specs.iter().any(|s| !known.contains(s))
            || specs.iter().collect::<BTreeSet<_>>().len() != specs.len()
        {
            return Err("spec_unknown");
        }
        // A run that leaves out a spec the prediction names cannot confirm it.
        if prediction.fails.iter().any(|f| !specs.contains(f)) {
            return Err("prediction_unrun");
        }
        Ok(())
    }

    /// Run the chosen specs of `item`'s task with its change in the worktree.
    /// The prediction it starts from is frozen into the run, and consumed.
    pub fn run_specs(
        &mut self,
        item: &str,
        edited: &str,
        specs: &[String],
        clock: &dyn Fn() -> u64,
    ) -> Result<&SpecRun, Refusal> {
        self.check_run(item, edited, specs)?;
        if self.runs.iter().filter(|run| run.item == item).count() >= MAX_RUNS_PER_ITEM {
            return Err("run_limit");
        }
        let line = self.item(item)?.clone();
        let prediction = self.predictions.remove(item).ok_or("prediction_required")?;
        let project = self
            .config
            .task_of(&line.file)
            .map(|t| t.project.clone())
            .ok_or("item_unknown")?;
        let started_at = clock();
        let limit = Duration::from_secs(self.config.spec_seconds);
        let env = self.env();
        let sbt = self.tools.sbt.clone();
        // The student's line, at the original line's indentation.
        let original = line
            .near
            .get(line.line - line.near_first)
            .map_or("", String::as_str);
        let indent = &original[..original.len() - original.trim_start().len()];
        let replacement = format!("{indent}{}", edited.trim());
        let with = line.changed_line.as_ref().map(|_| replacement.as_str());
        let (outcome, tail) = match self.worktree() {
            Err(reason) => (Outcome::TelemetryGap { reason }, String::new()),
            Ok(worktree) => match worktree.edit(&line.file, line.line, with) {
                Err(reason) => (Outcome::TelemetryGap { reason }, String::new()),
                Ok(_restore) => {
                    let command = format!("{project}/testOnly {}", specs.join(" "));
                    let ran = run(
                        &sbt,
                        &["-batch".to_owned(), command],
                        worktree.dir(),
                        &env,
                        limit,
                    );
                    (outcome_of(&ran.ended, &ran.tail, specs), ran.tail)
                }
            },
        };
        let matched = match &outcome {
            // Both lists are sorted and without repeats.
            Outcome::Tests { failed } => Some(prediction.compiles && prediction.fails == *failed),
            Outcome::CompileError => Some(!prediction.compiles),
            Outcome::TelemetryGap { .. } => None,
        };

        // "Nothing fails" is confirmed only by every spec of the task, not by
        // one that happens to pass.
        let every = self.specs_for(item)?.len() == specs.len();
        // A build that fails as predicted shows as much as a failing spec.
        let conclusive = matched == Some(true)
            && (every
                || matches!(&outcome, Outcome::CompileError)
                || matches!(&outcome, Outcome::Tests { failed } if !failed.is_empty()));
        self.runs.push(SpecRun {
            item: item.to_owned(),
            started_at,
            ended_at: clock(),
            edited: edited.trim().to_owned(),
            specs: specs.to_vec(),
            prediction,
            outcome,
            matched,
            conclusive,
            summary: summary_of(&tail),
        });
        self.runs.last().ok_or("run_lost")
    }

    /// Build the two models of the bug item, the core with and without the
    /// mutant, unless they are built already. `still_open` is asked before
    /// each: once the attempt has ended, the second build is not started.
    pub fn build_models(&mut self, still_open: impl Fn() -> bool) -> Result<(), Refusal> {
        self.opened("bug")?;
        if MODELS.iter().all(|model| self.models.contains_key(model)) {
            return Ok(());
        }
        let task = self.bug_task()?.clone();
        let bug = self.plan.bug.clone();
        let limit = Duration::from_secs(self.config.model_seconds);
        let env = self.env();
        let dir = match self.worktree() {
            Ok(worktree) => worktree.dir().to_path_buf(),
            Err(reason) => {
                self.build_error = Some(reason);
                return Err("model_build_failed");
            }
        };
        let mutants: Vec<String> = self.config.bugs.iter().map(|b| b.mutant.clone()).collect();
        for name in MODELS {
            let mutant = (name == Model::Bug).then_some(bug.mutant.as_str());
            if self.models.contains_key(&name) {
                continue;
            }
            if !still_open() {
                self.build_error = None;
                return Err("defense_closed");
            }
            let mut args = vec![
                "--no-print-directory".to_owned(),
                "-C".to_owned(),
                task.dir.clone(),
                "verilator".to_owned(),
            ];
            if let Some(variant) = &bug.variant {
                args.push(format!("VARIANT={variant}"));
            }
            if let Some(mutant) = mutant {
                args.push(format!("MUTANT={mutant}"));
            }
            let ran = run(&self.tools.make, &args, &dir, &env, limit);
            let model = (ran.ended == Ended::Exited(0))
                .then(|| find_model(&dir, &task.dir, mutant, &mutants))
                .flatten();
            let Some(model) = model else {
                // The mutant is named in the tail; the page never sees it.
                self.build_error = Some(match ran.ended {
                    Ended::Timeout => "the model build timed out".to_owned(),
                    Ended::NotStarted(why) => why,
                    _ => "the model did not build: run `make verilator` for the task in your checkout".to_owned(),
                });
                return Err("model_build_failed");
            };
            self.models.insert(name, model);
        }
        self.build_error = None;
        Ok(())
    }

    /// Run `program` on one model and show `count` cycles from `first`.
    pub fn simulate(
        &mut self,
        model: Model,
        program: &str,
        first: u64,
        count: u64,
        now: u64,
    ) -> Result<&SimRun, Refusal> {
        self.opened("bug")?;
        let count = count.clamp(1, MAX_WINDOW);
        let task = self.bug_task()?.clone();
        if !safe_name(program) || !self.programs.iter().any(|p| p == program) {
            return Err("program_unknown");
        }
        let binary = self.models.get(&model).cloned().ok_or("model_not_built")?;

        // The copy made when the attempt began, so that both models run the
        // same program whatever the student rebuilds meanwhile.
        let elf = self
            .program_files
            .as_ref()
            .ok_or("program_unknown")?
            .0
            .join(program);
        let scratch = scratch_dir().map_err(|_| "run_failed")?;
        let log = scratch.0.join("retire.log");
        let trace = scratch.0.join("run.obs");
        let number = task.dir.split('-').next().unwrap_or("0");
        let args = vec![
            format!("+elf={}", elf.display()),
            format!("+retire-log={}", log.display()),
            format!("+obs-trace={}", trace.display()),
            format!("+obs-task=T{number}"),
            format!(
                "+obs-variant={}",
                self.plan.bug.variant.as_deref().unwrap_or("default")
            ),
            format!("+obs-start={first}"),
            format!("+obs-cycles={count}"),
            format!("+max-cycles={MAX_CYCLES}"),
        ];
        let dir = binary
            .parent()
            .map_or_else(|| scratch.0.clone(), Path::to_path_buf);
        let ran = run(
            &binary.to_string_lossy(),
            &args,
            &dir,
            &[],
            Duration::from_secs(self.config.sim_seconds),
        );
        let (exit, mut error) = match &ran.ended {
            // The harness's exit status for a run cut off at `+max-cycles`.
            Ended::Exited(124) => (
                Some(124),
                Some(format!(
                    "the run reached the {MAX_CYCLES}-cycle limit; choose a shorter program"
                )),
            ),
            Ended::Exited(code) => (Some(*code), None),
            Ended::Killed => (None, Some("the model was killed".to_owned())),
            Ended::Timeout => (None, Some("the run timed out".to_owned())),
            // Its reason names the model's path, and the path the mutant.
            Ended::NotStarted(_) => (None, Some("the model could not start".to_owned())),
        };
        let full_log = read_bounded(&log);
        if full_log.is_none() && log.exists() {
            error.get_or_insert_with(|| "the run's retire log is too long to compare".to_owned());
        }
        let retire = full_log
            .as_deref()
            .map(|text| retire_window(text, first, count))
            .unwrap_or_default();
        let table = match std::fs::read(&trace).map(|bytes| read(&bytes)) {
            Ok(Ok(table)) => Some(table),
            Ok(Err(why)) => {
                error.get_or_insert(why);
                None
            }
            Err(_) => None,
        };

        // Only a run that ended by itself has a whole log to compare.
        if let (Some(text), Ended::Exited(_)) = (full_log, &ran.ended)
            && ended_by_itself(&text)
        {
            self.logs
                .insert((model, program.to_owned()), retire_digest(&text));
        }
        self.sims.push(SimRun {
            at: now,
            model,
            program: program.to_owned(),
            first,
            count,
            exit,
            retire,
            table,
            error,
        });
        self.sims.last().ok_or("run_lost")
    }

    /// Record the student's account of the bug, and check what can be
    /// checked: the cycle against the first divergence on that program, the
    /// exercise against the package's.
    pub fn claim_bug(
        &mut self,
        program: &str,
        cycle: u64,
        signal: &str,
        rule: &str,
        exercise: &str,
        now: u64,
    ) -> Result<&Claim, Refusal> {
        self.opened("bug")?;
        // One claim: a second could probe the checks the first was given.
        if self.claim.is_some() {
            return Err("claim_final");
        }
        if !self.programs.iter().any(|p| p == program) {
            return Err("program_unknown");
        }
        let texts = [signal, rule, exercise];
        if texts.iter().any(|t| !super::super::bounded_text(t, 1024)) {
            return Err("claim_invalid");
        }

        // A claim names a program run on both models, however the runs ended:
        // one made before would be final unseen, and a bug model that never
        // finishes the program must not block it. Only two whole logs give a
        // cycle to check.
        let ran_on: BTreeSet<Model> = self
            .sims
            .iter()
            .filter(|sim| sim.program == program)
            .map(|sim| sim.model)
            .collect();
        if ran_on.len() < MODELS.len() {
            return Err("claim_unrun");
        }
        let divergence = match (
            self.logs.get(&(Model::Clean, program.to_owned())),
            self.logs.get(&(Model::Bug, program.to_owned())),
        ) {
            (Some(clean), Some(bug)) => first_divergence_in(clean, bug),
            _ => None,
        };
        let cycle_close = divergence.map(|at| at.abs_diff(cycle) <= CYCLE_SLACK);
        let exercise_matches = self
            .plan
            .bug
            .exercise
            .as_deref()
            .map(|expected| exercise_number(exercise).as_deref() == Some(expected));
        self.claim = Some(Claim {
            stated: Stated {
                at: now,
                program: program.to_owned(),
                cycle,
                signal: signal.trim().to_owned(),
                rule: rule.trim().to_owned(),
                exercise: exercise.trim().to_owned(),
            },
            first_divergence: divergence,
            cycle_close,
            exercise_matches,
        });
        self.claim.as_ref().ok_or("claim_invalid")
    }

    fn worktree(&mut self) -> Result<&Worktree, String> {
        if self.worktree.is_none() {
            let worktree = Worktree::create(&self.checkout, &self.plan.stamp.head)?;

            // The bootcamp gate's stamp, so the first build need not run the
            // bootcamp's specs again for a clone that has not changed.
            let stamp = self.checkout.root().join("build/bootcamp.ok");
            if stamp.is_file() {
                let target = worktree.dir().join("build");
                let _made = std::fs::create_dir_all(&target);
                let _copied = std::fs::copy(&stamp, target.join("bootcamp.ok"));
            }

            // The student's built programs, which specs and the models run: a
            // fresh worktree has none, and every spec that needs one would fail
            // for that rather than for the change.
            copy_tree(
                &self.checkout.root().join("build/sw"),
                &worktree.dir().join("build/sw"),
            );
            self.worktree = Some(worktree);
        }
        self.worktree
            .as_ref()
            .ok_or_else(|| "no worktree".to_owned())
    }

    /// Builds in the worktree read the student's `externals/`.
    ///
    /// The bootcamp is the student's clone as it is, `BOOTCAMP_DIR` naming it
    /// unless CodeTrial was started with one of its own, and `BOOTCAMP_SRC`
    /// emptied: that mode copies sources into `$EXTERNALS/chisel-bootcamp`,
    /// which here is the student's checkout, and a `BOOTCAMP_DIR` beside it
    /// would make the course's Makefile refuse the build. A relative
    /// `BOOTCAMP_DIR` is the student's checkout's, as the course reads it,
    /// never the worktree's, which has no `externals/`; an empty one is unset.
    fn env(&self) -> Vec<(String, String)> {
        let externals = self.checkout.root().join("externals");
        let bootcamp = bootcamp_dir(self.checkout.root(), std::env::var_os("BOOTCAMP_DIR"));
        vec![
            ("EXTERNALS".to_owned(), externals.display().to_string()),
            ("BOOTCAMP_DIR".to_owned(), bootcamp.display().to_string()),
            ("BOOTCAMP_SRC".to_owned(), String::new()),
        ]
    }
}

/// The bootcamp clone the builds read: `set`, CodeTrial's own `BOOTCAMP_DIR`,
/// relative to the student's checkout at `root`, or the checkout's own clone
/// when it is unset or empty.
pub(super) fn bootcamp_dir(root: &Path, set: Option<std::ffi::OsString>) -> PathBuf {
    set.filter(|dir| !dir.is_empty()).map_or_else(
        || root.join("externals/chisel-bootcamp"),
        |dir| root.join(dir),
    )
}

/// The first `<task>.<n>` in what the student wrote: "4.4", "Exercise 4.4"
/// and "4.4." all name exercise 4.4.
fn exercise_number(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    (0..bytes.len()).find_map(|start| {
        let digits = |from: usize| {
            bytes[from..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count()
        };
        let starts =
            bytes[start].is_ascii_digit() && (start == 0 || !bytes[start - 1].is_ascii_digit());
        let task = digits(start);
        let dot = start + task;
        let item = if starts && bytes.get(dot) == Some(&b'.') {
            digits(dot + 1)
        } else {
            0
        };
        (item > 0).then(|| text[start..dot + 1 + item].to_owned())
    })
}

/// What an sbt `testOnly` run says: the specs ScalaTest lists as failed or
/// as erroring, a confirmed compile failure, or a telemetry gap.
pub fn outcome_of(ended: &Ended, tail: &str, specs: &[String]) -> Outcome {
    match ended {
        Ended::Timeout => {
            return Outcome::TelemetryGap {
                reason: "timeout".to_owned(),
            };
        }
        Ended::NotStarted(why) => {
            return Outcome::TelemetryGap {
                reason: why.clone(),
            };
        }
        Ended::Killed => {
            return Outcome::TelemetryGap {
                reason: "killed".to_owned(),
            };
        }
        Ended::Exited(_) => {}
    }
    let mut failed = BTreeSet::new();
    let mut listing = false;
    for line in tail.lines() {
        let text = line.trim_start_matches("[error]").trim();
        if text == "Failed tests:" || text == "Error during tests:" {
            listing = true;
        } else if listing && specs.iter().any(|s| s == text) {
            failed.insert(text.to_owned());
        } else {
            listing = false;
        }
    }
    let ran_tests = tail.contains("Tests: succeeded") || tail.contains("All tests passed");
    if (ran_tests && *ended == Ended::Exited(0)) || !failed.is_empty() {
        Outcome::Tests {
            failed: failed.into_iter().collect(),
        }
    } else if tail.lines().any(|line| {
        let text = line.trim_start_matches("[error]").trim();
        text == "Compilation failed" || text.ends_with(") Compilation failed")
    }) {
        Outcome::CompileError
    } else {
        Outcome::TelemetryGap {
            reason: "unclassified sbt failure".to_owned(),
        }
    }
}

/// The model a `make verilator` just built: `build/t<n>.../Vsim`, the one
/// named after `mutant`, or with no mutant's name, the newest of them.
fn find_model(
    worktree: &Path,
    task: &str,
    mutant: Option<&str>,
    mutants: &[String],
) -> Option<PathBuf> {
    let number = task.split('-').next()?;
    let prefix = format!("t{number}");
    std::fs::read_dir(worktree.join("build"))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let ours = name == prefix || name.starts_with(&format!("{prefix}-"));
            let named = match mutant {
                Some(m) => name.ends_with(&format!("-{m}")),
                None => !mutants.iter().any(|m| name.ends_with(&format!("-{m}"))),
            };
            let binary = entry.path().join("Vsim");
            let modified = binary.metadata().ok()?.modified().ok()?;
            (ours && named).then_some((modified, binary))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, binary)| binary)
}

/// Copies the directories and regular files under `from` to `to`, leaving
/// links behind; what cannot be copied is left out.
fn copy_tree(from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    let _made = std::fs::create_dir_all(to);
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_file() {
            let _copied = std::fs::copy(entry.path(), target);
        }
    }
}

fn read_bounded(path: &Path) -> Option<String> {
    let length = std::fs::metadata(path).ok()?.len();
    let bytes = (length <= MAX_LOG_BYTES).then(|| std::fs::read(path).ok())??;
    // A log is ASCII; only a damaged one pays for the lossy copy.
    Some(
        String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    )
}

/// A temporary directory removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch_dir() -> std::io::Result<Scratch> {
    let name = crate::accounts::random_token(8)?;
    let dir = std::env::temp_dir().join(format!("codetrial-defense-run-{name}"));
    std::fs::create_dir(&dir)?;
    Ok(Scratch(dir))
}
