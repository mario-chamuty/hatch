//! Stream B deliverable 7 – TOFU trust model, non-interactive behaviour.
//!
//! When stdin is not a TTY (e.g. CI, test harness, piped) and no prior
//! trust record exists for a dependency-authored script, the trust check
//! MUST return an error. Setting `HATCH_TRUST_SCRIPTS=all` bypasses and is
//! audit-logged.
//!
//! Env-var driven tests share a process, so they are serialised behind a
//! single `#[test]` to avoid races between `set_var`/`remove_var`.

use hatch::scripts::trust::{check_script_trust, ScriptOrigin};

fn uniq_origin(tag: &str) -> ScriptOrigin {
    ScriptOrigin::Dependency {
        package: format!("stream_b_test_{tag}_{}", std::process::id()),
        version: "0.0.0-nope".to_string(),
    }
}

#[test]
fn trust_model_envvar_and_non_tty_behaviour() {
    // ------------------------------------------------------------------
    // 1. No env var, no prior trust, non-TTY – must fail with a clear
    //    message that mentions the non-interactive situation.
    // ------------------------------------------------------------------
    std::env::remove_var("HATCH_TRUST_SCRIPTS");
    let origin = uniq_origin("deny_path");
    let err = check_script_trust(&origin, "postinstall", "echo hi")
        .expect_err("non-TTY + no prior trust must fail");
    assert!(
        err.to_string().contains("non-interactive"),
        "expected non-interactive error, got: {err}"
    );

    // ------------------------------------------------------------------
    // 2. HATCH_TRUST_SCRIPTS=all – bypasses.
    // ------------------------------------------------------------------
    std::env::set_var("HATCH_TRUST_SCRIPTS", "all");
    let origin_allow = uniq_origin("allow_path");
    let res = check_script_trust(&origin_allow, "postinstall", "echo hi");
    std::env::remove_var("HATCH_TRUST_SCRIPTS");
    assert!(res.is_ok(), "HATCH_TRUST_SCRIPTS=all must bypass");
}

#[test]
fn root_origin_bypasses_trust_entirely() {
    let d = check_script_trust(&ScriptOrigin::Root, "build", "echo anything")
        .expect("root-origin scripts never consult TOFU");
    assert!(matches!(d, hatch::scripts::trust::TrustDecision::Allow));
}
