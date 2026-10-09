use super::*;
use crate::tasks::test_support::{PIN, download as fixture};
use serde_json::json;

#[test]
fn a_python_built_package_opens_with_its_pin_and_nothing_else() {
    let fixture = fixture();
    let open = |set: &str, version, manifest: &[u8], cipher: &[u8], pin| {
        open_package(set, version, manifest, cipher.to_vec(), pin)
    };
    let set = open("classroom", 7, &fixture.manifest, &fixture.ciphertext, PIN).unwrap();
    assert_eq!(set.records.len(), 2);
    assert_eq!(set.config.rule("durationMin"), 15);
    assert_eq!(set.config.rule("lookAwaySeconds"), 8);

    assert_eq!(
        open(
            "classroom",
            7,
            &fixture.manifest,
            &fixture.ciphertext,
            "654321"
        )
        .err(),
        Some(OpenError::WrongPin)
    );
    assert_eq!(
        open(
            "classroom",
            7,
            &fixture.manifest,
            &fixture.ciphertext,
            "12a456"
        )
        .err(),
        Some(OpenError::WrongPin)
    );
    // A damaged download fails its hash before any key is derived.
    let mut corrupt = fixture.ciphertext.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(matches!(
        open("classroom", 7, &fixture.manifest, &corrupt, PIN),
        Err(OpenError::Invalid(_))
    ));

    // Refused by the manifest, before a key derived for the wrong set or
    // version could fail to decrypt it anyway.
    for (set, version) in [("other", 7), ("classroom", 8)] {
        assert_eq!(
            open(set, version, &fixture.manifest, &fixture.ciphertext, PIN).err(),
            Some(OpenError::Invalid(invalid(
                "invalid manifest or ciphertext"
            )))
        );
    }
}

#[test]
fn a_manifest_may_fill_its_size_limit_and_no_more() {
    let fixture = fixture();
    let padded = |size: usize| {
        let mut manifest = fixture.manifest.clone();
        manifest.resize(size, b' ');
        manifest
    };
    assert!(
        open_package(
            "classroom",
            7,
            &padded(MAX_MANIFEST_BYTES),
            fixture.ciphertext.clone(),
            PIN
        )
        .is_ok()
    );
    assert_eq!(
        open_package(
            "classroom",
            7,
            &padded(MAX_MANIFEST_BYTES + 1),
            fixture.ciphertext.clone(),
            PIN
        )
        .err(),
        Some(OpenError::Invalid(invalid("manifest exceeds size limit")))
    );
}

#[test]
fn the_salt_is_bound_into_the_key() {
    let fixture = fixture();
    let mut manifest: Value = serde_json::from_slice(&fixture.manifest).unwrap();
    manifest["salt"] = Value::from("AAAAAAAAAAAAAAAAAAAAAA==");
    let manifest = serde_json::to_vec(&manifest).unwrap();
    assert_eq!(
        open_package("classroom", 7, &manifest, fixture.ciphertext, PIN).err(),
        Some(OpenError::WrongPin)
    );
}

#[test]
fn a_site_is_an_https_base_that_sets_join_under() {
    assert_eq!(
        site_base("https://teacher.github.io/course")
            .unwrap()
            .as_str(),
        "https://teacher.github.io/course/"
    );
    assert_eq!(
        site_base("https://teacher.github.io/course/")
            .unwrap()
            .join("sets/classroom/7/manifest.json")
            .unwrap()
            .as_str(),
        "https://teacher.github.io/course/sets/classroom/7/manifest.json"
    );
    for bad in [
        "http://teacher.github.io/course",
        "https://user:pw@teacher.github.io",
        // Either half of a credential alone is still one.
        "https://user@teacher.github.io",
        "https://:pw@teacher.github.io",
        "https://teacher.github.io/course?x=1",
        "https://teacher.github.io/course#top",
        "teacher.github.io",
    ] {
        assert!(site_base(bad).is_err(), "{bad}");
    }
    assert!(parse_close("2026-10-06T12:00:00+00:00").is_err());
    assert!(parse_close("2026-10-06T12:00:00Z").is_ok());
    // The packaging tool writes whole seconds only; both sides take one form.
    assert!(parse_close("2026-10-06T12:00:00.5Z").is_err());
    assert!(parse_close("2026-02-31T12:00:00Z").is_err());

    // RFC 3339 also takes these, at the length of the one form; the tool writes
    // neither, so neither is read.
    for other in [
        "2026-10-06t12:00:00Z",
        "2026-10-06 12:00:00Z",
        "2026-10-06T12:00:00z",
    ] {
        assert_eq!(
            parse_close(other).unwrap_err().0,
            "close date must be UTC to the second",
            "{other}"
        );
    }
}

/// Serves fixed responses by path until dropped, with the `Date` header a
/// static host sends.
async fn routed_fixture(
    routes: Vec<(&'static str, u16, Vec<u8>, Option<String>)>,
) -> (reqwest::Url, tokio::task::JoinHandle<()>) {
    routed_fixture_sized(routes, true).await
}

/// `routed_fixture`, optionally sending each body with no declared length,
/// ended by closing the connection, as a host streaming a response may.
async fn routed_fixture_sized(
    routes: Vec<(&'static str, u16, Vec<u8>, Option<String>)>,
    sized: bool,
) -> (reqwest::Url, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = reqwest::Url::parse(&format!(
        "http://{}/course/",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0; 8192];
            let read = stream.read(&mut request).await.unwrap_or(0);
            let line = String::from_utf8_lossy(&request[..read]);
            let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            let (status, body, location) = routes
                .iter()
                .find(|(route, ..)| *route == path)
                .map(|(_, status, body, location)| (*status, body.clone(), location.clone()))
                .unwrap_or((404, b"missing".to_vec(), None));
            let location = location
                .map(|value| format!("Location: {value}\r\n"))
                .unwrap_or_default();
            let length = if sized {
                format!("Content-Length: {}\r\n", body.len())
            } else {
                String::new()
            };
            let header = format!(
                "HTTP/1.1 {status} fixture\r\n{length}Date: Tue, 06 Oct 2026 12:00:00 GMT\r\nConnection: close\r\n{location}\r\n"
            );
            let _ = stream.write_all(header.as_bytes()).await;
            let _ = stream.write_all(&body).await;
        }
    });
    (url, task)
}

#[tokio::test]
async fn a_download_takes_the_versioned_files_and_the_site_clock_and_refuses_the_rest() {
    let fixture = fixture();
    const MANIFEST: &str = "/course/sets/classroom/7/manifest.json";
    const CIPHER: &str = "/course/sets/classroom/7/tasks.enc";
    let valid = || {
        vec![
            (MANIFEST, 200, fixture.manifest.clone(), None),
            (CIPHER, 200, fixture.ciphertext.clone(), None),
        ]
    };
    let fetched = |routes| async move {
        let (site, server) = routed_fixture(routes).await;
        let downloaded = download(&Assignment {
            site,
            set_id: "classroom".to_owned(),
            version: 7,
        })
        .await;
        server.abort();
        downloaded
    };
    let downloaded = fetched(valid()).await.unwrap();
    assert_eq!(downloaded.ciphertext, fixture.ciphertext);
    assert_eq!(
        downloaded.site_time.map(|(at, _)| at),
        Some(parse_close("2026-10-06T12:00:00Z").unwrap())
    );
    let redirect = vec![
        (
            MANIFEST,
            302,
            Vec::new(),
            Some("https://elsewhere.invalid/manifest.json".to_owned()),
        ),
        (CIPHER, 200, fixture.ciphertext.clone(), None),
    ];
    let missing = vec![(MANIFEST, 200, fixture.manifest.clone(), None)];
    let oversize = vec![
        (MANIFEST, 200, vec![b' '; MAX_MANIFEST_BYTES + 1], None),
        (CIPHER, 200, fixture.ciphertext.clone(), None),
    ];
    for (name, routes) in [("redirect", redirect), ("404", missing)] {
        assert!(fetched(routes).await.is_err(), "{name} downloaded");
    }
    // Refused on its declared length, before a byte of the body is read.
    assert_eq!(
        fetched(oversize).await.err(),
        Some(invalid("package response refused"))
    );
    // A manifest may fill its limit exactly.
    let mut full = fixture.manifest.clone();
    full.resize(MAX_MANIFEST_BYTES, b' ');
    let at_limit = vec![
        (MANIFEST, 200, full.clone(), None),
        (CIPHER, 200, fixture.ciphertext.clone(), None),
    ];
    assert_eq!(fetched(at_limit).await.unwrap().manifest, full);

    // Without a declared length the limit is applied as the body arrives.
    let streamed = |manifest: Vec<u8>| async {
        let routes = vec![
            (MANIFEST, 200, manifest, None),
            (CIPHER, 200, fixture.ciphertext.clone(), None),
        ];
        let (site, server) = routed_fixture_sized(routes, false).await;
        let downloaded = download(&Assignment {
            site,
            set_id: "classroom".to_owned(),
            version: 7,
        })
        .await;
        server.abort();
        downloaded
    };
    assert_eq!(streamed(full.clone()).await.unwrap().manifest, full);
    let mut over = full.clone();
    over.push(b' ');
    assert_eq!(
        streamed(over).await.err(),
        Some(invalid("package response exceeds limit"))
    );
}

/// The fixture set as the packaging tool encrypts it: two tasks, default
/// rules.
fn plaintext() -> Value {
    let rows: Vec<Value> = [
        include_str!("../../fixtures/task-mode/customized.json"),
        include_str!("../../fixtures/task-mode/uncustomized.json"),
    ]
    .iter()
    .map(|text| serde_json::from_str(text).unwrap())
    .collect();
    let by_id = |field: &str| {
        Value::Object(
            rows.iter()
                .filter(|row| row.get(field).is_some())
                .map(|row| {
                    (
                        row["problem"]["id"].as_str().unwrap().to_owned(),
                        row[field].clone(),
                    )
                })
                .collect(),
        )
    };
    json!({"packageVersion": 1, "setId": "classroom", "setVersion": 7,
        "rules": {"durationMin": 15, "maxHintRungs": 3, "closesAt": null,
            "lookAwaySeconds": 8, "rulesNote": null},
        "problems": rows.iter().map(|row| row["problem"].clone()).collect::<Vec<_>>(),
        "judges": by_id("judge"), "variants": by_id("variant"), "sidecars": by_id("sidecar")})
}

fn decoded(value: &Value) -> Result<LoadedSet, TaskError> {
    decode("classroom", 7, &serde_json::to_vec(value).unwrap())
}

#[test]
fn a_decoded_package_must_name_the_set_it_was_opened_for() {
    let set = decoded(&plaintext()).unwrap();
    assert_eq!(set.records.len(), 2);
    assert_eq!(
        (set.config.id.as_str(), set.config.version),
        ("classroom", 7)
    );
    for (field, value) in [
        ("packageVersion", json!(2)),
        ("setId", json!("other")),
        ("setVersion", json!(8)),
        ("problems", json!([])),
    ] {
        let mut changed = plaintext();
        changed[field] = value;
        assert!(decoded(&changed).is_err(), "{field}");
    }
}

#[test]
fn set_rules_are_held_to_their_bounds_at_both_ends() {
    let with = |field: &str, value: Value| {
        let mut changed = plaintext();
        changed["rules"][field] = value;
        decoded(&changed)
    };
    let (low, high) = (
        crate::config::MIN_DURATION_MIN as u64,
        crate::config::MAX_DURATION_MIN as u64,
    );
    let hints = default_number("maxHintRungs");
    for (field, accepted, refused) in [
        ("durationMin", vec![low, high], vec![low - 1, high + 1]),
        ("maxHintRungs", vec![0, hints], vec![hints + 1]),
        ("lookAwaySeconds", vec![5, 60], vec![4, 61]),
    ] {
        for value in accepted {
            let set = with(field, json!(value)).unwrap_or_else(|e| panic!("{field}={value}: {e}"));
            assert_eq!(set.config.rule(field), value, "{field}");
        }
        for value in refused {
            assert!(with(field, json!(value)).is_err(), "{field}={value}");
        }
    }
    assert!(with("closesAt", json!("2026-02-31T00:00:00Z")).is_err());
    assert_eq!(
        with("closesAt", json!("2026-12-31T00:00:00Z"))
            .unwrap()
            .config
            .closes_at
            .as_deref(),
        Some("2026-12-31T00:00:00Z")
    );
    let note = "x".repeat(MAX_RULES_NOTE_BYTES);
    assert_eq!(
        with("rulesNote", json!(note)).unwrap().config.rules_note,
        Some(note)
    );
    for refused in [json!(" "), json!("x".repeat(MAX_RULES_NOTE_BYTES + 1))] {
        assert!(with("rulesNote", refused).is_err());
    }
}

#[test]
fn every_record_must_be_whole_and_belong_to_a_task() {
    let id = plaintext()["problems"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for field in ["judges", "variants"] {
        let mut missing = plaintext();
        missing[field].as_object_mut().unwrap().remove(&id);
        assert!(decoded(&missing).is_err(), "missing {field}");
        let mut orphan = plaintext();
        orphan[field]["stray-task"] = orphan[field][&id].clone();
        assert!(decoded(&orphan).is_err(), "orphan {field}");
    }
    let mut orphan = plaintext();
    orphan["sidecars"]["stray-task"] = orphan["sidecars"][&id].clone();
    assert!(decoded(&orphan).is_err(), "orphan sidecar");
    let mut duplicate = plaintext();
    let first = duplicate["problems"][0].clone();
    duplicate["problems"].as_array_mut().unwrap().push(first);
    assert_eq!(decoded(&duplicate).err().unwrap().0, "duplicate task id");
    let mut nameless = plaintext();
    nameless["problems"][0]
        .as_object_mut()
        .unwrap()
        .remove("id");
    assert!(decoded(&nameless).is_err());
}

#[test]
fn one_task_may_not_outgrow_its_byte_limit() {
    let limit = default_number("taskBytes") as usize;
    let mut padded = plaintext();
    let id = padded["problems"][0]["id"].as_str().unwrap().to_owned();
    let row_bytes = |value: &Value| {
        serde_json::to_vec(
            &json!({"problem": value["problems"][0], "judge": value["judges"][&id],
            "variant": value["variants"][&id], "sidecar": value["sidecars"].get(&id)}),
        )
        .unwrap()
        .len()
    };

    // Padded in a judge case's input, which no text bound limits, so the record
    // stays valid and the byte limit is the only rule in play.
    let cases = padded["judges"][&id]["cases"].as_array_mut().unwrap();
    let mut case = cases[0].clone();
    case["label"] = json!("padding");
    case["input"] = json!([""]);
    cases.push(case);
    let pad = |value: &mut Value, size: usize| {
        let cases = value["judges"][&id]["cases"].as_array_mut().unwrap();
        cases.last_mut().unwrap()["input"] = json!(["x".repeat(size)]);
    };
    // Exactly at the limit is still a task; one byte more is not.
    let room = limit - row_bytes(&padded);
    pad(&mut padded, room);
    assert_eq!(row_bytes(&padded), limit);
    assert!(decoded(&padded).is_ok());
    pad(&mut padded, room + 1);
    assert_eq!(
        decoded(&padded).err().unwrap().0,
        "plaintext task exceeds limit"
    );
}

/// Each manifest rule on its own, with the rest of the manifest made to agree:
/// a wrong PIN is what a ciphertext that passes them all and still fails to
/// decrypt reports, so passing a check shows as `WrongPin` and failing one
/// as `Invalid`.
#[test]
fn each_manifest_rule_refuses_on_its_own() {
    let fixture = fixture();
    let refused = || {
        Some(OpenError::Invalid(invalid(
            "invalid manifest or ciphertext",
        )))
    };
    let open = |patch: &dyn Fn(&mut Value), ciphertext: Vec<u8>| {
        let mut manifest: Value = serde_json::from_slice(&fixture.manifest).unwrap();
        manifest["ciphertextBytes"] = json!(ciphertext.len());
        manifest["ciphertextSha256"] = json!(crate::sha256_hex(&[&ciphertext]));
        patch(&mut manifest);
        open_package(
            "classroom",
            7,
            &serde_json::to_vec(&manifest).unwrap(),
            ciphertext,
            PIN,
        )
        .err()
    };
    let keep = |_: &mut Value| {};
    assert_eq!(open(&keep, fixture.ciphertext.clone()), None);
    assert_eq!(
        open(
            &|manifest| manifest["salt"] = json!("AAAAAAAAAAAAAAAAAAAA"),
            fixture.ciphertext.clone()
        ),
        refused()
    );
    assert_eq!(
        open(
            &|manifest| manifest["ciphertextBytes"] = json!(fixture.ciphertext.len() + 1),
            fixture.ciphertext.clone()
        ),
        refused()
    );
    // A ciphertext holds a nonce and a tag around at most `setBytes`.
    let largest = default_number("setBytes") as usize + 28;
    for (size, expected) in [
        (27, refused()),
        (28, Some(OpenError::WrongPin)),
        (largest, Some(OpenError::WrongPin)),
        (largest + 1, refused()),
    ] {
        assert_eq!(open(&keep, vec![7; size]), expected, "{size}");
    }
}
