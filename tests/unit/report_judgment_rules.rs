use super::super::refused_judgments;

/// "Confident" after a verb of looking or sounding is a delivery judgment in
/// the tenses, persons and participles listed, not only the few listed first.
#[test]
fn the_listed_forms_of_looking_or_sounding_confident_are_refused() {
    for claim in [
        "You came across as confident while explaining the loop.",
        "You sound confident about the invariant.",
        "Your answer sounds confident.",
        "You seem confident with the approach.",
        "She seems confident throughout.",
        "You look confident at the keyboard.",
        "You appear confident in the walkthrough.",
        "Sounding confident helped the explanation.",
        "Looking confident at the keyboard won it.",
        "Seeming confident, you moved on.",
        "Appearing confident, you wrote the loop.",
    ] {
        assert!(
            !refused_judgments(claim, false).is_empty(),
            "{claim:?} was accepted"
        );
    }
    // What describes the work, or advises, is not a judgment of the person.
    for fine in [
        "The invariant looks correct to me.",
        "Be confident in the proof by stating the invariant first.",
    ] {
        assert!(
            refused_judgments(fine, false).is_empty(),
            "{fine:?} refused"
        );
    }
}

/// A phrase that also has a technical meaning is refused only in the sense a
/// candidate is judged by: a posture is a body's, a primary language is one
/// the candidate speaks, and neither refuses advice about code or design.
#[test]
fn a_phrase_with_a_technical_meaning_is_refused_only_in_the_personal_sense() {
    for claim in [
        "Your posture looked closed.",
        "Their posture suggested hesitation.",
        "Good body posture throughout.",
    ] {
        assert!(!refused_judgments(claim, false).is_empty(), "{claim:?}");
    }
    for claim in [
        "Speak in your primary language during the interview.",
        "Your primary spoken language was audible.",
    ] {
        assert!(!refused_judgments(claim, true).is_empty(), "{claim:?}");
    }
    for fine in [
        "The design leaves the security posture of the cache unaddressed.",
        "A defensive posture toward bad input would catch this earlier.",
    ] {
        assert!(refused_judgments(fine, false).is_empty(), "{fine:?}");
    }
    for fine in [
        "Practice the pattern in your primary language.",
        "Port the solution to a primary language of your choice and compare.",
    ] {
        assert!(refused_judgments(fine, true).is_empty(), "{fine:?}");
    }
}
