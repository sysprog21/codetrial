//! Properties of what the defense reads from a student's diff and trace.

use std::collections::BTreeSet;

use super::select::git_name;
use super::*;

fn numbered(files: &[u8], per_file: u8) -> Vec<Added> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| Added {
            file: format!("{}.scala", file % per_file),
            line: index + 1,
            text: format!("line {index}"),
        })
        .collect()
}

/// One answer for a candidate set: its size and members, a line to a line, a
/// different file each time while one is untaken, a prefix of the next
/// answer, and the same whatever order the candidates arrive in (each key is
/// a function of its own file and line). The generator's default size gives
/// seven candidates at most in 128 cases, so this one asks for 8 to 64.
#[ephyra::test(tests = 128, seed = 1)]
fn selection_is_diverse_a_prefix_chain_and_ignores_input_order(
    #[ephyra(gen = ephyra::gens::vec(ephyra::gens::int(0u8..=255)).len(8..=64))] files: Vec<u8>,
    count: u8,
    turn: usize,
) {
    let candidates = numbered(&files, 8);
    let count = usize::from(count % 70);
    let choose = |candidates: &[Added], count| {
        choose(b"test salt", "TEST123", "test head", candidates, count)
    };
    let now = choose(&candidates, count);
    assert_eq!(now.len(), count.min(candidates.len()));
    assert!(now.iter().all(|line| candidates.contains(line)));
    let lines: BTreeSet<_> = now.iter().map(|line| line.line).collect();
    assert_eq!(lines.len(), now.len());
    let files_available: BTreeSet<_> = candidates.iter().map(|line| &line.file).collect();
    let diverse: BTreeSet<_> = now
        .iter()
        .take(files_available.len())
        .map(|line| &line.file)
        .collect();
    assert_eq!(diverse.len(), now.len().min(files_available.len()));
    let more = choose(&candidates, count + 1);
    assert_eq!(more[..now.len()], now[..]);
    let mut turned = candidates;
    let by = turn % turned.len();
    turned.rotate_left(by);
    turned.reverse();
    assert_eq!(choose(&turned, count), now);
}

#[test]
fn selection_depends_on_salt_student_and_head() {
    let candidates = numbered(&(0..64).map(|index| index % 4).collect::<Vec<u8>>(), 4);
    let base = choose(b"test salt", "TEST123", "test head", &candidates, 8);
    assert_ne!(
        base,
        choose(b"other salt", "TEST123", "test head", &candidates, 8)
    );
    assert_ne!(
        base,
        choose(b"test salt", "OTHER99", "test head", &candidates, 8)
    );
    assert_ne!(
        base,
        choose(b"test salt", "TEST123", "other head", &candidates, 8)
    );
}

/// A name as Git prints it after a prefix: as it is, or, holding a quote, a
/// backslash or a control character, in double quotes with C escapes.
fn spelled(prefix: &str, path: &str) -> String {
    if !path
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_control())
    {
        return format!("{prefix}{path}");
    }
    let mut out = format!("\"{prefix}");
    for c in path.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\{:03o}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What follows `+++ ` for a name: its spelling, and a tab, when the name has
/// a space, quoted or not.
fn plus_name(path: &str) -> String {
    let mut name = spelled("b/", path);
    if path.contains(' ') {
        name.push('\t');
    }
    name
}

/// A hunk is its lines: `true` adds one, `false` deletes one, each with a text.
type Hunk = Vec<(bool, u8)>;
/// A file's kind byte, the bytes of its name and its hunks.
type DiffFile = (u8, Vec<u8>, Vec<Hunk>);

/// Names with a space, a space at either end, no space, several directories,
/// multibyte letters, and the characters Git quotes (a quote, a backslash, a
/// tab, a control character).
fn generated_path(index: usize, bytes: &[u8]) -> String {
    const ALPHABET: &[char] = &[
        'a', 'b', 'z', 'A', '0', '9', ' ', ' ', '-', '_', '.', 'α', '😀', '+', '@', '/', '"', '\\',
        '\t', '\u{1}', '\u{7f}',
    ];
    let stem: String = bytes
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()])
        .collect();
    let directories: Vec<&str> = stem
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect();
    format!("d{index}/{}x.scala", directories.join("/"))
}

/// One generated file of a diff and what `added_lines` must report for it.
fn render_file(index: usize, file: &DiffFile) -> (String, Vec<Added>) {
    const TEXTS: [&str; 7] = [
        "",
        "x",
        "++ plus",
        "@@ -1 +1 @@",
        "diff --git a/q b/q",
        "carriage\r",
        "α 😀",
    ];
    let (kind, name, hunks) = file;
    let path = generated_path(index, name);
    let (old, new) = (spelled("a/", &path), spelled("b/", &path));
    let mut text = format!("diff --git {old} {new}\n");
    let mut expected = Vec::new();
    let mut added = |text: &mut String, line: usize, body: &str| {
        text.push_str(&format!("+{body}\n"));
        expected.push(Added {
            file: path.clone(),
            line,
            text: body.to_owned(),
        });
    };
    match kind % 5 {
        0 => {
            text.push_str(&format!(
                "index 111..222 100644\n--- {old}\n+++ {}\n",
                plus_name(&path)
            ));
            let mut cursor = 1;
            for hunk in hunks {
                let adds = hunk.iter().filter(|(add, _)| *add).count();
                let dels = hunk.len() - adds;
                text.push_str(&format!("@@ -{cursor},{dels} +{cursor},{adds} @@\n"));
                let mut line = cursor;
                for (add, body) in hunk {
                    let body = TEXTS[usize::from(*body) % TEXTS.len()];
                    if *add {
                        added(&mut text, line, body);
                        line += 1;
                    } else {
                        text.push_str(&format!("-{body}\n"));
                    }
                }
                cursor = line + 2;
            }
        }
        1 => {
            let bodies: Vec<&str> = hunks
                .iter()
                .flatten()
                .filter(|(add, _)| *add)
                .map(|(_, body)| TEXTS[usize::from(*body) % TEXTS.len()])
                .collect();
            text.push_str("new file mode 100644\n");
            if !bodies.is_empty() {
                text.push_str(&format!(
                    "--- /dev/null\n+++ {}\n@@ -0,0 +1,{} @@\n",
                    plus_name(&path),
                    bodies.len()
                ));
            }
            for (at, body) in bodies.iter().enumerate() {
                added(&mut text, at + 1, body);
            }
        }
        2 => text.push_str(&format!(
            "deleted file mode 100644\n--- {old}\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-gone one\n-gone two\n"
        )),
        3 => text.push_str(&format!("Binary files {old} and {new} differ\n")),
        _ => text.push_str(&format!(
            "similarity index 100%\nrename from {path}\nrename to {path}.old\n"
        )),
    }
    (text, expected)
}

/// The reader takes exactly the added lines of the modified and new files, at
/// their new line numbers, under the name the file has however Git spelled
/// it, whatever else the diff holds: deletions, binary and renamed files,
/// several hunks, added lines that look like headers.
#[ephyra::test(tests = 256, seed = 2, max_size = 8)]
fn added_lines_match_a_model_of_the_diff(files: Vec<DiffFile>) {
    let mut diff = String::new();
    let mut expected = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let (text, added) = render_file(index, file);
        diff.push_str(&text);
        expected.extend(added);
        let path = generated_path(index, &file.1);
        assert_eq!(
            git_name(&spelled("", &path)).as_deref(),
            Some(path.as_str())
        );
    }
    assert_eq!(added_lines(&diff), expected);
}

/// The bytes the observation writer produces for these records, claiming
/// `claimed` of them in the trailer.
fn trace_bytes(first: u16, values: &[(u32, u16, u8)], claimed: usize) -> Vec<u8> {
    let header = serde_json::json!({
        "first": first,
        "record_bytes": 7,
        "fields": [
            {"name": "core.pc", "offset": 0, "bytes": 4},
            {"name": "core.half", "offset": 4, "bytes": 2},
            {"name": "core.byte", "offset": 6, "bytes": 1}
        ]
    });
    let trailer = serde_json::json!({"records": claimed, "end": {"how": "exit=0"}}).to_string();
    let mut trace = header.to_string().into_bytes();
    trace.push(b'\n');
    for (word, half, byte) in values {
        trace.extend(word.to_le_bytes());
        trace.extend(half.to_le_bytes());
        trace.push(*byte);
    }
    trace.extend(trailer.as_bytes());
    trace.extend(
        u64::try_from(trailer.len())
            .expect("trailer length fits u64")
            .to_le_bytes(),
    );
    trace.extend(b"OBSEND01");
    trace
}

/// Every record reads back as written. A trace cut anywhere, or one whose
/// trailer counts a record too many or too few, is refused; a flipped byte is
/// refused or read, never a panic.
#[ephyra::test(tests = 256, seed = 3, max_size = 32)]
fn a_trace_reads_back_and_a_damaged_one_is_refused(
    first: u16,
    values: Vec<(u32, u16, u8)>,
    cut: usize,
    at: usize,
    flip: u8,
) {
    let trace = trace_bytes(first, &values, values.len());
    let table = read_trace(&trace).expect("valid trace");
    assert_eq!(table.first, u64::from(first));
    assert_eq!(table.fields, ["core.pc", "core.half", "core.byte"]);
    assert_eq!(
        table.rows,
        values
            .iter()
            .map(|&(word, half, byte)| vec![word, u32::from(half), u32::from(byte)])
            .collect::<Vec<_>>()
    );
    assert_eq!(table.end["how"], "exit=0");

    let cut = cut % trace.len();
    assert!(read_trace(&trace[..cut]).is_err(), "cut {cut}");
    for claimed in [values.len() + 1, values.len().wrapping_sub(1)] {
        assert!(
            read_trace(&trace_bytes(first, &values, claimed)).is_err(),
            "claimed {claimed} of {}",
            values.len()
        );
    }
    let at = at % trace.len();
    let mut flipped = trace;
    flipped[at] ^= flip | 1;
    let _ = read_trace(&flipped);
}

#[ephyra::test(tests = 256, seed = 4)]
fn decimal_literal_change_increments_only_the_literal(value: u32, padding: u8) {
    let literal = format!("{}{value}.U", "0".repeat(usize::from(padding % 8)));
    let line = format!("α := ({literal})");
    let expected = format!("α := ({}.U)", u64::from(value) + 1);
    assert_eq!(change_of(&line).changed.as_deref(), Some(expected.as_str()));
}

/// A line is deleted exactly when none of the course's patterns occurs in it.
/// Every piece but the last two is a pattern or holds one, and a space keeps a
/// neighbour from hiding it (a decimal needs a word boundary): a rule dropped
/// from `change_of` turns a line of that piece into a deletion.
#[ephyra::test(tests = 256, seed = 13, max_size = 40)]
fn a_line_is_deleted_only_when_no_rule_applies(codes: Vec<u16>) {
    const PIECES: [&str; 12] = [
        "&&", "||", "=/=", "===", "<=", "<", "!", "0.U", "9.U", "+", "α", " ",
    ];
    let pieces: Vec<&str> = codes
        .iter()
        .map(|code| PIECES[usize::from(*code) % PIECES.len()])
        .collect();
    let line = pieces.join(" ");
    let rule_applies = pieces.iter().any(|piece| !["α", " "].contains(piece));
    let change = change_of(&line);
    assert_eq!(change.changed.is_some(), rule_applies, "{line}");
    assert_eq!(change.what == "delete the line", !rule_applies, "{line}");
}
