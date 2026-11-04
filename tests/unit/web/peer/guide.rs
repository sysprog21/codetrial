use super::*;

#[test]
fn published_guide_uses_the_variant_contract_and_three_hint_rungs() {
    let guide = present("two-sum", true).unwrap();
    let variant = crate::agent::variant_for("two-sum").unwrap();
    assert_eq!(guide["contract"], variant.contract);
    assert_eq!(guide["hints"].as_array().unwrap().len(), 3);
    assert_eq!(
        guide["optimal"],
        crate::agent::find_problem("two-sum").unwrap().optimal
    );
    let problem = present("two-sum", false).unwrap();
    assert_eq!(problem["page"], variant.page);
    assert!(problem.get("hints").is_none());
    assert!(present("unknown-peer-problem", true).is_none());
}
