use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::session::Phase;
use super::{TaskError, bounded_text, default_number, default_value, identifier, invalid};

/// One downloaded task, shared by every attempt at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exercise(pub Arc<TaskRecord>);

impl Exercise {
    pub fn id(&self) -> &str {
        &self.0.id
    }

    pub fn title(&self) -> &str {
        &self.0.title
    }

    pub fn starter(&self, language: &str) -> Option<&str> {
        self.0.starters.get(language).map(String::as_str)
    }

    pub fn hints(&self) -> &[String] {
        &self.0.hints[..self.0.sidecar.max_hint_rungs]
    }

    pub fn understanding_checks(&self) -> &[UnderstandingCheck] {
        &self.0.sidecar.understanding_checks
    }

    /// The marked code a task asks the learner to complete.
    pub fn completion_targets(&self) -> &[CompletionTarget] {
        &self.0.sidecar.completion_targets
    }

    /// The languages the learner may choose from, the first offered first.
    pub fn languages(&self) -> &[String] {
        &self.0.sidecar.languages
    }

    /// The targets marked in `language`'s starter, the only ones an attempt
    /// in it can complete.
    pub fn targets_in(&self, language: &str) -> Vec<&CompletionTarget> {
        self.completion_targets()
            .iter()
            .filter(|target| target.language == language)
            .collect()
    }

    /// The question of the check named `id`.
    pub fn check_question(&self, id: &str) -> Option<&str> {
        self.understanding_checks()
            .iter()
            .find(|check| check.id == id)
            .map(|check| check.question.as_str())
    }

    /// The private notes and reference code the review is graded against,
    /// one per line, the code empty when the bank has none.
    pub fn review_reference(&self) -> String {
        let record = &self.0;
        format!(
            "{}\n{}\n{}",
            record.optimal,
            record.pitfalls,
            record.reference_code.as_deref().unwrap_or("")
        )
    }

    pub fn brief(&self) -> &Value {
        &self.0.brief
    }

    pub fn contract(&self) -> &str {
        &self.0.contract
    }

    /// What an attempt in `language` is told about the task: its targets in
    /// that language and nothing marked in another. Constructed by field
    /// selection, so new private bank fields cannot leak.
    pub fn live_projection(&self, language: &str) -> Value {
        let record = &self.0;
        json!({"title": record.title, "brief": record.brief,
            "contract": record.contract, "clarifications": record.clarifications,
            "hints": self.hints(), "language": language,
            "completionTargets": self.targets_in(language),
            "understandingChecks": record.sidecar.understanding_checks})
    }

    #[cfg(test)]
    pub fn task_prompt(
        &self,
        phase: Phase,
        target: Option<&str>,
        code: &str,
        language: &str,
    ) -> Result<String, TaskError> {
        self.task_prompt_with_hint_limit(phase, target, code, language, self.hints().len())
    }

    pub fn task_prompt_with_hint_limit(
        &self,
        phase: Phase,
        target: Option<&str>,
        code: &str,
        language: &str,
        hint_limit: usize,
    ) -> Result<String, TaskError> {
        let record = &self.0;
        let instructor = record.sidecar.interaction_prompt.clone();
        let target_goal = match target {
            Some(id) => self
                .targets_in(language)
                .into_iter()
                .find(|row| row.id == id)
                .map(|row| row.goal.as_str())
                .ok_or_else(|| invalid("unknown completion target"))?,
            None => "",
        };
        let instructor = interpolate(
            &instructor,
            &BTreeMap::from([
                ("title", self.title()),
                ("language", language),
                ("target", target_goal),
                ("phase", phase.as_str()),
            ]),
        )?;
        let mut projection = self.live_projection(language);
        projection.as_object_mut().unwrap().remove("hints");
        projection["hintRungsMax"] = json!(hint_limit);
        let projection = projection.to_string();
        interpolate(
            include_str!("../../problem-bank/task-prompt.txt"),
            &BTreeMap::from([
                ("phase", phase.as_str()),
                ("language", language),
                ("instructorPrompt", instructor.as_str()),
                ("publicProjection", projection.as_str()),
                ("code", serde_json::to_string(code).unwrap().as_str()),
            ]),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionTarget {
    pub id: String,
    pub language: String,
    pub marker: String,
    pub goal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnderstandingCheck {
    pub id: String,
    pub question: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sidecar {
    pub prompt_version: u32,
    pub interaction_prompt: String,
    pub completion_targets: Vec<CompletionTarget>,
    pub understanding_checks: Vec<UnderstandingCheck>,
    pub required_cases: Vec<String>,
    pub max_hint_rungs: usize,
    pub rubric_profile: String,
    /// The languages the learner may choose from, the first offered first.
    /// A sidecar that names none allows the default language alone.
    #[serde(default = "default_languages")]
    pub languages: Vec<String>,
}

fn default_languages() -> Vec<String> {
    vec![default_value("language").as_str().unwrap().to_owned()]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    id: String,
    title: String,
    brief: Value,
    contract: String,
    clarifications: Value,
    starters: BTreeMap<String, String>,
    hints: Vec<String>,
    reference_code: Option<String>,
    optimal: String,
    pitfalls: String,
    judge: Value,
    sidecar: Sidecar,
    /// Each required case's index in `judge.cases`, bound once by its source
    /// label so a variant rename cannot move it.
    required_cases: Vec<usize>,
}

impl TaskRecord {
    pub fn from_bank(
        problem: &Value,
        judge: &Value,
        variant: &Value,
        sidecar: Option<&Value>,
    ) -> Result<Self, TaskError> {
        // Every shape the posing below unwraps (rename maps of strings, class
        // cases as operation and argument arrays) is one this has checked: a
        // malformed package is refused here, never by a panic further down.
        super::validation::bank(problem, judge, variant)?;
        let id = string(problem, "id")?;
        if !super::record_id(&id) {
            return Err(invalid("invalid task id"));
        }
        let title = string(variant, "title")?;
        let contract = string(variant, "contract")?;
        let hints: Vec<String> = serde_json::from_value(variant["hints"].clone())
            .map_err(|_| invalid("invalid hint ladder"))?;
        let starters: BTreeMap<String, String> =
            serde_json::from_value(problem["starterCode"].clone())
                .map_err(|_| invalid("invalid starters"))?;
        if !starters.contains_key("python") {
            return Err(invalid("missing Python starter"));
        }

        // Words renamed in prose and labels; the entry and class names are
        // renamed in code too, after them, as the catalog renames them.
        let mut worded = BTreeMap::new();
        let mut rename_order = Vec::new();
        for key in ["terms", "parameters"] {
            if let Some(map) = variant.get(key) {
                let map: BTreeMap<String, String> = serde_json::from_value(map.clone())
                    .map_err(|_| invalid("invalid variant renames"))?;
                for (old, new) in map {
                    rename_order.push((old.clone(), new.clone()));
                    worded.insert(old, new);
                }
            }
        }
        let mut prefixes: BTreeMap<String, String> = variant
            .get("parameters")
            .map(|value| serde_json::from_value(value.clone()).unwrap())
            .unwrap_or_default();
        for key in ["entry", "className"] {
            if let Some(new) = variant.get(key) {
                let old = string(judge, key)?;
                let new = new
                    .as_str()
                    .ok_or_else(|| invalid(format!("invalid {key}")))?
                    .to_owned();
                if key == "className" {
                    prefixes.insert(
                        format!("{}{}", old[..1].to_ascii_lowercase(), &old[1..]),
                        format!("{}{}", new[..1].to_ascii_lowercase(), &new[1..]),
                    );
                }
                rename_order.push((old, new));
            }
        }
        let renames: BTreeMap<_, _> = rename_order.iter().cloned().collect();
        let starters: BTreeMap<_, _> = starters
            .into_iter()
            .map(|(language, code)| (language, rename_starter(&code, &rename_order, &prefixes)))
            .collect();
        let sidecar = match sidecar {
            Some(value) => serde_json::from_value::<Sidecar>(value.clone())
                .map_err(|_| invalid("invalid sidecar fields or types"))?,
            None => Sidecar {
                prompt_version: 1,
                interaction_prompt: default_value("interactionPrompt")
                    .as_str()
                    .unwrap()
                    .to_owned(),
                completion_targets: Vec::new(),
                understanding_checks: serde_json::from_value(default_value("understandingChecks"))
                    .unwrap(),
                required_cases: Vec::new(),
                max_hint_rungs: (default_number("maxHintRungs") as usize).min(hints.len()),
                rubric_profile: "task-engagement-v1".to_owned(),
                languages: default_languages(),
            },
        };
        validate_sidecar(&sidecar, &starters, &hints)?;
        if sidecar.languages.iter().any(|language| language == "c")
            && judge["kind"].as_str() == Some("class")
        {
            return Err(invalid("C runs only function judges"));
        }

        // The starter is the attempt's first revision; one the capture limit
        // refuses could never be run, discussed or kept.
        if sidecar
            .languages
            .iter()
            .any(|language| starters[language].len() > super::session::MAX_CODE_BYTES)
        {
            return Err(invalid("starter exceeds the code byte limit"));
        }
        let cases = judge["cases"]
            .as_array()
            .ok_or_else(|| invalid("invalid judge cases"))?;
        let mut seen = HashSet::new();
        if sidecar.required_cases.len() > 20 {
            return Err(invalid("too many required cases"));
        }
        let mut required_cases = Vec::with_capacity(sidecar.required_cases.len());
        for label in &sidecar.required_cases {
            let matches: Vec<usize> = cases
                .iter()
                .enumerate()
                .filter(|(_, row)| row["label"].as_str() == Some(label))
                .map(|(index, _)| index)
                .collect();
            if !bounded_text(label, 1024) || !seen.insert(label) || matches.len() != 1 {
                return Err(invalid("unresolved, ambiguous or repeated required case"));
            }
            required_cases.push(matches[0]);
        }
        if judge["kind"].as_str() == Some("function")
            && sidecar
                .languages
                .iter()
                .any(|language| super::COMPILED_LANGUAGES.contains(&language.as_str()))
            && !compiled_harness_types(judge, cases)
        {
            return Err(invalid(
                "compiled languages need parameter and return types",
            ));
        }
        let mut public_judge = judge.clone();
        if let Some(entry) = variant.get("entry") {
            public_judge["entry"] = entry.clone();
        }
        if let Some(class_name) = variant.get("className") {
            public_judge.as_object_mut().unwrap().remove("entry");
            public_judge["className"] = class_name.clone();
            for case in public_judge["cases"].as_array_mut().unwrap() {
                for operation in case["input"][0].as_array_mut().unwrap() {
                    if let Some(new) = renames.get(operation.as_str().unwrap()) {
                        *operation = json!(new);
                    }
                }
            }
        }
        if let Some(names) = public_judge
            .get_mut("paramNames")
            .and_then(Value::as_array_mut)
        {
            for name in names {
                if let Some(new) = renames.get(name.as_str().unwrap_or("")) {
                    *name = json!(new);
                }
            }
        }
        for case in public_judge["cases"].as_array_mut().unwrap() {
            case["label"] = Value::String(rename_tokens(&string(case, "label")?, &worded));
        }
        if variant.get("className").is_some() {
            let mut returns = serde_json::Map::new();
            for case in public_judge["cases"].as_array().unwrap() {
                for (operation, expected) in case["input"][0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(case["expected"].as_array().unwrap())
                {
                    let operation = operation.as_str().unwrap();
                    if Some(operation) != public_judge["className"].as_str() {
                        if !expected.is_null() {
                            returns.insert(operation.to_owned(), json!("value"));
                        } else {
                            returns.entry(operation.to_owned()).or_insert(json!("void"));
                        }
                    }
                }
            }
            public_judge["methodReturnTypes"] = Value::Object(returns);
        }
        Ok(Self {
            id,
            title,
            brief: variant["brief"].clone(),
            contract,
            clarifications: variant["clarifications"].clone(),
            starters,
            hints,
            reference_code: problem
                .get("referenceCode")
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("invalid reference code"))
                })
                .transpose()?,
            optimal: rename_tokens(&string(problem, "optimal")?, &worded),
            pitfalls: rename_tokens(&string(problem, "pitfalls")?, &worded),
            judge: public_judge,
            sidecar,
            required_cases,
        })
    }

    pub fn sidecar(&self) -> &Sidecar {
        &self.sidecar
    }
    pub fn judge(&self) -> &Value {
        &self.judge
    }

    /// The required cases as indexes into `judge.cases`. The page marks them
    /// by index: a rename can make a shown label equal another case's.
    pub fn required_cases(&self) -> &[usize] {
        &self.required_cases
    }
}

/// Whether a function judge declares what a C, C++ or Java harness is built
/// from: a type for every argument each case passes, and a return type. The
/// browser languages need neither, so the bank does not always carry them.
fn compiled_harness_types(judge: &Value, cases: &[Value]) -> bool {
    let Some(arity) = judge["paramTypes"].as_array().map(Vec::len) else {
        return false;
    };
    judge["returnType"].is_string()
        && judge
            .get("outputParam")
            .is_none_or(|index| index.as_u64().is_some_and(|index| (index as usize) < arity))
        && cases
            .iter()
            .all(|case| case["input"].as_array().map(Vec::len) == Some(arity))
}

fn string(value: &Value, key: &str) -> Result<String, TaskError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| bounded_text(s, 32768))
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("invalid {key}")))
}

fn validate_sidecar(
    sidecar: &Sidecar,
    starters: &BTreeMap<String, String>,
    hints: &[String],
) -> Result<(), TaskError> {
    if sidecar.prompt_version != 1 || sidecar.rubric_profile != "task-engagement-v1" {
        return Err(invalid("unknown task version or rubric"));
    }
    if !bounded_text(
        &sidecar.interaction_prompt,
        default_number("promptBytes") as usize,
    ) {
        return Err(invalid("invalid instructor prompt size"));
    }
    interpolate(
        &sidecar.interaction_prompt,
        &BTreeMap::from([
            ("title", ""),
            ("language", ""),
            ("target", ""),
            ("phase", ""),
        ]),
    )?;
    if sidecar.max_hint_rungs > hints.len().min(default_number("maxHintRungs") as usize) {
        return Err(invalid("hint maximum exceeds ladder"));
    }
    if sidecar.completion_targets.len() > 8
        || sidecar.understanding_checks.len() != default_number("coreChecks") as usize
    {
        return Err(invalid("invalid target or check count"));
    }
    let mut languages = HashSet::new();
    if sidecar.languages.is_empty()
        || sidecar.languages.iter().any(|language| {
            !super::LANGUAGES.contains(&language.as_str())
                || !languages.insert(language)
                || !starters.contains_key(language)
        })
    {
        return Err(invalid("unknown, repeated or starterless task language"));
    }
    let mut ids = HashSet::new();
    let mut markers = HashSet::new();
    for target in &sidecar.completion_targets {
        if !identifier(&target.id)
            || !ids.insert(&target.id)
            || !languages.contains(&target.language)
            || !bounded_text(&target.goal, 1024)
        {
            return Err(invalid("invalid completion target"));
        }
        let marker = &target.marker;
        if marker.is_empty()
            || marker.len() > 128
            || !marker.as_bytes()[0].is_ascii_uppercase()
            || !marker
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || !markers.insert((&target.language, marker))
            || token_count(&starters[&target.language], marker) != 1
        {
            return Err(invalid("missing, invalid or duplicate completion marker"));
        }
    }
    ids.clear();
    for check in &sidecar.understanding_checks {
        if !identifier(&check.id) || !ids.insert(&check.id) || !bounded_text(&check.question, 1024)
        {
            return Err(invalid("invalid understanding check"));
        }
    }
    Ok(())
}

fn token_count(code: &str, marker: &str) -> usize {
    code.split(|c: char| !is_word(c))
        .filter(|word| *word == marker)
        .count()
}

fn rename_starter(
    code: &str,
    renames: &[(String, String)],
    prefixes: &BTreeMap<String, String>,
) -> String {
    let mut code = code.to_owned();
    for (old, new) in renames {
        code = map_tokens(&code, |token| (token == old).then(|| new.clone()));
    }
    for (old, new) in prefixes {
        code = map_tokens(&code, |token| {
            token
                .strip_prefix(old.as_str())
                .filter(|tail| tail.as_bytes().first().is_some_and(u8::is_ascii_uppercase))
                .map(|tail| format!("{new}{tail}"))
        });
    }
    code
}

fn rename_tokens(code: &str, renames: &BTreeMap<String, String>) -> String {
    map_tokens(code, |token| renames.get(token).cloned())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `code` with each identifier token, a run of alphanumerics and `_`, replaced
/// where `rename` names a replacement; everything between tokens is kept.
fn map_tokens(code: &str, mut rename: impl FnMut(&str) -> Option<String>) -> String {
    let mut output = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(start) = rest.find(is_word) {
        let (gap, word) = rest.split_at(start);

        // `word` opens with a word character, so `tail` is always shorter and
        // the loop cannot stall on an empty token.
        let tail = word.trim_start_matches(is_word);
        let token = &word[..word.len() - tail.len()];
        output.push_str(gap);
        match rename(token) {
            Some(renamed) => output.push_str(&renamed),
            None => output.push_str(token),
        }
        rest = tail;
    }
    output.push_str(rest);
    output
}

fn interpolate(prompt: &str, values: &BTreeMap<&str, &str>) -> Result<String, TaskError> {
    let mut rest = prompt;
    let mut output = String::new();
    while let Some(at) = rest.find("{{") {
        if rest[..at].contains("}}") {
            return Err(invalid("malformed prompt variable"));
        }
        if rest[at..].starts_with("{{{{") {
            return Err(invalid("malformed prompt variable"));
        }
        // A third brace is literal text; the variable starts after it.
        if rest[at..].starts_with("{{{") {
            let (text, brace) = rest.split_at(at);
            output.push_str(text);
            output.push('{');
            rest = &brace[1..];
            continue;
        }
        output.push_str(&rest[..at]);
        rest = &rest[at + 2..];
        let end = rest
            .find("}}")
            .ok_or_else(|| invalid("malformed prompt variable"))?;
        let name = &rest[..end];
        output.push_str(
            values
                .get(name)
                .ok_or_else(|| invalid("unknown prompt variable"))?,
        );
        rest = &rest[end + 2..];
    }
    if rest.contains("}}") {
        return Err(invalid("malformed prompt variable"));
    }
    output.push_str(rest);
    Ok(output)
}

#[cfg(test)]
#[path = "../../tests/unit/tasks/exercise.rs"]
mod tests;
