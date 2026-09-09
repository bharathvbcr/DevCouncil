use dc_evidence::{
    ArtifactInputs, ExpectedRun, Verdict, parse_bundle, parse_contract, sha256, verify,
};
use serde_json::{Value, json};

const CONTRACT: &[u8] = include_bytes!("../fixtures/v1/contract.json");
const BUNDLE: &[u8] = include_bytes!("../fixtures/v1/bundle.json");
const NOTE: &[u8] = include_bytes!("../fixtures/v1/note.txt");

fn expected() -> ExpectedRun {
    serde_json::from_slice(include_bytes!("../fixtures/v1/expected.json")).unwrap()
}

fn inputs() -> ArtifactInputs<'static> {
    ArtifactInputs::from([("note", Ok(NOTE))])
}

fn edit_bundle(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut bundle: Value = serde_json::from_slice(BUNDLE).unwrap();
    edit(&mut bundle);
    serde_json::to_vec(&bundle).unwrap()
}

#[test]
fn immutable_fixture_passes_and_report_binds_exact_input_bytes() {
    let report = verify(CONTRACT, BUNDLE, &expected(), &inputs()).unwrap();
    assert_eq!(report.verdict, Verdict::Passed);
    assert_eq!(report.bundle_sha256, sha256(BUNDLE));
    assert_eq!(report.contract_sha256, sha256(CONTRACT));
    assert_eq!(report.criteria[0].verdict, Verdict::Passed);
    assert!(report.issues.is_empty());
    assert!(report.ok);
}

#[test]
fn refused_input_attempt_does_not_replace_successful_outcome_evidence() {
    let retried = edit_bundle(|b| {
        b["actions"][0]["sequence"] = json!(2);
        b["observations"][0]["sequence"] = json!(3);
        b["actions"].as_array_mut().unwrap().insert(
            0,
            json!({"id":"refused-attempt","sequence":1,"disposition":"not_dispatched"}),
        );
    });
    assert_eq!(
        verify(CONTRACT, &retried, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Passed
    );
    for disposition in ["unknown", "failed", "cancelled", "denied"] {
        let mut changed: Value = serde_json::from_slice(&retried).unwrap();
        changed["actions"][0]["disposition"] = json!(disposition);
        assert_ne!(
            verify(
                CONTRACT,
                &serde_json::to_vec(&changed).unwrap(),
                &expected(),
                &inputs()
            )
            .unwrap()
            .verdict,
            Verdict::Passed
        );
    }
    let forged = edit_bundle(|b| b["actions"][0]["disposition"] = json!("not_dispatched"));
    assert_ne!(
        verify(CONTRACT, &forged, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Passed
    );
}

#[test]
fn independent_admission_values_cannot_be_replaced_by_bundle_assertions() {
    for field in [
        "run_id",
        "session_id",
        "contract_sha256",
        "capability_sha256",
    ] {
        let bundle = edit_bundle(|b| {
            b[field] = json!(if field.ends_with("sha256") {
                "0".repeat(64)
            } else {
                "other".into()
            })
        });
        assert_eq!(
            verify(CONTRACT, &bundle, &expected(), &inputs())
                .unwrap()
                .verdict,
            Verdict::Failed,
            "{field}"
        );
    }
    let mut admission = expected();
    admission.contract_sha256 = "0".repeat(64);
    assert_eq!(
        verify(CONTRACT, BUNDLE, &admission, &inputs())
            .unwrap()
            .verdict,
        Verdict::Failed
    );
}

#[test]
fn whitespace_changes_contract_identity_without_changing_parsed_semantics() {
    let mut changed = CONTRACT.to_vec();
    changed.push(b' ');
    assert_eq!(
        verify(&changed, BUNDLE, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Failed
    );
}

#[test]
fn runner_pass_field_is_not_an_accepted_oracle() {
    let bundle = edit_bundle(|b| b["passed"] = json!(true));
    assert!(parse_bundle(&bundle).is_err());
}

#[test]
fn missing_empty_or_degraded_journals_never_pass() {
    let edits: Vec<(&str, Value)> = vec![
        ("actions", json!([])),
        ("observations", json!([])),
        ("journal_complete", json!(false)),
        ("degraded", json!(["capture unavailable"])),
    ];
    for (field, value) in edits {
        let bundle = edit_bundle(|b| b[field] = value);
        assert_eq!(
            verify(CONTRACT, &bundle, &expected(), &inputs())
                .unwrap()
                .verdict,
            Verdict::Incomplete,
            "{field}"
        );
    }
}

#[test]
fn missing_fact_does_not_mean_unequal_or_exists() {
    let bundle = edit_bundle(|b| b["observations"][0]["facts"] = json!({}));
    assert_eq!(
        verify(CONTRACT, &bundle, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Incomplete
    );
}

#[test]
fn action_failure_dominates_incomplete_capture() {
    let bundle = edit_bundle(|b| {
        b["actions"][0]["disposition"] = json!("failed");
        b["journal_complete"] = json!(false);
    });
    assert_eq!(
        verify(CONTRACT, &bundle, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Failed
    );
    let without_observation = edit_bundle(|b| {
        b["actions"][0]["disposition"] = json!("failed");
        b["observations"] = json!([]);
    });
    assert_eq!(
        verify(CONTRACT, &without_observation, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Failed
    );
}

#[test]
fn uncertain_cancelled_or_denied_action_dispositions_are_not_completion() {
    for disposition in ["unknown", "cancelled", "denied"] {
        let bundle = edit_bundle(|b| b["actions"][0]["disposition"] = json!(disposition));
        assert_eq!(
            verify(CONTRACT, &bundle, &expected(), &inputs())
                .unwrap()
                .verdict,
            Verdict::Incomplete
        );
    }
}

#[test]
fn wrong_run_session_or_epoch_observation_is_not_replayable_evidence() {
    for field in ["run_id", "session_id", "epoch"] {
        let bundle = edit_bundle(|b| {
            b["observations"][0][field] = if field == "epoch" {
                json!(2)
            } else {
                json!("other")
            }
        });
        assert_eq!(
            verify(CONTRACT, &bundle, &expected(), &inputs())
                .unwrap()
                .verdict,
            Verdict::Failed
        );
    }
}

fn resumed_bundle(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    edit_bundle(|b| {
        b["epoch_transitions"] = json!([
            {"sequence":3,"from_epoch":1,"to_epoch":2,"reason":"pause"},
            {"sequence":4,"from_epoch":2,"to_epoch":3,"reason":"resume"}
        ]);
        let mut resumed = b["observations"][0].clone();
        resumed["sequence"] = json!(5);
        resumed["epoch"] = json!(3);
        b["observations"].as_array_mut().unwrap().push(resumed);
        edit(b);
    })
}

#[test]
fn admitted_epoch_and_acknowledged_transitions_allow_fresh_recovery() {
    let fixture = include_bytes!("../fixtures/v1/bundle-resumed.json");
    assert_eq!(
        verify(CONTRACT, fixture, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Passed
    );
    let bundle = resumed_bundle(|_| {});
    assert_eq!(
        verify(CONTRACT, &bundle, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Passed
    );
    let mut wrong_admission = expected();
    wrong_admission.epoch = 3;
    assert_eq!(
        verify(CONTRACT, &bundle, &wrong_admission, &inputs())
            .unwrap()
            .verdict,
        Verdict::Failed
    );
}

#[test]
fn transitions_cannot_reuse_old_epoch_observation_or_old_final_state() {
    let stale_epoch = resumed_bundle(|b| b["observations"][1]["epoch"] = json!(1));
    assert_eq!(
        verify(CONTRACT, &stale_epoch, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Failed
    );
    let stale_state = resumed_bundle(|b| {
        b["observations"].as_array_mut().unwrap().pop();
    });
    assert_eq!(
        verify(CONTRACT, &stale_state, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Incomplete
    );
    let unknown = resumed_bundle(|b| b["actions"][0]["disposition"] = json!("unknown"));
    assert_eq!(
        verify(CONTRACT, &unknown, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Incomplete
    );
}

#[test]
fn invalid_transition_chains_and_global_sequence_collisions_are_refused() {
    for (field, value) in [
        ("sequence", json!(2)),
        ("sequence", json!(0)),
        ("from_epoch", json!(9)),
        ("to_epoch", json!(1)),
        ("reason", json!("")),
    ] {
        let bundle = resumed_bundle(|b| b["epoch_transitions"][0][field] = value);
        assert!(
            verify(CONTRACT, &bundle, &expected(), &inputs()).is_err(),
            "{field}"
        );
    }
    let reversed = resumed_bundle(|b| b["epoch_transitions"].as_array_mut().unwrap().reverse());
    assert!(verify(CONTRACT, &reversed, &expected(), &inputs()).is_err());
    let too_many = edit_bundle(|b| {
        b["epoch_transitions"] = json!(vec![
            json!({"sequence":3,"from_epoch":1,"to_epoch":2,"reason":"pause"});
            dc_evidence::MAX_EVENTS
        ])
    });
    assert!(parse_bundle(&too_many).is_err());
}

#[test]
fn later_action_requires_new_observation_and_cannot_reuse_earlier_success() {
    let bundle = edit_bundle(|b| {
        b["actions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"action-2","sequence":3,"disposition":"succeeded"}))
    });
    assert_eq!(
        verify(CONTRACT, &bundle, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Incomplete
    );
}

#[test]
fn final_observation_can_revoke_earlier_satisfaction() {
    let bundle = edit_bundle(|b| {
        let mut later = b["observations"][0].clone();
        later["sequence"] = json!(3);
        later["facts"]["document.text"] = json!("Wrong content");
        b["observations"].as_array_mut().unwrap().push(later);
    });
    assert_eq!(
        verify(CONTRACT, &bundle, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Failed
    );
}

#[test]
fn sequence_order_duplicates_and_unknown_links_are_invalid() {
    for field in ["duplicate_action", "duplicate_sequence", "unknown_action"] {
        let bundle = edit_bundle(|b| match field {
            "duplicate_action" => {
                let action = b["actions"][0].clone();
                b["actions"].as_array_mut().unwrap().push(action);
            }
            "duplicate_sequence" => b["observations"][0]["sequence"] = json!(1),
            _ => b["observations"][0]["after_action_id"] = json!("missing"),
        });
        assert!(
            verify(CONTRACT, &bundle, &expected(), &inputs()).is_err(),
            "{field}"
        );
    }
}

#[test]
fn observation_cannot_reference_action_preceding_the_most_recent_action() {
    let bundle = edit_bundle(|b| {
        b["actions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"action-2","sequence":2,"disposition":"succeeded"}));
        b["observations"][0]["sequence"] = json!(3);
    });
    assert_eq!(
        verify(CONTRACT, &bundle, &expected(), &inputs())
            .unwrap()
            .verdict,
        Verdict::Failed
    );
}

#[test]
fn unavailable_and_corrupt_artifacts_are_distinct() {
    assert_eq!(
        verify(CONTRACT, BUNDLE, &expected(), &ArtifactInputs::new())
            .unwrap()
            .verdict,
        Verdict::Incomplete
    );
    let bad = ArtifactInputs::from([("note", Ok(b"changed".as_slice()))]);
    assert_eq!(
        verify(CONTRACT, BUNDLE, &expected(), &bad).unwrap().verdict,
        Verdict::Failed
    );
    let missing = ArtifactInputs::from([("note", Err("read refused"))]);
    assert_eq!(
        verify(CONTRACT, BUNDLE, &expected(), &missing)
            .unwrap()
            .verdict,
        Verdict::Incomplete
    );
}

#[test]
fn undeclared_and_duplicate_artifact_references_are_invalid() {
    let extra = ArtifactInputs::from([("other", Ok(NOTE))]);
    assert!(verify(CONTRACT, BUNDLE, &expected(), &extra).is_err());
    for ids in [json!(["missing"]), json!(["note", "note"])] {
        let bundle = edit_bundle(|b| b["observations"][0]["artifact_ids"] = ids);
        assert!(verify(CONTRACT, &bundle, &expected(), &inputs()).is_err());
    }
}

#[test]
fn artifact_paths_are_portable_relative_and_bounded() {
    for path in [
        "../secret",
        "/secret",
        "C:/secret",
        "a\\b",
        "a/../b",
        "a//b",
        "./a",
        "CON",
        "x ",
        "x.",
    ] {
        let bundle = edit_bundle(|b| b["artifacts"][0]["path"] = json!(path));
        assert!(parse_bundle(&bundle).is_err(), "{path}");
    }
    let oversized = edit_bundle(|b| {
        b["artifacts"][0]["size_bytes"] = json!(dc_evidence::MAX_ARTIFACT_BYTES + 1)
    });
    assert!(parse_bundle(&oversized).is_err());
}

#[test]
fn schemas_duplicates_defaults_and_depth_fail_closed() {
    let contract: Value = serde_json::from_slice(CONTRACT).unwrap();
    let mut bad = contract.clone();
    bad["schema_version"] = json!(2);
    assert!(parse_contract(&serde_json::to_vec(&bad).unwrap()).is_err());
    bad = contract.clone();
    bad["criteria"][0]
        .as_object_mut()
        .unwrap()
        .remove("required");
    assert!(parse_contract(&serde_json::to_vec(&bad).unwrap()).is_err());
    bad = contract.clone();
    bad["criteria"][0]["predicate"]["op"] = json!("runner_passed");
    assert!(parse_contract(&serde_json::to_vec(&bad).unwrap()).is_err());
    let repeated = String::from_utf8(BUNDLE.to_vec()).unwrap().replace(
        "\"document.text\": \"Hello Jarvis\"",
        "\"document.text\":false,\"document.text\":\"Hello Jarvis\"",
    );
    assert!(parse_bundle(repeated.as_bytes()).is_err());
    assert!(parse_bundle(format!("{}0{}", "[".repeat(256), "]".repeat(256)).as_bytes()).is_err());
    assert!(parse_bundle(&vec![b' '; dc_evidence::MAX_BUNDLE_BYTES + 1]).is_err());
}

fn compare(op: &str, desired: Value, actual: Value) -> Verdict {
    let mut contract: Value = serde_json::from_slice(CONTRACT).unwrap();
    contract["criteria"][0]["predicate"] = if op == "exists" {
        json!({"op":op})
    } else {
        json!({"op":op,"value":desired})
    };
    let contract = serde_json::to_vec(&contract).unwrap();
    let mut expected = expected();
    expected.contract_sha256 = sha256(&contract);
    let bundle = edit_bundle(|b| {
        b["contract_sha256"] = json!(expected.contract_sha256);
        b["observations"][0]["facts"]["document.text"] = actual;
    });
    verify(&contract, &bundle, &expected, &inputs())
        .unwrap()
        .verdict
}

#[test]
fn typed_predicates_compare_values_without_string_coercion() {
    assert_eq!(
        compare("exists", Value::Null, json!(false)),
        Verdict::Passed
    );
    assert_eq!(compare("exists", Value::Null, Value::Null), Verdict::Failed);
    assert_eq!(
        compare("equals", json!(false), json!("false")),
        Verdict::Failed
    );
    assert_eq!(
        compare("equals", json!({"x":[1,true]}), json!({"x":[1,true]})),
        Verdict::Passed
    );
    assert_eq!(compare("not_equals", json!(1), json!(2)), Verdict::Passed);
    assert_eq!(
        compare("contains", json!("Jarvis"), json!("Hello Jarvis")),
        Verdict::Passed
    );
    assert_eq!(
        compare("contains", json!([false, 2]), json!([1, 2, false])),
        Verdict::Passed
    );
    assert_eq!(
        compare("contains", json!("1"), json!([1])),
        Verdict::Incomplete
    );
    assert_eq!(compare("minimum", json!(2), json!(3)), Verdict::Passed);
    assert_eq!(compare("minimum", json!(2), json!(1)), Verdict::Failed);
    assert_eq!(
        compare("minimum", json!(2), json!("3")),
        Verdict::Incomplete
    );
    assert_eq!(
        compare("minimum", json!(2), json!(9_007_199_254_740_992_u64)),
        Verdict::Incomplete
    );
}

#[test]
fn empty_or_optional_only_contract_cannot_pass_vacuously() {
    for criteria in [
        json!([]),
        json!([{"id":"x","fact":"x","required":false,"predicate":{"op":"exists"}}]),
    ] {
        assert!(
            parse_contract(
                &serde_json::to_vec(&json!({"schema_version":1,"id":"x","criteria":criteria}))
                    .unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn same_inputs_replay_to_exactly_the_same_report() {
    let one =
        serde_json::to_vec(&verify(CONTRACT, BUNDLE, &expected(), &inputs()).unwrap()).unwrap();
    let two =
        serde_json::to_vec(&verify(CONTRACT, BUNDLE, &expected(), &inputs()).unwrap()).unwrap();
    assert_eq!(one, two);
}
