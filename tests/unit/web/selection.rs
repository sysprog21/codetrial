use super::*;

// Policy checks need one known draw, not OS entropy. If a mutation rejects it,
// fail on the next request instead of waiting for an improbable random value.
// The token API tests still exercise the production SystemRandom adapter.
fn assign_with_zero(
    request: &Value,
    owner: i64,
    secret: &str,
) -> Result<Option<Assignment>, SelectionError> {
    let mut word = Some(0_u64);
    assign_with_entropy(request, owner, secret, |bytes| {
        let value = word.take().expect("unexpected entropy request");
        bytes.copy_from_slice(&value.to_le_bytes());
        Ok(())
    })
}

#[test]
fn practice_keeps_the_requested_problem_and_needs_no_receipt() {
    for request in [
        json!({}),
        json!({"problemId": "anything"}),
        json!({"selectionMode": "practice", "problemId": "two-sum",
            "previousAssignment": "ignored"}),
    ] {
        assert!(assign_with_zero(&request, 7, "secret").unwrap().is_none());
    }
    for mode in [json!("typo"), json!(null), json!(true)] {
        assert_eq!(
            assign_with_zero(&json!({"selectionMode": mode}), 7, "secret").err(),
            Some(SelectionError::Mode)
        );
    }
}

#[test]
fn strict_refuses_client_problem_selection() {
    for value in [json!("two-sum"), json!(null), json!("")] {
        assert_eq!(
            assign_with_zero(
                &json!({"selectionMode": "strict", "problemId": value}),
                7,
                "secret"
            )
            .err(),
            Some(SelectionError::Problem)
        );
    }
}

#[test]
fn a_receipt_is_bound_to_its_owner_purpose_and_exact_page() {
    let page = PROBLEMS[0].variant().page;
    let receipt = receipt_for(page, 7, "secret");
    assert_eq!(read_receipt(&receipt, 7, "secret").unwrap(), page);
    for (value, owner, secret) in [
        (receipt.clone(), 8, "secret"),
        (receipt.clone(), 7, "changed"),
        (receipt.replace(page, "missing-page"), 7, "secret"),
        (receipt_for("missing-page", 7, "secret"), 7, "secret"),
        (receipt_for(PROBLEMS[0].id, 7, "secret"), 7, "secret"),
        ("x".repeat(513), 7, "secret"),
        ("unsigned".to_string(), 7, "secret"),
    ] {
        assert_eq!(
            read_receipt(&value, owner, secret),
            Err(SelectionError::Receipt)
        );
    }
    let payload = format!("another-purpose:7:{page}");
    let other = format!("{payload}.{}", sign_hs256("secret", &payload));
    assert_eq!(
        read_receipt(&other, 7, "secret"),
        Err(SelectionError::Receipt)
    );
}

#[test]
fn selection_excludes_only_the_previous_problem() {
    let bank = &PROBLEMS[..2];
    let first = choose(bank, DIFFICULTIES, None, |_| Ok(0)).unwrap();
    let second = choose(bank, DIFFICULTIES, Some(first.variant().page), |count| {
        assert_eq!(count, 1);
        Ok(0)
    })
    .unwrap();
    assert_ne!(first.id, second.id);
    let third = choose(bank, DIFFICULTIES, Some(second.variant().page), |_| Ok(0)).unwrap();
    assert_eq!(third.id, first.id);
    for index in 0..PROBLEMS.len() - 1 {
        assert_ne!(
            choose(PROBLEMS, DIFFICULTIES, Some(first.variant().page), |_| Ok(
                index
            ))
            .unwrap()
            .id,
            first.id
        );
    }
}

#[test]
fn an_empty_pool_or_a_lone_previous_problem_never_falls_back() {
    assert_eq!(
        choose(&[], DIFFICULTIES, None, |_| panic!("no draw")).err(),
        Some(SelectionError::NoAlternative)
    );
    let bank = &PROBLEMS[..1];
    assert_eq!(
        choose(bank, DIFFICULTIES, None, |_| Ok(0)).unwrap().id,
        bank[0].id
    );
    assert_eq!(
        choose(
            bank,
            DIFFICULTIES,
            Some(bank[0].variant().page),
            |_| panic!("no draw")
        )
        .err(),
        Some(SelectionError::NoAlternative)
    );
    assert_eq!(
        choose(bank, DIFFICULTIES, None, |_| Err(SelectionError::Random)).err(),
        Some(SelectionError::Random)
    );
}

#[test]
fn independent_tabs_exclude_their_own_previous_assignment() {
    for previous in &PROBLEMS[..2] {
        let receipt = receipt_for(previous.variant().page, 7, "secret");
        let assignment = assign_with_zero(
            &json!({"selectionMode": "strict",
            "previousAssignment": receipt}),
            7,
            "secret",
        )
        .unwrap()
        .unwrap();
        assert_ne!(assignment.page, previous.variant().page);
        assert_eq!(
            read_receipt(&assignment.receipt, 7, "secret").unwrap(),
            assignment.page
        );
    }
    for value in [json!("bad"), json!(null), json!(42)] {
        assert_eq!(
            assign_with_zero(
                &json!({"selectionMode": "strict",
            "previousAssignment": value}),
                7,
                "secret"
            )
            .err(),
            Some(SelectionError::Receipt)
        );
    }
}

#[test]
fn every_eligible_draw_respects_difficulties_and_the_previous_assignment() {
    for levels in [
        vec!["Easy"],
        vec!["Medium"],
        vec!["Hard"],
        vec!["Easy", "Hard"],
        DIFFICULTIES.to_vec(),
    ] {
        let first = PROBLEMS
            .iter()
            .find(|p| levels.contains(&p.difficulty))
            .unwrap();
        for previous in [None, Some(first.variant().page)] {
            let eligible = PROBLEMS
                .iter()
                .filter(|p| levels.contains(&p.difficulty) && Some(p.variant().page) != previous)
                .count();
            assert!(eligible > 0);
            for index in 0..eligible {
                let picked = choose(PROBLEMS, &levels, previous, |count| {
                    assert_eq!(count, eligible);
                    Ok(index)
                })
                .unwrap();
                assert!(levels.contains(&picked.difficulty));
                assert_ne!(Some(picked.variant().page), previous);
            }
        }
    }
}

#[test]
fn difficulty_exhaustion_never_uses_another_level_or_the_previous_problem() {
    let easy = PROBLEMS.iter().find(|p| p.difficulty == "Easy").unwrap();
    let hard = PROBLEMS.iter().find(|p| p.difficulty == "Hard").unwrap();
    let bank = [*easy, *hard];
    assert_eq!(
        choose(&bank, &["Easy"], None, |count| {
            assert_eq!(count, 1);
            Ok(0)
        })
        .unwrap(),
        easy
    );
    for (levels, previous) in [
        (vec!["Medium"], None),
        (vec!["Easy"], Some(easy.variant().page)),
    ] {
        assert_eq!(
            choose(&bank, &levels, previous, |_| panic!("no eligible draw")).err(),
            Some(SelectionError::NoAlternative)
        );
    }
    // A receipt remains meaningful when the next request changes difficulty.
    assert_eq!(
        choose(&bank, &["Hard"], Some(easy.variant().page), |_| Ok(0)).unwrap(),
        hard
    );
    assert_eq!(
        SelectionError::NoAlternative.response().status(),
        StatusCode::CONFLICT
    );
}

#[test]
fn strict_validates_the_entire_difficulty_filter() {
    for value in [
        json!(null),
        json!("Easy"),
        json!(true),
        json!(1),
        json!({}),
        json!([]),
        json!(["easy"]),
        json!(["Easy", "Extreme"]),
        json!(["Easy", null]),
        json!(["Easy", 1]),
        json!(["Easy", ""]),
        json!(["Easy", "Easy"]),
        json!(["Easy", "Medium", "Hard", "Easy"]),
    ] {
        assert_eq!(
            assign_with_zero(
                &json!({"selectionMode": "strict", "difficulties": value}),
                7,
                "secret"
            )
            .err(),
            Some(SelectionError::Difficulty)
        );
    }
    for levels in [
        json!(["Easy"]),
        json!(["Medium"]),
        json!(["Hard"]),
        json!(["Easy", "Hard"]),
    ] {
        let assignment = assign_with_zero(
            &json!({"selectionMode": "strict", "difficulties": levels}),
            7,
            "secret",
        )
        .unwrap()
        .unwrap();
        let problem = find_problem(assignment.page).unwrap();
        assert!(
            levels
                .as_array()
                .unwrap()
                .contains(&json!(problem.difficulty))
        );
        assert_eq!(
            read_receipt(&assignment.receipt, 7, "secret").unwrap(),
            assignment.page
        );
    }
}

#[test]
fn receipt_parsing_bounds_bytes_before_authentication() {
    // These parseable forgeries distinguish the size gate from the signature
    // and page checks, which reject them even when the size gate is broken.
    for len in [511, 512, 513, 1024] {
        let payload = "x".repeat(len - 2);
        let receipt = format!("{payload}.s");
        assert_eq!(receipt.len(), len);
        if len <= 512 {
            assert_eq!(receipt_parts(&receipt), Ok((payload.as_str(), "s")));
        } else {
            assert_eq!(receipt_parts(&receipt), Err(SelectionError::Receipt));
        }
        assert_eq!(
            read_receipt(&receipt, 7, "secret"),
            Err(SelectionError::Receipt)
        );
    }
    let multibyte = format!("{}.s", "\u{00e9}".repeat(256));
    assert_eq!(multibyte.len(), 514);
    assert_eq!(receipt_parts(&multibyte), Err(SelectionError::Receipt));
    for receipt in ["", "unsigned"] {
        assert_eq!(receipt_parts(receipt), Err(SelectionError::Receipt));
    }
    assert_eq!(
        receipt_parts("payload.with.dots.signature"),
        Ok(("payload.with.dots", "signature"))
    );
}

// Every supplied word must be consumed, and an unexpected retry fails instead
// of hanging. This observes the rejection path as well as the returned index.
fn index_from_words(
    count: usize,
    words: impl IntoIterator<Item = Result<u64, SelectionError>>,
) -> Result<usize, SelectionError> {
    let mut words = words.into_iter();
    let result = random_index(count, |bytes| {
        let word = words.next().expect("unexpected entropy request")?;
        bytes.copy_from_slice(&word.to_le_bytes());
        Ok(())
    });
    assert!(
        words.next().is_none(),
        "returned without consuming the expected entropy"
    );
    result
}

#[test]
fn random_indices_map_complete_buckets_without_favoring_an_index() {
    for count in [1, 2, 3, 7, 150] {
        for bucket in [0, 1, 2] {
            for expected in 0..count {
                let word = (bucket * count + expected) as u64;
                assert_eq!(index_from_words(count, [Ok(word)]), Ok(expected));
            }
        }
    }
}

#[test]
fn random_indices_reject_the_upper_boundary_and_retry() {
    // MAX is divisible by three. Two and ten leave partial buckets of different
    // sizes; spelling out the boundaries avoids copying the formula.
    for (count, limit, last_index) in [
        (1, u64::MAX, 0),
        (2, u64::MAX - 1, 1),
        (3, u64::MAX, 2),
        (10, u64::MAX - 5, 9),
    ] {
        assert_eq!(index_from_words(count, [Ok(limit - 1)]), Ok(last_index));
        assert_eq!(index_from_words(count, [Ok(limit), Ok(0)]), Ok(0));
        assert_eq!(
            index_from_words(count, [Ok(u64::MAX), Ok(limit), Ok(0)]),
            Ok(0)
        );
    }
    assert_eq!(index_from_words(3, [Ok(u64::MAX), Ok(5)]), Ok(2));
}

#[test]
fn entropy_failure_never_selects_a_default_or_retries_the_source() {
    assert_eq!(
        assign_with_entropy(&json!({"selectionMode": "strict"}), 7, "secret", |_| Err(
            SelectionError::Random
        ))
        .err(),
        Some(SelectionError::Random)
    );
    assert_eq!(
        index_from_words(3, [Err(SelectionError::Random)]),
        Err(SelectionError::Random)
    );
    assert_eq!(
        index_from_words(3, [Ok(u64::MAX), Err(SelectionError::Random)]),
        Err(SelectionError::Random)
    );
    assert_eq!(
        SelectionError::Random.response().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn random_indices_cover_the_largest_pool_on_this_target() {
    let count = usize::MAX;
    assert_eq!(
        index_from_words(count, [Ok((count - 1) as u64)]),
        Ok(count - 1)
    );
    assert_eq!(index_from_words(count, [Ok(u64::MAX), Ok(1)]), Ok(1));
}
