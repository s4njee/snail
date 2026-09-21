use serde::Deserialize;
use snail_core::pgp::{CertificateFacts, evaluate_certificate_facts};

#[derive(Deserialize)]
struct Case {
    name: String,
    facts: CertificateFacts,
    expected: String,
}

#[test]
fn adversarial_certificate_fixtures_are_never_reported_valid() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/e10-certificate-policy.json")).unwrap();
    for case in cases {
        let result = evaluate_certificate_facts(&case.facts);
        if case.expected == "valid" {
            assert!(result.is_ok(), "{}: {result:?}", case.name);
        } else {
            let reason = result.expect_err(&case.name);
            assert_eq!(format!("{reason:?}"), case.expected, "{}", case.name);
        }
    }
}
