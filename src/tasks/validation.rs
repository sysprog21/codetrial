//! Structural checks before an authenticated downloaded record is used.

use serde_json::Value;

use super::{TaskError, bounded_text, invalid};

pub(super) fn fields(value: &Value, required: &[&str], optional: &[&str]) -> Result<(), TaskError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("expected object"))?;
    if required.iter().any(|key| !object.contains_key(*key))
        || object
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(invalid("missing or unknown bank fields"));
    }
    Ok(())
}

fn text(value: &Value) -> Result<(), TaskError> {
    if value
        .as_str()
        .is_none_or(|value| !bounded_text(value, 32768))
    {
        return Err(invalid("expected nonblank bank text"));
    }
    Ok(())
}

fn texts(value: &Value, min: usize, max: usize) -> Result<(), TaskError> {
    let list = value
        .as_array()
        .ok_or_else(|| invalid("expected text list"))?;
    if !(min..=max).contains(&list.len()) {
        return Err(invalid("invalid bank list size"));
    }
    for item in list {
        text(item)?;
    }
    Ok(())
}

fn name(value: &Value) -> Result<&str, TaskError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid("invalid identifier"))?;
    if text.is_empty()
        || !text.as_bytes()[0].is_ascii_alphabetic()
        || !text.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(invalid("invalid identifier"));
    }
    Ok(text)
}

pub(super) fn bank(problem: &Value, judge: &Value, variant: &Value) -> Result<(), TaskError> {
    if !problem.is_object() {
        return Err(invalid("invalid problem"));
    }
    if let Some(reference) = problem.get("referenceCode") {
        text(reference)?;
    }
    for key in ["summary", "optimal", "pitfalls"] {
        text(&problem[key])?;
    }
    if !matches!(
        problem["difficulty"].as_str(),
        Some("Easy" | "Medium" | "Hard")
    ) {
        return Err(invalid("invalid difficulty"));
    }
    texts(&problem["topics"], 1, 8)?;
    let topics = problem["topics"].as_array().unwrap();
    let unique: std::collections::HashSet<_> =
        topics.iter().map(|v| v.as_str().unwrap().trim()).collect();
    if unique.len() != topics.len() {
        return Err(invalid("duplicate topic"));
    }
    if problem
        .get("origin")
        .is_some_and(|value| !value.is_string())
    {
        return Err(invalid("invalid problem origin"));
    }
    match problem
        .get("origin")
        .and_then(Value::as_str)
        .unwrap_or("leetcode")
    {
        "original"
            if !problem.as_object().unwrap().contains_key("title")
                && !problem.as_object().unwrap().contains_key("examples") => {}
        "leetcode" => {
            text(&problem["title"])?;
            if !problem["examples"].is_array() {
                return Err(invalid("missing imported examples"));
            }
        }
        _ => return Err(invalid("invalid problem origin")),
    }
    let starters = problem["starterCode"]
        .as_object()
        .ok_or_else(|| invalid("invalid starters"))?;
    for (language, starter) in starters {
        if !matches!(
            language.as_str(),
            "python" | "javascript" | "c" | "cpp" | "java"
        ) {
            return Err(invalid("unsupported starter language"));
        }
        text(starter)?;
    }
    fields(
        variant,
        &[
            "title",
            "brief",
            "contract",
            "examples",
            "clarifications",
            "followUps",
            "hints",
        ],
        &["entry", "className", "parameters", "terms"],
    )?;
    text(&variant["title"])?;
    text(&variant["contract"])?;
    texts(&variant["brief"], 1, 3)?;
    texts(&variant["followUps"], 2, 3)?;
    texts(&variant["hints"], 3, 3)?;
    let clarifications = variant["clarifications"]
        .as_array()
        .ok_or_else(|| invalid("invalid clarifications"))?;
    if !(3..=6).contains(&clarifications.len()) {
        return Err(invalid("invalid clarification count"));
    }
    for row in clarifications {
        fields(row, &["question", "answer"], &[])?;
        text(&row["question"])?;
        text(&row["answer"])?;
    }
    fields(
        judge,
        &["kind", "checker", "cases"],
        &[
            "entry",
            "className",
            "paramNames",
            "paramTypes",
            "returnType",
            "argTypes",
            "outputParam",
            "outputType",
            "outputPrefixParam",
            "cyclePosParam",
            "constructorArgTypes",
        ],
    )?;
    if let Some(names) = judge.get("paramNames") {
        let names = names
            .as_array()
            .ok_or_else(|| invalid("invalid parameter names"))?;
        let mut seen = std::collections::BTreeSet::new();
        for item in names {
            let item = name(item)?;
            if !seen.insert(item) {
                return Err(invalid("duplicate parameter names"));
            }
        }
    }
    for key in ["paramTypes", "argTypes", "constructorArgTypes"] {
        if let Some(types) = judge.get(key) {
            for item in types
                .as_array()
                .ok_or_else(|| invalid("invalid judge type list"))?
            {
                if key == "argTypes" && item.is_null() {
                    continue;
                }
                text(item)?;
            }
        }
    }
    for key in ["returnType", "outputType"] {
        if let Some(value) = judge.get(key) {
            text(value)?;
        }
    }
    for key in ["outputParam", "outputPrefixParam", "cyclePosParam"] {
        if judge.get(key).is_some_and(|value| value.as_u64().is_none()) {
            return Err(invalid("invalid judge parameter index"));
        }
    }
    if !matches!(
        judge["checker"].as_str(),
        Some(
            "exact"
                | "approxNumber"
                | "arrayBag"
                | "balancedBst"
                | "dependencyOrder"
                | "indexPair"
                | "integerCombinations"
                | "integerRows"
                | "palindrome"
                | "tripletSet"
                | "unorderedGroups"
        )
    ) {
        return Err(invalid("unsupported practice checker"));
    }
    let declared = match judge["kind"].as_str() {
        Some("function") => "entry",
        Some("class") => "className",
        _ => return Err(invalid("unsupported judge kind")),
    };
    let source_name = name(&judge[declared])?;
    let new_name = name(&variant[declared])?;
    if !new_name.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return Err(invalid("invalid variant name"));
    }
    let wrong = if declared == "entry" {
        "className"
    } else {
        "entry"
    };
    if variant.get(wrong).is_some()
        || (declared == "entry" && judge.get(wrong).is_some())
        || source_name.eq_ignore_ascii_case(new_name)
        || (declared == "entry" && !new_name.as_bytes()[0].is_ascii_lowercase())
        || (declared == "className" && !new_name.as_bytes()[0].is_ascii_uppercase())
    {
        return Err(invalid("invalid variant entry rename"));
    }
    if declared == "className"
        && let Some(entry) = judge.get("entry")
    {
        name(entry)?;
    }
    for key in ["terms", "parameters"] {
        if let Some(value) = variant.get(key) {
            let map = value
                .as_object()
                .ok_or_else(|| invalid("invalid rename map"))?;
            for (old, new) in map {
                name(&Value::String(old.clone()))?;
                name(new)?;
                if key == "terms"
                    && (!old.bytes().all(|c| c.is_ascii_alphanumeric())
                        || !new
                            .as_str()
                            .unwrap()
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric()))
                {
                    return Err(invalid("invalid term rename"));
                }
                if key == "parameters"
                    && !judge["paramNames"]
                        .as_array()
                        .is_some_and(|names| names.contains(&Value::String(old.clone())))
                {
                    return Err(invalid("unknown parameter rename"));
                }
            }
        }
    }
    let mut renames = std::collections::BTreeMap::new();
    for key in ["terms", "parameters"] {
        if let Some(map) = variant.get(key).and_then(Value::as_object) {
            for (old, new) in map {
                if renames
                    .insert(old.as_str(), new.as_str().unwrap())
                    .is_some()
                {
                    return Err(invalid("overlapping rename keys"));
                }
            }
        }
    }
    if renames.insert(source_name, new_name).is_some() {
        return Err(invalid("rename key repeats source name"));
    }
    if renames.values().any(|value| renames.contains_key(value)) {
        return Err(invalid("chained rename is ambiguous"));
    }
    if renames
        .values()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != renames.len()
    {
        return Err(invalid("duplicate rename output"));
    }
    if let Some(names) = judge.get("paramNames").and_then(Value::as_array) {
        let posed: Vec<_> = names
            .iter()
            .map(|name| {
                let name = name.as_str().unwrap();
                renames.get(name).copied().unwrap_or(name)
            })
            .collect();
        if posed
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != posed.len()
        {
            return Err(invalid("parameter rename collides"));
        }
    }
    let mut prefixes: Vec<String> = variant
        .get("parameters")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    let mut outputs: Vec<String> = renames.values().map(|value| (*value).to_owned()).collect();
    if declared == "className" {
        let class_prefix = format!(
            "{}{}",
            source_name[..1].to_ascii_lowercase(),
            &source_name[1..]
        );
        if prefixes.contains(&class_prefix) {
            return Err(invalid("duplicate prefix rename key"));
        }
        prefixes.push(class_prefix);
        outputs.push(format!(
            "{}{}",
            new_name[..1].to_ascii_lowercase(),
            &new_name[1..]
        ));
    }
    if prefixes.iter().any(|key| outputs.contains(key)) {
        return Err(invalid("chained prefix rename"));
    }
    for prefix in &prefixes {
        if outputs.iter().any(|output| {
            prefix
                .strip_prefix(output)
                .is_some_and(|tail| tail.as_bytes().first().is_some_and(u8::is_ascii_uppercase))
        }) {
            return Err(invalid("ambiguous reverse prefix rename"));
        }
        if outputs.iter().chain(prefixes.iter()).any(|value| {
            value
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.as_bytes().first().is_some_and(u8::is_ascii_uppercase))
        }) {
            return Err(invalid("ambiguous prefix rename"));
        }
    }
    let cases = judge["cases"]
        .as_array()
        .filter(|cases| !cases.is_empty() && cases.len() <= 1000)
        .ok_or_else(|| invalid("invalid judge cases"))?;
    for case in cases {
        fields(case, &["label", "input", "expected"], &[])?;
        text(&case["label"])?;
        if !case["input"].is_array() {
            return Err(invalid("invalid case input"));
        }
        if declared == "className" {
            let input = case["input"].as_array().unwrap();
            if input.len() != 2 {
                return Err(invalid("invalid class input"));
            }
            let operations = input[0]
                .as_array()
                .ok_or_else(|| invalid("invalid operations"))?;
            let arguments = input[1]
                .as_array()
                .ok_or_else(|| invalid("invalid arguments"))?;
            let expected = case["expected"]
                .as_array()
                .ok_or_else(|| invalid("invalid class expectations"))?;
            if operations.is_empty()
                || operations.len() != arguments.len()
                || expected.len() != operations.len()
                || operations[0].as_str() != Some(source_name)
            {
                return Err(invalid("invalid class case lengths"));
            }
            for operation in operations {
                name(operation)?;
            }
            if arguments.iter().any(|row| !row.is_array()) {
                return Err(invalid("invalid class arguments"));
            }
        }
    }
    let examples = variant["examples"]
        .as_array()
        .filter(|rows| (1..=2).contains(&rows.len()))
        .ok_or_else(|| invalid("invalid examples"))?;
    for row in examples {
        fields(row, &["case"], &["output", "explanation"])?;
        if row["case"]
            .as_u64()
            .is_none_or(|at| at >= cases.len() as u64)
        {
            return Err(invalid("unresolved example case"));
        }
        for key in ["output", "explanation"] {
            if let Some(value) = row.get(key) {
                text(value)?;
            }
        }
    }
    Ok(())
}
