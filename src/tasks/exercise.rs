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

    pub fn hints(&self) -> Vec<&str> {
        self.0
            .hints
            .iter()
            .take(self.0.sidecar.max_hint_rungs)
            .map(String::as_str)
            .collect()
    }

    pub fn understanding_checks(&self) -> &[UnderstandingCheck] {
        &self.0.sidecar.understanding_checks
    }

    /// The marked code a task asks the learner to complete.
    pub fn completion_targets(&self) -> &[CompletionTarget] {
        &self.0.sidecar.completion_targets
    }

    /// The question of the check named `id`.
    pub fn check_question(&self, id: &str) -> Option<&str> {
        self.understanding_checks()
            .iter()
            .find(|check| check.id == id)
            .map(|check| check.question.as_str())
    }

    pub fn reference_code(&self) -> Option<&str> {
        self.0.reference_code.as_deref()
    }

    pub fn reference_notes(&self) -> (&str, &str) {
        (&self.0.optimal, &self.0.pitfalls)
    }

    /// Constructed by field selection, so new private bank fields cannot leak.
    pub fn live_projection(&self) -> Value {
        let record = &self.0;
        json!({"title": record.title, "brief": record.brief,
            "contract": record.contract, "clarifications": record.clarifications,
            "hints": self.hints(), "completionTargets": record.sidecar.completion_targets,
            "understandingChecks": record.sidecar.understanding_checks})
    }

    pub fn task_prompt(
        &self,
        phase: Phase,
        target: Option<&str>,
        code: &str,
    ) -> Result<String, TaskError> {
        self.task_prompt_with_hint_limit(phase, target, code, self.hints().len())
    }

    pub fn task_prompt_with_hint_limit(
        &self,
        phase: Phase,
        target: Option<&str>,
        code: &str,
        hint_limit: usize,
    ) -> Result<String, TaskError> {
        let record = &self.0;
        let instructor = record.sidecar.interaction_prompt.clone();
        let target_goal = match target {
            Some(id) => record
                .sidecar
                .completion_targets
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.goal.as_str())
                .ok_or_else(|| invalid("unknown completion target"))?,
            None => "",
        };
        let instructor = interpolate(
            &instructor,
            &BTreeMap::from([
                ("title", self.title()),
                ("language", "python"),
                ("target", target_goal),
                ("phase", phase.as_str()),
            ]),
        )?;
        let mut projection = self.live_projection();
        projection.as_object_mut().unwrap().remove("hints");
        projection["hintRungsMax"] = json!(hint_limit.min(self.hints().len()));
        let projection = projection.to_string();
        interpolate(
            include_str!("../../problem-bank/task-prompt.txt"),
            &BTreeMap::from([
                ("phase", phase.as_str()),
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
    required_case_indices: Vec<usize>,
    sidecar: Sidecar,
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
        let mut renames = BTreeMap::new();
        let mut rename_order = Vec::new();
        for key in ["terms", "parameters"] {
            if let Some(map) = variant.get(key) {
                let map: BTreeMap<String, String> = serde_json::from_value(map.clone())
                    .map_err(|_| invalid("invalid variant renames"))?;
                for (old, new) in map {
                    rename_order.push((old.clone(), new.clone()));
                    renames.insert(old, new);
                }
            }
        }
        if let Some(entry) = variant.get("entry") {
            renames.insert(
                string(judge, "entry")?,
                entry
                    .as_str()
                    .ok_or_else(|| invalid("invalid entry"))?
                    .to_owned(),
            );
        }
        if let Some(class_name) = variant.get("className") {
            renames.insert(
                string(judge, "className")?,
                class_name
                    .as_str()
                    .ok_or_else(|| invalid("invalid className"))?
                    .to_owned(),
            );
        }
        let worded: BTreeMap<String, String> = ["terms", "parameters"]
            .into_iter()
            .flat_map(|key| {
                variant
                    .get(key)
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|map| {
                        map.iter()
                            .map(|(old, new)| (old.clone(), new.as_str().unwrap().to_owned()))
                    })
            })
            .collect();
        if let Some(entry) = variant.get("entry") {
            rename_order.push((string(judge, "entry")?, entry.as_str().unwrap().to_owned()));
        }
        if let Some(class_name) = variant.get("className") {
            rename_order.push((
                string(judge, "className")?,
                class_name.as_str().unwrap().to_owned(),
            ));
        }
        let mut prefixes: BTreeMap<String, String> = variant
            .get("parameters")
            .map(|value| serde_json::from_value(value.clone()).unwrap())
            .unwrap_or_default();
        if let Some(class_name) = variant.get("className") {
            let old = string(judge, "className")?;
            let new = class_name.as_str().unwrap();
            prefixes.insert(
                format!("{}{}", old[..1].to_ascii_lowercase(), &old[1..]),
                format!("{}{}", new[..1].to_ascii_lowercase(), &new[1..]),
            );
        }
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
            },
        };
        validate_sidecar(&sidecar, &starters, &hints)?;

        // The starter is the attempt's first revision; one the capture limit
        // refuses could never be run, discussed or kept.
        if starters
            .get("python")
            .is_some_and(|code| code.len() > super::session::MAX_CODE_BYTES)
        {
            return Err(invalid("starter exceeds the code byte limit"));
        }
        let cases = judge["cases"]
            .as_array()
            .ok_or_else(|| invalid("invalid judge cases"))?;
        let mut required_case_indices = Vec::new();
        let mut seen = HashSet::new();
        if sidecar.required_cases.len() > 20 {
            return Err(invalid("too many required cases"));
        }
        for label in &sidecar.required_cases {
            let matches: Vec<_> = cases
                .iter()
                .enumerate()
                .filter(|(_, row)| row["label"].as_str() == Some(label))
                .map(|(index, _)| index)
                .collect();
            if !bounded_text(label, 1024) || !seen.insert(label) || matches.len() != 1 {
                return Err(invalid("unresolved, ambiguous or repeated required case"));
            }
            required_case_indices.push(matches[0]);
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
        let label_renames = worded.clone();
        for case in public_judge["cases"].as_array_mut().unwrap() {
            case["label"] = Value::String(rename_tokens(&string(case, "label")?, &label_renames));
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
            required_case_indices,
            sidecar,
        })
    }

    pub fn sidecar(&self) -> &Sidecar {
        &self.sidecar
    }
    pub fn judge(&self) -> &Value {
        &self.judge
    }
    pub fn required_case_indices(&self) -> &[usize] {
        &self.required_case_indices
    }
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
    let mut ids = HashSet::new();
    let mut markers = HashSet::new();
    for target in &sidecar.completion_targets {
        if !identifier(&target.id)
            || !ids.insert(&target.id)
            || target.language != "python"
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
            || !markers.insert(marker)
            || token_count(&starters["python"], marker) != 1
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
    code.split(|c: char| !c.is_alphanumeric() && c != '_')
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
        code = rename_tokens(&code, &BTreeMap::from([(old.clone(), new.clone())]));
    }
    for (old, new) in prefixes {
        let mut output = String::new();
        let mut token = String::new();
        for c in code.chars().chain(std::iter::once('\0')) {
            if c.is_alphanumeric() || c == '_' {
                token.push(c);
            } else {
                if token
                    .strip_prefix(old)
                    .is_some_and(|tail| tail.as_bytes().first().is_some_and(u8::is_ascii_uppercase))
                {
                    output.push_str(new);
                    output.push_str(&token[old.len()..]);
                } else {
                    output.push_str(&token);
                }
                token.clear();
                if c != '\0' {
                    output.push(c);
                }
            }
        }
        code = output;
    }
    code
}

fn rename_tokens(code: &str, renames: &BTreeMap<String, String>) -> String {
    let mut output = String::new();
    let mut token = String::new();
    for c in code.chars().chain(std::iter::once('\0')) {
        if c.is_alphanumeric() || c == '_' {
            token.push(c);
        } else {
            output.push_str(renames.get(&token).map(String::as_str).unwrap_or(&token));
            token.clear();
            if c != '\0' {
                output.push(c);
            }
        }
    }
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
