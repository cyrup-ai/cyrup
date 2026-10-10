//! The files this process wrote so that the model can read them back.
//!
//! A `bash` result that was cut short ends in `Full output: /tmp/cyrup-bash-<id>.log`, and a
//! `codemode` result that was cut ends in `Full output: /tmp/pi-codemode-<id>.txt (read or
//! tools.read with offset/limit)`; a script's `image()` leaves `/tmp/pi-codemode-<id>.png`. Those
//! paths are offered to the model unconditionally, and they are outside the project. A permission
//! policy that guards reads outside the working directory therefore blocked, or asked about, the
//! very file the tool had just told the model to read.
//!
//! Whoever writes such a file [`record`]s its path, and the permission system asks
//! [`any_recorded`] before it applies the external-directory guard to a `read`. It is the path that
//! was recorded, exactly, and nothing near it: a name that merely looks like a spill file, in the
//! same directory, is not one. The record lives as long as the process, which is as long as the
//! session that wrote the files and the model that was told about them; a subagent child is its own
//! process with its own record.
//!
//! No I/O happens here. The record only says which paths were handed to the model.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// How many paths are remembered. A spill file is written when a command's output or a script's
/// result overflows its limit, so a long session records one per such call; past this the oldest
/// path is forgotten and a `read` of it meets the permission policy like any file outside the
/// project.
const CAPACITY: usize = 1024;

/// The remembered paths, oldest first. A type of its own so the eviction can be tested with a small
/// capacity on a local value, without filling the process-wide record other tests read.
struct Recorded(VecDeque<PathBuf>);

impl Recorded {
    const fn new() -> Self {
        Self(VecDeque::new())
    }

    fn push(&mut self, path: &Path, capacity: usize) {
        if self.0.iter().any(|known| known == path) {
            return;
        }
        if self.0.len() >= capacity {
            self.0.pop_front();
        }
        self.0.push_back(path.to_path_buf());
    }

    fn any(&self, mut matches: impl FnMut(&Path) -> bool) -> bool {
        self.0.iter().any(|path| matches(path))
    }
}

static RECORDED: Mutex<Recorded> = Mutex::new(Recorded::new());

/// Remember that this process wrote `path` for the model to read. Recording the same path again
/// keeps one entry.
pub fn record(path: &Path) {
    RECORDED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(path, CAPACITY);
}

/// Whether `matches` holds for any recorded path. `matches` runs under the record's lock and must
/// not call back into [`record`].
pub fn any_recorded(matches: impl FnMut(&Path) -> bool) -> bool {
    RECORDED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .any(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(path: &Path) -> bool {
        any_recorded(|known| known == path)
    }

    #[test]
    fn a_recorded_path_is_found_and_a_neighbour_is_not() {
        let spill = Path::new("/spilled-files-test/a/pi-codemode-0000000000000001.txt");
        record(spill);
        assert!(has(spill));
        assert!(!has(Path::new(
            "/spilled-files-test/a/pi-codemode-0000000000000002.txt"
        )));
    }

    #[test]
    fn recording_twice_keeps_one_entry() {
        let mut recorded = Recorded::new();
        let spill = Path::new("/spilled-files-test/b/once.log");
        recorded.push(spill, 8);
        recorded.push(spill, 8);
        assert_eq!(recorded.0.len(), 1);
    }

    #[test]
    fn the_oldest_path_is_forgotten_past_the_capacity() {
        let mut recorded = Recorded::new();
        for n in 0..4 {
            recorded.push(Path::new(&format!("/spilled-files-test/c/{n}.log")), 3);
        }
        assert!(!recorded.any(|known| known == Path::new("/spilled-files-test/c/0.log")));
        assert!(recorded.any(|known| known == Path::new("/spilled-files-test/c/1.log")));
        assert!(recorded.any(|known| known == Path::new("/spilled-files-test/c/3.log")));
        assert_eq!(recorded.0.len(), 3);
    }
}
