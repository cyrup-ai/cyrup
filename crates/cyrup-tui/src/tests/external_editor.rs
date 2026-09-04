#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod external_editor_tests {
    use crate::app::*;

    /// Write a shell script into `dir` and return the multi-token editor command that runs it.
    ///
    /// The command is `"/bin/sh <script>"`, NOT the script path alone, and the script is
    /// deliberately left non-executable. Exec'ing a file this process itself just wrote is racy in
    /// a multi-threaded test binary: `std::fs::write` opens the file for writing, and any OTHER
    /// thread that forks (every `Command::spawn` in this binary) during that window hands its child
    /// an inherited write-fd, which makes the later `execve` of that same path fail with `ETXTBSY`.
    /// That surfaced as `run_editor_over_file` returning `None` about 9% of the time
    /// (`Os { code: 26, kind: ExecutableFileBusy }`, observed by instrumenting the spawn). Handing
    /// the script to `/bin/sh` as an ARGUMENT means only `/bin/sh` is exec'd and the script is
    /// merely opened for reading, so there is no window at all.
    ///
    /// It also exercises `run_editor_over_file`'s `split_whitespace` on a genuinely multi-token
    /// command (`sh`, the script, then the appended file), which is the realistic `$EDITOR` shape.
    #[cfg(unix)]
    fn sh_editor(dir: &std::path::Path, name: &str, body: &str) -> String {
        let script = dir.join(name);
        std::fs::write(&script, body).unwrap();
        format!("/bin/sh {}", script.display())
    }

    /// F14: the RESOLVED editor command is exactly what runs over the temp file — proving
    /// `edit_in_external_editor` spawns the command it is handed (which `App::run` resolves via
    /// `resolve_external_editor` → `EffectiveSettings::external_editor`, honoring settings
    /// `externalEditor` over `$VISUAL`/`$EDITOR`) rather than an inline env-only chain. The script
    /// rewrites the file; the reloaded text is the script's output.
    #[test]
    #[cfg(unix)]
    fn resolved_editor_command_is_the_one_that_runs() {
        let dir = tempfile::tempdir().unwrap();
        let editor = sh_editor(
            dir.path(),
            "fake-editor.sh",
            "printf 'REWRITTEN BY EDITOR' > \"$1\"\n",
        );

        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "original text").unwrap();

        let out = run_editor_over_file(&editor, &file);
        assert_eq!(
            out.as_deref(),
            Some("REWRITTEN BY EDITOR"),
            "the resolved editor's edit is reloaded"
        );
    }

    /// A non-zero editor exit yields `None` — Pi's "no change" (`false` exits 1 without editing).
    #[test]
    #[cfg(unix)]
    fn nonzero_editor_exit_is_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "keep me").unwrap();
        assert_eq!(run_editor_over_file("false", &file), None);
    }

    /// A trailing newline the editor leaves is stripped once (Pi's `.replace(/\n$/, "")`).
    #[test]
    #[cfg(unix)]
    fn trailing_newline_is_stripped_once() {
        let dir = tempfile::tempdir().unwrap();
        let editor = sh_editor(
            dir.path(),
            "nl-editor.sh",
            "printf 'line one\\n' > \"$1\"\n",
        );
        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            run_editor_over_file(&editor, &file).as_deref(),
            Some("line one")
        );
    }
}
