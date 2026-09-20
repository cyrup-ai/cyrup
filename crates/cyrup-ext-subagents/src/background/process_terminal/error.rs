//! [`ProcessTerminalError`] — one variant per upstream `throw`, each carrying upstream's sentence
//! byte for byte (pi `src/runs/background/process-terminal.ts:79`, `:83`, `:88`, `:91`, `:95`,
//! `:96`, `:97`, `:163`, `:165`, `:166`, `:168`, `:171`, `:172`, `:174`, `:176` @v0.68.0).
//!
//! # Why the sentences are deliverables
//!
//! None of them reaches a user as an error: every one is CAUGHT and folded into an
//! [`Unknown`](super::ProcessTerminal::Unknown) proof's `diagnostic`
//! (`process-terminal.ts:186`, `:197`, `:297`), which is the string an operator reads out of
//! `process-terminal.json` and out of `debug.run` when a run will not settle. A paraphrase makes
//! two builds' artifacts disagree about the same defect; the shape of these sentences is the
//! whole diagnostic.
//!
//! Modelled on [`RecoveryDescriptorError`](crate::background::recovery_descriptor::RecoveryDescriptorError),
//! which established this one-variant-per-upstream-throw shape for the same kind of on-disk
//! contract.

/// Every refusal the process-terminal readers can raise.
#[derive(Debug, thiserror::Error)]
pub enum ProcessTerminalError {
    /// pi `:79` — the candidate's own required fields are missing or wrongly typed.
    #[error("Invalid process-terminal candidate in '{dir}'.")]
    InvalidCandidate {
        /// The run directory the candidate was read from.
        dir: String,
    },

    /// pi `:83` — one step's `writers` entry is not an array of valid pi-writer exits.
    #[error("Invalid writer process records for child '{index}'.")]
    InvalidWriterRecords {
        /// The step index key, as it is spelled in the JSON object.
        index: String,
    },

    /// pi `:88` — `expectedWriters` is present but is not an object.
    #[error("Invalid expected writer process records.")]
    InvalidExpectedWriters,

    /// pi `:91` — one `expectedWriters` entry is not a non-negative integer.
    #[error("Invalid expected writer count for child '{index}'.")]
    InvalidExpectedWriterCount {
        /// The step index key, as it is spelled in the JSON object.
        index: String,
    },

    /// pi `:95` — `sessionFile` is present but is not a string.
    #[error("Invalid process-terminal candidate sessionFile.")]
    InvalidCandidateSessionFile,

    /// pi `:96` — `revivalLeaseToken` is present but is not a string.
    #[error("Invalid process-terminal candidate lease token.")]
    InvalidCandidateLeaseToken,

    /// pi `:97` — `revivalLeaseReleaseAcknowledged` is present but is not a boolean.
    #[error("Invalid process-terminal lease release acknowledgement.")]
    InvalidLeaseReleaseAcknowledgement,

    /// pi `:163` — the proof's own required fields are missing, wrongly typed, or its `state` is
    /// not one of the four words.
    #[error("Invalid process-terminal proof in '{label}'.")]
    InvalidProof {
        /// The run directory, or `"status"` for the overlay reader (pi's `label` default, `:180`).
        label: String,
    },

    /// pi `:165` — the proof belongs to a DIFFERENT run than the reader expected.
    #[error(
        "Process-terminal proof in '{label}' belongs to run '{actual}', expected '{expected}'."
    )]
    ProofRunMismatch {
        /// The run directory, or `"status"`.
        label: String,
        /// The run id the proof carries.
        actual: String,
        /// The run id the reader expected.
        expected: String,
    },

    /// pi `:166` — the proof belongs to a DIFFERENT runner instance than the reader expected.
    /// This is the check the whole [`RunnerProcessInstanceId`](super::RunnerProcessInstanceId)
    /// mint exists to make meaningful.
    #[error(
        "Process-terminal proof in '{label}' belongs to runner '{actual}', expected '{expected}'."
    )]
    ProofRunnerMismatch {
        /// The run directory, or `"status"`.
        label: String,
        /// The instance id the proof carries.
        actual: String,
        /// The instance id the reader expected.
        expected: String,
    },

    /// pi `:168` — `instances` is present but is not an array of valid instance exits.
    #[error("Invalid process-terminal instances in '{label}'.")]
    InvalidInstances {
        /// The run directory, or `"status"`.
        label: String,
    },

    /// pi `:171` — an `observed` proof without an `observedAt`.
    #[error("Observed process-terminal proof in '{label}' is missing observedAt.")]
    ObservedMissingObservedAt {
        /// The run directory, or `"status"`.
        label: String,
    },

    /// pi `:172` — an `observed` proof without an `instances` array.
    #[error("Observed process-terminal proof in '{label}' is missing instances.")]
    ObservedMissingInstances {
        /// The run directory, or `"status"`.
        label: String,
    },

    /// pi `:174` — an `observed` proof whose `instances` hold no runner exit matching its own
    /// `runnerProcessInstanceId`.
    #[error("Observed process-terminal proof in '{label}' has no matching runner instance.")]
    ObservedMissingRunnerInstance {
        /// The run directory, or `"status"`.
        label: String,
    },

    /// pi `:176` — `resumeDisposition` is present but is not one of the three words.
    #[error("Invalid process-terminal resume disposition in '{label}'.")]
    InvalidResumeDisposition {
        /// The run directory, or `"status"`.
        label: String,
    },

    /// Not an upstream sentence: upstream rethrows Node's own `SyntaxError`/`ErrnoException` from
    /// the same `catch` (`:110`, `:195`) and the message lands in the same `diagnostic`. This
    /// variant is that rethrow, with Rust's own reader message in place of Node's.
    #[error("Failed to read process-terminal record at '{path}': {source}")]
    Read {
        /// The file that could not be read.
        path: String,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },

    /// The JSON half of [`Self::Read`] — the same upstream `catch`, a different Rust error type.
    #[error("Failed to parse process-terminal record at '{path}': {source}")]
    Parse {
        /// The file that could not be parsed.
        path: String,
        /// The underlying decode failure.
        #[source]
        source: serde_json::Error,
    },
}
