//! The completion-settlement decision — pi's three unawaited-work checks inside `finish`
//! (`scripted-workflow.ts:1832-1846`): a workflow that COMPLETED (returned a value) but left a
//! `runs.run` launch, `runs.steer` call, or `runs.host` call unobserved fails with a verbatim
//! re-prompt instead of resolving.
//!
//! # Precedence is a decision enum, not statement order
//!
//! Upstream expresses run ▸ steer ▸ host precedence as the order of three checks across an
//! if/else chain (`:1834-1841`). SCOPE_3 §A.1's last row forbids that shape here: the precedence
//! lives in ONE pure function returning ONE enum, and the engine's shell merely formats it.
//!
//! # Better than upstream, and why
//!
//! Upstream burns `promiseHooks`, three `Proxy`s, seven `WeakMap`s and a `Promise.prototype.then`
//! patch (963 lines) to infer *"did the script await this launch?"* from promise-graph topology,
//! because Node cannot see inside V8's promises. Here the host ISSUED handle `n` and knows whether
//! `result(n)` was ever read: the whole apparatus is a `bool` per handle, checked at settlement.
//! The error still names each launch with upstream's exact bytes; the launch's POSITION (its
//! ordinal and `started` trace entry) is recoverable from the returned partial's trace, where
//! upstream can only name the key.

/// The settlement decision for a workflow that completed with a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionSettlement {
    /// Every launch, steer and host call was observed — resolve with the value.
    Clean,
    /// ≥1 `runs.run` launch was never observed (highest precedence, `:1834`).
    UnawaitedRuns(Vec<String>),
    /// No unawaited launches, but ≥1 `runs.steer` call was never observed (`:1839`).
    UnawaitedSteers(Vec<String>),
    /// No unawaited launches or steers, but ≥1 `runs.host` call was never observed (`:1841`).
    UnawaitedHosts(Vec<String>),
}

impl CompletionSettlement {
    /// The one pure decision: given the three unobserved sets IN LAUNCH ORDER, which (if any)
    /// settlement error fires. Precedence: runs ▸ steers ▸ hosts — a workflow with all three
    /// reports only the launches, exactly as upstream's chain does.
    #[must_use]
    pub fn decide(
        unobserved_runs: Vec<String>,
        unobserved_steers: Vec<String>,
        unobserved_hosts: Vec<String>,
    ) -> Self {
        if !unobserved_runs.is_empty() {
            return Self::UnawaitedRuns(unobserved_runs);
        }
        if !unobserved_steers.is_empty() {
            return Self::UnawaitedSteers(unobserved_steers);
        }
        if !unobserved_hosts.is_empty() {
            return Self::UnawaitedHosts(unobserved_hosts);
        }
        Self::Clean
    }

    /// The verbatim completion-error text, or `None` for a clean settlement. `'key'` quoting and
    /// `", "` joins are upstream's (`:1834`, `:1839`, `:1841`).
    #[must_use]
    pub fn message(&self) -> Option<String> {
        fn quoted(keys: &[String]) -> String {
            keys.iter()
                .map(|key| format!("'{key}'"))
                .collect::<Vec<_>>()
                .join(", ")
        }
        match self {
            Self::Clean => None,
            Self::UnawaitedRuns(keys) => Some(format!(
                "workflowScript completed with unawaited runs.run launch(es): {}. For ordinary parallel fanout use await runs.all([{{key, agent, task}}, ...]); do not read .output from unawaited launches.",
                quoted(keys)
            )),
            Self::UnawaitedSteers(keys) => Some(format!(
                "workflowScript completed with unawaited runs.steer call(s): {}. Await or return each call.",
                quoted(keys)
            )),
            Self::UnawaitedHosts(keys) => Some(format!(
                "workflowScript completed with unawaited runs.host call(s): {}. Await or return each call.",
                quoted(keys)
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::CompletionSettlement;

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn clean_when_everything_was_observed() {
        let decision = CompletionSettlement::decide(vec![], vec![], vec![]);
        assert_eq!(decision, CompletionSettlement::Clean);
        assert_eq!(decision.message(), None);
    }

    #[test]
    fn runs_beat_steers_beat_hosts() {
        let decision = CompletionSettlement::decide(keys(&["a", "b"]), keys(&["s"]), keys(&["h"]));
        assert_eq!(
            decision,
            CompletionSettlement::UnawaitedRuns(keys(&["a", "b"]))
        );
        let decision = CompletionSettlement::decide(vec![], keys(&["s"]), keys(&["h"]));
        assert_eq!(
            decision,
            CompletionSettlement::UnawaitedSteers(keys(&["s"]))
        );
        let decision = CompletionSettlement::decide(vec![], vec![], keys(&["h"]));
        assert_eq!(decision, CompletionSettlement::UnawaitedHosts(keys(&["h"])));
    }

    #[test]
    fn messages_are_byte_verbatim() {
        assert_eq!(
            CompletionSettlement::decide(keys(&["a", "b"]), vec![], vec![])
                .message()
                .unwrap(),
            "workflowScript completed with unawaited runs.run launch(es): 'a', 'b'. For ordinary parallel fanout use await runs.all([{key, agent, task}, ...]); do not read .output from unawaited launches."
        );
        assert_eq!(
            CompletionSettlement::decide(vec![], keys(&["s1"]), vec![])
                .message()
                .unwrap(),
            "workflowScript completed with unawaited runs.steer call(s): 's1'. Await or return each call."
        );
        assert_eq!(
            CompletionSettlement::decide(vec![], vec![], keys(&["gate"]))
                .message()
                .unwrap(),
            "workflowScript completed with unawaited runs.host call(s): 'gate'. Await or return each call."
        );
    }
}
