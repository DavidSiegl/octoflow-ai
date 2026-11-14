//! The proof step's proofs (Pipeline::proof_step, Pipeline::proof_command): the records of their runs
//! (RunState::proofs) and the section of a review that says what they showed.

use crate::constants::PROOF_COPIES_DIR;
use crate::error::OrchestratorError;
use serde_json::Value;
use crate::json::{object, Object};

/// The heading of the section Workflow::judge_proofs appends to a review, which a Reviewer's own section is unlikely
/// to take.
pub const PROOFS_HEADING: &str = "## Proofs the orchestrator ran";
/// Seconds one proof may run.
pub const PROOF_TIMEOUT: u64 = 300;
/// How many lines of its output the review quotes.
pub const PROOF_TAIL_LINES: usize = 10;
/// A shell script that removes the directory "$1" names and everything under it, if it exists, failing as rm does.
/// Write permission is given back first, as far as chmod can: a proof may leave a directory it made read-only, as a
/// test of a permission error does.
pub const REMOVE_TREE_SCRIPT: &str = r#"{ [ ! -e "$1" ] || chmod -R u+rwX -- "$1" 2>/dev/null; }; rm -rf -- "$1""#;

/// The number of the finding a proof file is for: the first number in its name, as in test_3_rate.py; None for a name
/// without one.
pub fn proof_finding(file: &str) -> Option<u64> {
    let start = file.find(|c: char| c.is_ascii_digit())?;
    let mut n: u64 = 0;
    for c in file[start..].chars() {
        let Some(d) = c.to_digit(10) else { break };
        n = n.saturating_mul(10).saturating_add(d.into());
    }
    Some(n)
}

/// One run of a proof, as a RunState::proofs record holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofRun {
    /// The round it ran in.
    pub round: u32,
    /// The round whose proof step wrote the proof; equal to round for the proof's first run.
    pub written: u32,
    /// The proof's basename in RunState::proofs_dir(written).
    pub file: String,
    /// The finding it is for, the first number in its name (proof_finding); None for a name without one.
    pub finding: Option<u64>,
    /// Whether it failed against the change, which demonstrates its finding.
    pub demonstrated: bool,
    /// How it exited (proof_outcome) and, a line each below that, the last PROOF_TAIL_LINES lines it printed; or why it
    /// did not run.
    pub said: String,
    /// True means its file was gone from its directory when a rerun came to it, which is taken as the Builder having
    /// moved it into the project (PROOFS_NOTE): it was not run, and is carried no further. A record saved without the
    /// field, as every one was before v0.58.4, is false.
    pub moved: bool,
}

/// What one run of a proof found, before it is recorded as a ProofRun.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofOutcome {
    /// As ProofRun::demonstrated.
    pub demonstrated: bool,
    /// As ProofRun::said.
    pub said: String,
    /// As ProofRun::moved.
    pub moved: bool,
}

impl ProofRun {
    /// The run record holds; one without moved is not moved (ProofRun::record).
    ///
    /// Errors: a field missing or of another type than the engines write, as no released engine's state.json has
    /// (every record a format-2 state.json holds has each field, moved only when true).
    pub fn read(record: &Object) -> Result<ProofRun, OrchestratorError> {
        let field = |key: &str| {
            record.get(key).ok_or_else(|| OrchestratorError(format!("a proof run saved in state.json has no {key}")))
        };
        let invalid = |key: &str, value: &Value, what: &str| {
            OrchestratorError(format!("a proof run saved in state.json has {key} {}, not {what}", value))
        };
        let round_of = |key: &str| {
            let value = field(key)?;
            value.as_i64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| invalid(key, value, "a round"))
        };
        let text_of = |key: &str| {
            let value = field(key)?;
            value.as_str().map(String::from).ok_or_else(|| invalid(key, value, "a string"))
        };
        let (round, written) = (round_of("round")?, round_of("written")?);
        let finding = match field("finding")? {
            Value::Null => None,
            value => Some(value.as_i64().and_then(|n| u64::try_from(n).ok()).ok_or_else(|| invalid("finding", value, "a number"))?),
        };
        let demonstrated = field("demonstrated")?;
        let demonstrated = demonstrated.as_bool().ok_or_else(|| invalid("demonstrated", demonstrated, "true or false"))?;
        let moved = match record.get("moved") {
            None => false,
            Some(flag) => flag.as_bool().ok_or_else(|| invalid("moved", flag, "true or false"))?,
        };
        Ok(ProofRun { round, written, file: text_of("file")?, finding, demonstrated, said: text_of("said")?, moved })
    }

    /// The run as a RunState::proofs record.
    ///
    /// moved is written only when true, so a record of a proof that is not moved is as a release without the field
    /// writes it, and such a release reads one that is moved, taking it for an erroring proof.
    pub fn record(&self) -> Object {
        let mut record = object([
            ("round", Value::from(self.round)),
            ("written", Value::from(self.written)),
            ("file", Value::from(self.file.as_str())),
            ("finding", Value::from(self.finding)),
            ("demonstrated", Value::Bool(self.demonstrated)),
            ("said", Value::from(self.said.as_str())),
        ]);
        if self.moved {
            record.insert("moved".to_string(), Value::Bool(true));
        }
        record
    }

    /// Whether the run passed: exited 0, neither failing nor erroring.
    pub fn passed(&self) -> bool {
        proof_outcome(&self.said) == "it passes"
    }
}

/// How a proof exited, from a record's said: "it fails", "it passes" or how it errored.
pub fn proof_outcome(said: &str) -> &str {
    said.split('\n').next().unwrap_or_default().trim_end_matches(':')
}

/// The lines a proof printed last, from a record's said, fenced and indented under its list item; none when it
/// printed nothing.
pub fn proof_output(said: &str) -> Vec<String> {
    let Some((_, printed)) = said.split_once('\n') else { return Vec::new() };
    let mut out = vec![String::new(), "  ````".to_string()];
    out.extend(printed.split('\n').map(|line| format!("  {line}")));
    out.push("  ````".to_string());
    out
}

/// The section Workflow::judge_proofs appends to a review: for each of proofs, the round's first runs in
/// RunState::proofs, whether it demonstrates its finding, and the review's numbered findings left without one, which
/// stand as unproven. command ran each in cwd.
pub fn proofs_section(command: &str, cwd: &str, review: &str, proofs: &[ProofRun]) -> String {
    let mut out = vec![
        PROOFS_HEADING.to_string(),
        String::new(),
        format!(
            "The orchestrator ran each proof with `{command}` in {cwd}, from a copy of its directory under \
             `{cwd}/{PROOF_COPIES_DIR}/`, removed after each run; a path to a proof in its output is the copy's. A \
             proof that fails demonstrates its finding; one that passes or errors does not."
        ),
    ];
    for p in proofs {
        let finding = p.finding.map_or("no finding".to_string(), |n| format!("finding {n}"));
        let demonstrated = if p.demonstrated { "demonstrated" } else { "not demonstrated" };
        out.push(String::new());
        out.push(format!("- `{}` ({finding}): {demonstrated}, {}.", p.file, proof_outcome(&p.said)));
        out.extend(proof_output(&p.said));
    }
    let proven: Vec<Option<i128>> = proofs.iter().map(|p| p.finding.map(i128::from)).collect();
    let unproven: Vec<String> = numbered_findings(review)
        .into_iter()
        .filter(|k| !proven.contains(&Some(*k)))
        .map(|k| k.to_string())
        .collect();
    if !unproven.is_empty() {
        out.push(String::new());
        out.push(format!("Findings without a proof, unproven: {}.", unproven.join(", ")));
    }
    out.join("\n") + "\n"
}

/// The number of each line of review that starts with one and ". ", as re.findall(r"^(\d+)\. ", review,
/// re.MULTILINE) finds them: lines end at "\n" alone, and the digits are ASCII ones.
fn numbered_findings(review: &str) -> Vec<i128> {
    review
        .split('\n')
        .filter_map(|line| {
            let digits: Vec<u32> = line.chars().map_while(|c| c.to_digit(10)).collect();
            let rest = &line[line.char_indices().nth(digits.len()).map_or(line.len(), |(i, _)| i)..];
            (!digits.is_empty() && rest.starts_with(". "))
                .then(|| digits.iter().fold(0i128, |n, d| n.saturating_mul(10).saturating_add((*d).into())))
        })
        .collect()
}

#[cfg(test)]
mod tests;
