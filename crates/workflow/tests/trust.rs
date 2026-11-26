//! Whose workflow a run is decides what an unattended run does without
//! `--approve`: one's own workflow runs its steps, a package's does not.

mod common;

use std::sync::Arc;

use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{MemoryStore, Services};
use common::{output, Harness};
use serde_json::{json, Value};

/// An unattended run without `--approve`, of one's own workflow or of a
/// package's.
fn unattended(trusted: bool) -> Harness {
    Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
        Services {
            trusted,
            ..Services::default()
        },
    )
}

const SHELL: &str = r#"
import { step, shell } from "codemod/workflow";
export default async function main() {
  const said = await step("Say", shell("echo hi"));
  return said.stdout.trim();
}
"#;

#[test]
fn ones_own_workflow_runs_its_shell_steps_unattended() {
    let report = unattended(true).start(SHELL, Value::Null);
    assert_eq!(output(&report).unwrap(), json!("hi"));
    assert!(report.run.settings.trusted, "recorded, for a resume");
}

#[test]
fn a_packages_shell_step_needs_approval_unattended() {
    let report = unattended(false).start(SHELL, Value::Null);
    let error = output(&report).unwrap_err();
    // The step, and the flag that allows it.
    assert!(error.contains("step 'Say'"), "{error}");
    assert!(error.contains("--approve=shell"), "{error}");
    assert!(!report.run.settings.trusted);
}

const PARENT: &str = r#"
import { step, codemod } from "codemod/workflow";
export default async function main() {
  const child = await step("Child", codemod("./child"));
  return child.output;
}
"#;

const CHILD: &str = r#"
import { step, shell } from "codemod/workflow";
export default async function main() {
  const said = await step("Say", shell("echo nested"));
  return said.stdout.trim();
}
"#;

/// A local codemod nested in one's own workflow is one's own too; nested in
/// a package's, it is the package's.
#[test]
fn a_local_nested_codemod_has_its_parents_trust() {
    let own = unattended(true);
    own.bundle_file("child/workflow.ts", CHILD);
    assert_eq!(
        output(&own.start(PARENT, Value::Null)).unwrap(),
        json!("nested")
    );

    let package = unattended(false);
    package.bundle_file("child/workflow.ts", CHILD);
    let error = output(&package.start(PARENT, Value::Null)).unwrap_err();
    assert!(error.contains("was not approved"), "{error}");
}

/// Trust answers what a step runs or sends, not a manual approval.
#[test]
fn ones_own_workflow_still_waits_at_a_manual_approval() {
    let report = unattended(true).start(
        r#"
import { step, approval } from "codemod/workflow";
export default async function main() { await step(approval("Ship it?")); }
"#,
        Value::Null,
    );
    assert_eq!(
        report.ending,
        codemod_workflow::Ending::AwaitingApproval("Ship it?".into())
    );
}
