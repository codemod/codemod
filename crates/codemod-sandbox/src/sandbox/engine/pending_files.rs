//! Files a step has changed but not written yet.
//!
//! A step's transforms run on several threads and nothing is written until
//! the step commits, so two transforms that both change one file with
//! `jssgTransform` would each start from the text on disk and one change
//! would be lost. Reading and updating such a file through [`PendingFiles`]
//! gives it to one transform at a time and hands each the text the step has
//! so far.
//!
//! A thread may also open a [`Transaction`]: the files it takes then stay
//! its own until the transaction ends, and are put back as they were if the
//! transaction does not keep its changes (the transform failed). A file's
//! changes to other files are then applied with its own, or not at all.
//!
//! Each take of a file gets its own token. A guard whose take has ended
//! (its transaction finished while a transform still had it, such as an
//! unawaited `jssgTransform`) changes nothing any more.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// How long a transform waits for a file another transform holds. Waits
/// that could never end are found and refused without waiting.
const WAIT_LIMIT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Pending {
    original: String,
    text: String,
    /// The thread whose transform has the file now.
    holder: Option<ThreadId>,
    /// A transform is running on the file now: the holder may not take it
    /// again until it is done.
    busy: bool,
    /// The text when the holder's transaction took the file.
    before: String,
    /// The take that has the file now; 0 when none has.
    token: u64,
}

#[derive(Debug, Default)]
struct State {
    files: HashMap<PathBuf, Pending>,
    /// Threads with an open transaction.
    open: HashSet<ThreadId>,
    /// The file each thread is waiting for.
    waiting: HashMap<ThreadId, PathBuf>,
    /// Transactions that were refused a file to break a cycle of waits: the
    /// thread that held it, and how many transactions that thread had ended.
    aborted: HashMap<ThreadId, (ThreadId, u64)>,
    /// How many transactions each thread has ended.
    ended: HashMap<ThreadId, u64>,
    /// The files each open transaction holds.
    held: HashMap<ThreadId, Vec<PathBuf>>,
    /// The last token given to a take.
    tokens: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PendingFiles {
    inner: Arc<(Mutex<State>, Condvar)>,
    /// Where a file's text comes from before the step changed it, when not
    /// from disk: see [`PendingFiles::with_base`].
    base: Option<Base>,
}

/// A file's text before the step changed it, for the files the caller has
/// other text for; `None` for those read from disk.
type BaseFn = dyn Fn(&Path) -> Option<std::io::Result<String>> + Send + Sync;

#[derive(Clone)]
struct Base(Arc<BaseFn>);

impl std::fmt::Debug for Base {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Base")
    }
}

/// One file taken with [`PendingFiles::take`]; gives it back when dropped,
/// or, in a transaction, when the transaction ends.
#[derive(Debug)]
pub struct TakenFile {
    files: PendingFiles,
    path: PathBuf,
    token: u64,
}

/// The files one thread takes until [`Transaction::finish`]. Dropped
/// unfinished, it keeps none of their changes.
#[derive(Debug)]
pub struct Transaction {
    files: PendingFiles,
    thread: ThreadId,
    done: bool,
}

impl State {
    /// Whether `me` waiting for `key` would wait forever: its holder waits,
    /// directly or through others, for a file `me` holds.
    fn would_deadlock(&self, me: ThreadId, key: &Path) -> bool {
        let mut holder = self.files.get(key).and_then(|file| file.holder);
        for _ in 0..=self.waiting.len() {
            match holder {
                None => return false,
                Some(thread) if thread == me => return true,
                Some(thread) => {
                    holder = self
                        .waiting
                        .get(&thread)
                        .and_then(|path| self.files.get(path))
                        .and_then(|file| file.holder);
                }
            }
        }
        false
    }

    fn next_token(&mut self) -> u64 {
        self.tokens += 1;
        self.tokens
    }

    /// Give back one file `thread` holds, putting back its text from when
    /// the holder took it unless `keep`.
    fn release_one(&mut self, path: &Path, thread: ThreadId, keep: bool) {
        let unchanged = match self.files.get_mut(path) {
            Some(file) if file.holder == Some(thread) => {
                if !keep {
                    file.text = std::mem::take(&mut file.before);
                }
                file.holder = None;
                file.busy = false;
                file.token = 0;
                file.before.clear();
                file.text == file.original
            }
            _ => false,
        };
        // A file left as it was needs no keeping: the next take reads it again.
        if unchanged {
            self.files.remove(path);
        }
    }

    /// Give back what `thread`'s transaction holds, putting back the texts
    /// if `keep` is false. Only those files are visited.
    fn release(&mut self, thread: ThreadId, keep: bool) {
        for path in self.held.remove(&thread).unwrap_or_default() {
            self.release_one(&path, thread, keep);
        }
    }
}

impl PendingFiles {
    /// Files whose text before the step comes from `base` when it has the
    /// file (a dry run's earlier steps changed it, in memory), else from
    /// disk. `base` gets the path a file is kept under: canonical when the
    /// file is on disk.
    pub fn with_base(
        base: impl Fn(&Path) -> Option<std::io::Result<String>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::default(),
            base: Some(Base(Arc::new(base))),
        }
    }

    /// A file's text as the step started: what analyses answer for, even
    /// after the step changed it.
    pub fn text_at_step_start(&self, path: &Path) -> std::io::Result<String> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.read_base(&key)
    }

    /// A file's text before the step changed it.
    fn read_base(&self, key: &Path) -> std::io::Result<String> {
        match self.base.as_ref().and_then(|base| (base.0)(key)) {
            Some(text) => text,
            None => std::fs::read_to_string(key),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The file's text as the step has it so far, and the file itself until
    /// the returned guard is dropped (or the thread's transaction ends).
    /// Waits while another transform has it.
    pub fn take(&self, path: &Path) -> std::io::Result<(String, TakenFile)> {
        self.take_inner(path, false)
    }

    /// [`Self::take`], where a file that does not exist yet is taken as
    /// empty: setting its text creates it when the step commits.
    pub fn take_creating(&self, path: &Path) -> std::io::Result<(String, TakenFile)> {
        self.take_inner(path, true)
    }

    fn take_inner(&self, path: &Path, create: bool) -> std::io::Result<(String, TakenFile)> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let me = std::thread::current().id();
        let mut state = self.lock();
        if let Some(file) = state.files.get_mut(&key) {
            if file.holder == Some(me) {
                if file.busy {
                    return Err(std::io::Error::other(format!(
                        "'{}' is being transformed already; return its new text from that transform instead",
                        path.display()
                    )));
                }
                // Taken earlier in this transaction, and free again.
                file.busy = true;
                let text = file.text.clone();
                let token = state.next_token();
                if let Some(file) = state.files.get_mut(&key) {
                    file.token = token;
                }
                drop(state);
                return Ok((text, self.taken(key, token)));
            }
        }
        let deadline = Instant::now() + WAIT_LIMIT;
        while state
            .files
            .get(&key)
            .is_some_and(|file| file.holder.is_some())
        {
            if state.would_deadlock(me, &key) {
                if let Some(holder) = state.files.get(&key).and_then(|file| file.holder) {
                    let ended = state.ended.get(&holder).copied().unwrap_or(0);
                    state.aborted.insert(me, (holder, ended));
                }
                return Err(std::io::Error::other(format!(
                    "'{}' is held by another file's transform, which is waiting for a file this one holds",
                    path.display()
                )));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(std::io::Error::other(format!(
                    "timed out waiting for another transform of '{}'",
                    path.display()
                )));
            }
            state.waiting.insert(me, key.clone());
            state = self
                .inner
                .1
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            state.waiting.remove(&me);
        }
        let token = state.next_token();
        let text = match state.files.get_mut(&key) {
            Some(file) => {
                file.holder = Some(me);
                file.busy = true;
                file.token = token;
                file.before = file.text.clone();
                file.text.clone()
            }
            None => {
                let text = match self.read_base(&key) {
                    Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => String::new(),
                    other => other?,
                };
                state.files.insert(
                    key.clone(),
                    Pending {
                        original: text.clone(),
                        text: text.clone(),
                        holder: Some(me),
                        busy: true,
                        before: text.clone(),
                        token,
                    },
                );
                text
            }
        };
        if state.open.contains(&me) {
            state.held.entry(me).or_default().push(key.clone());
        }
        drop(state);
        Ok((text, self.taken(key, token)))
    }

    fn taken(&self, path: PathBuf, token: u64) -> TakenFile {
        TakenFile {
            files: self.clone(),
            path,
            token,
        }
    }

    /// Keep the files this thread takes until [`Transaction::finish`].
    pub fn begin(&self) -> Transaction {
        let thread = std::thread::current().id();
        let mut state = self.lock();
        state.open.insert(thread);
        state.aborted.remove(&thread);
        Transaction {
            files: self.clone(),
            thread,
            done: false,
        }
    }

    /// Write every changed file no transform holds now with `write`, and
    /// forget it: the next take reads it from disk again, so writes made
    /// meanwhile by other means (`root.write()`) are seen. Done under the
    /// lock, so no transform takes a file between its write and its
    /// eviction. A file `write` fails on stays pending; the errors are
    /// returned with their paths.
    pub fn write_released(
        &self,
        mut write: impl FnMut(&Path, &str) -> std::io::Result<()>,
    ) -> Vec<(PathBuf, std::io::Error)> {
        let mut state = self.lock();
        let mut ready: Vec<PathBuf> = state
            .files
            .iter()
            .filter(|(_, file)| file.holder.is_none() && file.text != file.original)
            .map(|(path, _)| path.clone())
            .collect();
        ready.sort();
        let mut errors = Vec::new();
        for path in ready {
            let Some(file) = state.files.get(&path) else {
                continue;
            };
            match write(&path, &file.text) {
                Ok(()) => {
                    state.files.remove(&path);
                }
                Err(error) => errors.push((path, error)),
            }
        }
        errors
    }

    /// Every file whose text the step changed: `(path, at the start of the
    /// step, now)`, by path. The first text is the disk's, or in a dry run
    /// what earlier steps left.
    pub fn changed(&self) -> Vec<(PathBuf, String, String)> {
        let mut changed: Vec<_> = self
            .lock()
            .files
            .iter()
            .filter(|(_, file)| file.text != file.original)
            .map(|(path, file)| (path.clone(), file.original.clone(), file.text.clone()))
            .collect();
        changed.sort();
        changed
    }
}

impl TakenFile {
    /// The path the file is kept under.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether this take still has the file: its transaction has not ended.
    pub fn is_current(&self) -> bool {
        self.files
            .lock()
            .files
            .get(&self.path)
            .is_some_and(|file| file.token == self.token)
    }

    /// The file's new text, for whoever takes it next and for the commit.
    /// Gives false, and changes nothing, when the take has ended.
    pub fn set(&self, text: String) -> bool {
        match self.files.lock().files.get_mut(&self.path) {
            Some(file) if file.token == self.token => {
                file.text = text;
                true
            }
            _ => false,
        }
    }
}

impl Drop for TakenFile {
    fn drop(&mut self) {
        let me = std::thread::current().id();
        let mut state = self.files.lock();
        let in_transaction = state.open.contains(&me);
        let Some(file) = state
            .files
            .get_mut(&self.path)
            .filter(|file| file.token == self.token)
        else {
            // Its take has ended.
            return;
        };
        if in_transaction {
            // Free for the transaction to take again; the transaction gives it back.
            file.busy = false;
            file.token = 0;
            return;
        }
        state.release_one(&self.path, me, true);
        drop(state);
        self.files.inner.1.notify_all();
    }
}

impl Transaction {
    /// Whether another transaction was given a file instead of this one, to
    /// break a cycle of waits: what this one did is incomplete, and should
    /// be dropped and done again.
    pub fn aborted(&self) -> bool {
        self.files.lock().aborted.contains_key(&self.thread)
    }

    /// Whether a transform still has one of the transaction's files (an
    /// unawaited `jssgTransform`): what it does next would come too late.
    pub fn unsettled(&self) -> bool {
        let state = self.files.lock();
        state.held.get(&self.thread).is_some_and(|paths| {
            paths
                .iter()
                .any(|path| state.files.get(path).is_some_and(|file| file.busy))
        })
    }

    /// Put back what an aborted transaction changed, and wait until the
    /// transaction it gave way to is done, so running it again does not
    /// take the files back first.
    pub fn give_way(mut self) {
        let winner = self.files.lock().aborted.get(&self.thread).copied();
        self.end(false);
        let Some((thread, ended)) = winner else {
            return;
        };
        let state = self.files.lock();
        let _ = self
            .files
            .inner
            .1
            .wait_timeout_while(state, WAIT_LIMIT, |state| {
                state.ended.get(&thread).copied().unwrap_or(0) <= ended
            });
    }

    /// Give back every file taken since [`PendingFiles::begin`], keeping
    /// their new texts or putting back the ones they had.
    pub fn finish(mut self, keep: bool) {
        self.end(keep);
    }

    fn end(&mut self, keep: bool) {
        if self.done {
            return;
        }
        self.done = true;
        let mut state = self.files.lock();
        state.open.remove(&self.thread);
        state.aborted.remove(&self.thread);
        *state.ended.entry(self.thread).or_default() += 1;
        state.release(self.thread, keep);
        drop(state);
        self.files.inner.1.notify_all();
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        self.end(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_to_one_file_from_several_threads_add_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.ts");
        std::fs::write(&path, "").unwrap();
        let files = PendingFiles::default();
        std::thread::scope(|scope| {
            for line in 0..8 {
                let (files, path) = (files.clone(), path.clone());
                scope.spawn(move || {
                    let (text, taken) = files.take(&path).unwrap();
                    std::thread::yield_now();
                    taken.set(format!("{text}export * from './{line}';\n"));
                });
            }
        });
        let changed = files.changed();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].1, "");
        assert_eq!(changed[0].2.lines().count(), 8);
        // Nothing reached the disk.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn taking_a_file_twice_on_one_thread_fails_instead_of_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ts");
        std::fs::write(&path, "x").unwrap();
        let files = PendingFiles::default();
        let (_, _taken) = files.take(&path).unwrap();
        let error = files.take(&path).unwrap_err().to_string();
        assert!(error.contains("being transformed already"), "{error}");
    }

    #[test]
    fn a_file_left_as_it_was_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ts");
        std::fs::write(&path, "x").unwrap();
        let files = PendingFiles::default();
        drop(files.take(&path).unwrap());
        assert!(files.changed().is_empty());
    }

    #[test]
    fn a_transaction_keeps_its_files_until_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ts");
        std::fs::write(&path, "0").unwrap();
        let files = PendingFiles::default();
        let transaction = files.begin();
        let (text, taken) = files.take(&path).unwrap();
        taken.set(format!("{text}1"));
        drop(taken);
        // The same transaction may take it again, and builds on its change.
        let (text, taken) = files.take(&path).unwrap();
        assert_eq!(text, "01");
        taken.set(format!("{text}2"));
        drop(taken);
        let other = {
            let (files, path) = (files.clone(), path.clone());
            std::thread::spawn(move || {
                let (text, taken) = files.take(&path).unwrap();
                taken.set(format!("{text}3"));
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        // The other thread waits for the transaction.
        assert_eq!(files.changed()[0].2, "012");
        transaction.finish(true);
        other.join().unwrap();
        assert_eq!(files.changed()[0].2, "0123");
    }

    #[test]
    fn a_transaction_that_does_not_keep_its_changes_puts_the_texts_back() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.ts"), dir.path().join("b.ts"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let files = PendingFiles::default();
        let (text, taken) = files.take(&a).unwrap();
        taken.set(format!("{text}1"));
        drop(taken);

        let transaction = files.begin();
        for path in [&a, &b] {
            let (text, taken) = files.take(path).unwrap();
            taken.set(format!("{text}2"));
        }
        drop(transaction);
        let changed = files.changed();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].2, "a1");
    }

    #[test]
    fn a_cycle_of_waits_is_refused_and_aborts_the_transaction_that_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.ts"), dir.path().join("b.ts"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let files = PendingFiles::default();
        let barrier = std::sync::Barrier::new(2);
        let aborted: Vec<bool> = std::thread::scope(|scope| {
            let handles: Vec<_> = [(&a, &b, "1"), (&b, &a, "2")]
                .into_iter()
                .map(|(mine, theirs, mark)| {
                    let (files, barrier) = (&files, &barrier);
                    scope.spawn(move || {
                        let transaction = files.begin();
                        let (text, held) = files.take(mine).unwrap();
                        held.set(format!("{text}{mark}"));
                        barrier.wait();
                        if let Ok((text, other)) = files.take(theirs) {
                            other.set(format!("{text}{mark}"));
                        }
                        let aborted = transaction.aborted();
                        drop(held);
                        if aborted {
                            // Returns once the other transaction is done.
                            transaction.give_way();
                            assert_eq!(files.changed().len(), 2);
                        } else {
                            transaction.finish(true);
                        }
                        aborted
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        // One gave way and kept nothing; the other changed both files.
        assert_eq!(aborted.iter().filter(|aborted| **aborted).count(), 1);
        let changed = files.changed();
        assert_eq!(changed.len(), 2);
        let mark = if aborted[0] { "2" } else { "1" };
        assert_eq!(changed[0].2, format!("a{mark}"));
        assert_eq!(changed[1].2, format!("b{mark}"));
    }

    /// A guard that outlives its transaction (an unawaited `jssgTransform`)
    /// changes nothing once the file is taken again, by another thread or
    /// by the next transaction on this one.
    #[test]
    fn a_guard_that_outlives_its_transaction_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ts");
        std::fs::write(&path, "0").unwrap();
        let files = PendingFiles::default();

        let transaction = files.begin();
        let (_, late) = files.take(&path).unwrap();
        assert!(transaction.unsettled());
        transaction.finish(false);
        assert!(!late.is_current());
        assert!(!late.set("late".into()));

        let transaction = files.begin();
        let (text, taken) = files.take(&path).unwrap();
        assert!(!late.set("late".into()));
        drop(late);
        // The stale guard did not free the file for a second take.
        assert!(files
            .take(&path)
            .unwrap_err()
            .to_string()
            .contains("being transformed already"));
        assert!(taken.set(format!("{text}1")));
        drop(taken);
        assert!(!transaction.unsettled());
        transaction.finish(true);
        assert_eq!(files.changed()[0].2, "01");
    }
}
