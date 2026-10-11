// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::{Access, Control, SearchError};
use globset::{GlobBuilder, GlobMatcher};
use ignore::{WalkBuilder, WalkState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub type Authorizer = Arc<dyn Fn(&Path, Access) -> Result<(), String> + Send + Sync>;
pub type Cancelled = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub struct Hooks {
    pub authorize: Authorizer,
    pub cancelled: Cancelled,
}

impl Hooks {
    pub fn unrestricted() -> Self {
        Self {
            authorize: Arc::new(|_, _| Ok(())),
            cancelled: Arc::new(|| false),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub enum Compare {
    Less,
    LessEqual,
    Equal,
    NotEqual,
    GreaterEqual,
    Greater,
}

impl Compare {
    pub fn test<T: Ord>(&self, actual: T, expected: T) -> bool {
        match self {
            Self::Less => actual < expected,
            Self::LessEqual => actual <= expected,
            Self::Equal => actual == expected,
            Self::NotEqual => actual != expected,
            Self::GreaterEqual => actual >= expected,
            Self::Greater => actual > expected,
        }
    }
}

#[derive(Clone)]
pub struct NamePredicate {
    matcher: GlobMatcher,
    pub negated: bool,
}

impl NamePredicate {
    pub fn compile(
        pattern: &str,
        negated: bool,
        case_insensitive: bool,
    ) -> Result<Self, SearchError> {
        let matcher = GlobBuilder::new(pattern)
            .backslash_escape(true)
            .case_insensitive(case_insensitive)
            .build()
            .map_err(|e| SearchError::Plan(e.to_string()))?
            .compile_matcher();
        Ok(Self { matcher, negated })
    }
    pub fn matches(&self, path: &Path) -> bool {
        self.matcher.is_match(path) != self.negated
    }
}

#[derive(Clone, Debug)]
pub struct Metadata {
    pub size: u64,
    pub modified: SystemTime,
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub kind: FileKind,
    pub metadata: Option<Metadata>,
}

pub struct TraversalPlan {
    pub roots: Vec<PathBuf>,
    pub names: Vec<NamePredicate>,
    pub kind: Option<FileKind>,
    pub sizes: Vec<(Compare, u64)>,
    pub ages: Vec<(Compare, Duration)>,
    pub hidden: bool,
    pub respect_ignore: bool,
    pub follow_links: bool,
    pub max_depth: Option<usize>,
    pub max_results: Option<usize>,
    pub metadata: bool,
    /// Request file-read authorization for result callbacks that open contents.
    /// Discovering names alone requires directory-list authority, not file reads.
    pub read_contents: bool,
    pub threads: usize,
    /// Deterministic depth-first, per-directory path ordering (single worker).
    pub sorted: bool,
}

impl TraversalPlan {
    pub fn new(root: PathBuf) -> Self {
        Self {
            roots: vec![root],
            names: vec![],
            kind: None,
            sizes: vec![],
            ages: vec![],
            hidden: false,
            respect_ignore: true,
            follow_links: false,
            max_depth: None,
            max_results: None,
            metadata: false,
            read_contents: false,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get().min(8)),
            sorted: false,
        }
    }
}

/// Worker-local sinks flush before their owning visitor is destroyed.
pub trait EntrySink {
    fn entry(&mut self, entry: FileEntry) -> Result<bool, SearchError>;
    fn finish(&mut self) -> Result<(), SearchError> {
        Ok(())
    }
}

impl<F: FnMut(FileEntry) -> Result<bool, SearchError>> EntrySink for F {
    fn entry(&mut self, entry: FileEntry) -> Result<bool, SearchError> {
        self(entry)
    }
}

struct SinkOwner<'a, E: EntrySink> {
    sink: E,
    failure: &'a Mutex<Option<SearchError>>,
    control: &'a Control,
}

impl<E: EntrySink> Drop for SinkOwner<'_, E> {
    fn drop(&mut self) {
        if let Err(error) = self.sink.finish() {
            let mut failure = self.failure.lock().unwrap_or_else(|p| p.into_inner());
            if failure.is_none() {
                *failure = Some(error);
            }
            self.control.stop();
        }
    }
}

/// Workers are scoped and joined before this returns. Emission is concurrent
/// and must provide its own bounded transport. Explicit limits serialize only
/// the final admission/send, so at most N entries are emitted, even in parallel.
/// The first traversal/authorization/metadata failure aborts the operation.
pub fn walk(
    plan: &TraversalPlan,
    hooks: Hooks,
    control: Arc<Control>,
    emit: impl Fn(FileEntry) -> Result<bool, SearchError> + Sync,
) -> Result<(), SearchError> {
    walk_with(plan, hooks, control, || &emit)
}

/// Construct independent mutable output/scanner state for each traversal worker.
pub fn walk_with<E>(
    plan: &TraversalPlan,
    hooks: Hooks,
    control: Arc<Control>,
    make_emit: impl Fn() -> E,
) -> Result<(), SearchError>
where
    E: EntrySink + Send,
{
    if plan.roots.is_empty() || plan.threads == 0 {
        return Err(SearchError::Plan(
            "at least one root and one worker are required".into(),
        ));
    }
    if plan.max_results == Some(0) || control.is_stopped() || (hooks.cancelled)() {
        return Ok(());
    }
    let failure = Arc::new(Mutex::new(None));
    let record = |e| {
        let mut failure = failure.lock().unwrap_or_else(|p| p.into_inner());
        if failure.is_none() {
            *failure = Some(e);
        }
        control.stop();
    };
    for root in &plan.roots {
        let meta = root.symlink_metadata().map_err(|source| SearchError::Io {
            path: root.clone(),
            source,
        })?;
        authorize_entry(root, meta.is_dir(), plan.respect_ignore, &hooks)?;
    }
    let mut builder = WalkBuilder::new(&plan.roots[0]);
    for root in &plan.roots[1..] {
        builder.add(root);
    }
    // Only root-local ignore files are read. Global, parent and repository
    // exclude files would introduce undeclared reads outside the search scope.
    builder
        .hidden(!plan.hidden)
        .ignore(plan.respect_ignore)
        .git_ignore(plan.respect_ignore)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .require_git(false)
        .follow_links(plan.follow_links)
        .max_depth(plan.max_depth)
        .threads(plan.threads);
    let filter_hooks = hooks.clone();
    let filter_control = control.clone();
    let filter_failure = failure.clone();
    let respect_ignore = plan.respect_ignore;
    builder.filter_entry(move |entry| {
        if filter_control.is_stopped() || (filter_hooks.cancelled)() {
            return false;
        }
        let authorization = if entry.file_type().is_some_and(|t| t.is_dir()) {
            authorize_entry(entry.path(), true, respect_ignore, &filter_hooks)
        } else {
            Ok(())
        };
        match authorization {
            Ok(()) => true,
            Err(e) => {
                let mut failure = filter_failure.lock().unwrap_or_else(|p| p.into_inner());
                if failure.is_none() {
                    *failure = Some(e);
                }
                filter_control.stop();
                false
            }
        }
    });
    let now = SystemTime::now();
    let admitted = Mutex::new(0usize);
    let make_visit = || {
        let mut sink = SinkOwner {
            sink: make_emit(),
            failure: &failure,
            control: &control,
        };
        let control = &control;
        let hooks = &hooks;
        let admitted = &admitted;
        let record = &record;
        Box::new(move |result: Result<ignore::DirEntry, ignore::Error>| {
            if control.is_stopped() || (hooks.cancelled)() {
                return WalkState::Quit;
            }
            let entry = match result {
                Ok(e) => e,
                Err(e) => {
                    record(SearchError::Walk(e));
                    return WalkState::Quit;
                }
            };
            if let Some(e) = entry.error() {
                record(SearchError::Plan(e.to_string()));
                return WalkState::Quit;
            }
            // Explicit directory roots are traversal scopes, not search results.
            if entry.depth() == 0 && entry.file_type().is_some_and(|t| t.is_dir()) {
                return WalkState::Continue;
            }
            let kind = match entry.file_type() {
                Some(t) if t.is_dir() => FileKind::Directory,
                Some(t) if t.is_file() => FileKind::File,
                Some(t) if t.is_symlink() => FileKind::Symlink,
                _ => FileKind::Other,
            };
            if plan.kind.is_some_and(|k| k != kind)
                || !plan
                    .names
                    .iter()
                    .all(|p| p.matches(Path::new(entry.file_name())))
            {
                return WalkState::Continue;
            }
            let needs_metadata = plan.metadata || !plan.sizes.is_empty() || !plan.ages.is_empty();
            if (needs_metadata || plan.read_contents)
                && let Err(error) = (hooks.authorize)(entry.path(), Access::File)
            {
                record(SearchError::Denied(error));
                return WalkState::Quit;
            }
            let metadata = if needs_metadata {
                match entry.metadata().and_then(|m| {
                    m.modified()
                        .map(|modified| Metadata {
                            size: m.len(),
                            modified,
                        })
                        .map_err(ignore::Error::Io)
                }) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        record(SearchError::Walk(e));
                        return WalkState::Quit;
                    }
                }
            } else {
                None
            };
            if let Some(m) = &metadata
                && (!plan.sizes.iter().all(|(op, size)| op.test(m.size, *size))
                    || !plan.ages.iter().all(|(op, age)| {
                        op.test(now.duration_since(m.modified).unwrap_or_default(), *age)
                    }))
            {
                return WalkState::Continue;
            }
            let result = FileEntry {
                path: entry.into_path(),
                kind,
                metadata,
            };
            let outcome = if let Some(limit) = plan.max_results {
                let mut count = admitted.lock().unwrap_or_else(|p| p.into_inner());
                if *count >= limit {
                    return WalkState::Quit;
                }
                let outcome = sink.sink.entry(result);
                *count += 1;
                if *count >= limit {
                    control.stop();
                }
                outcome
            } else {
                sink.sink.entry(result)
            };
            match outcome {
                Ok(true) => WalkState::Continue,
                Ok(false) => {
                    control.stop();
                    WalkState::Quit
                }
                Err(e) => {
                    record(e);
                    WalkState::Quit
                }
            }
        })
            as Box<dyn FnMut(Result<ignore::DirEntry, ignore::Error>) -> WalkState + Send + '_>
    };
    if plan.sorted {
        builder.sort_by_file_path(Path::cmp);
        let mut visit = make_visit();
        for entry in builder.build() {
            if matches!(visit(entry), WalkState::Quit) {
                break;
            }
        }
    } else {
        builder.build_parallel().run(make_visit);
    }
    let error = failure.lock().unwrap_or_else(|p| p.into_inner()).take();
    match error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Execute an explicit file list without directory walking or traversal metadata.
/// One file runs inline; larger lists use at most the requested number of scoped
/// workers. Each worker reuses its sink/scanner and finalizes before returning.
pub fn visit_files<E: EntrySink + Send>(
    paths: &[PathBuf],
    hooks: Hooks,
    control: Arc<Control>,
    threads: usize,
    make_sink: impl Fn() -> E,
) -> Result<(), SearchError> {
    if threads == 0 {
        return Err(SearchError::Plan("worker count must be positive".into()));
    }
    if paths.is_empty() {
        return Ok(());
    }
    let failure = Mutex::new(None);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let run = |sink: E| {
        let mut owner = SinkOwner {
            sink,
            failure: &failure,
            control: &control,
        };
        while !control.is_stopped() && !(hooks.cancelled)() {
            let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(path) = paths.get(index) else {
                break;
            };
            let result = (hooks.authorize)(path, Access::File)
                .map_err(SearchError::Denied)
                .and_then(|()| {
                    owner.sink.entry(FileEntry {
                        path: path.clone(),
                        kind: FileKind::File,
                        metadata: None,
                    })
                });
            match result {
                Ok(true) => {}
                Ok(false) => {
                    control.stop();
                    break;
                }
                Err(error) => {
                    let mut failure = failure.lock().unwrap_or_else(|p| p.into_inner());
                    if failure.is_none() {
                        *failure = Some(error);
                    }
                    control.stop();
                    break;
                }
            }
        }
    };
    if paths.len() == 1 || threads == 1 {
        run(make_sink());
    } else {
        let sinks: Vec<_> = (0..threads.min(paths.len())).map(|_| make_sink()).collect();
        std::thread::scope(|scope| {
            for sink in sinks {
                let run = &run;
                scope.spawn(move || run(sink));
            }
        });
    }
    match failure.into_inner().unwrap_or_else(|p| p.into_inner()) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn authorize_entry(
    path: &Path,
    directory: bool,
    ignore: bool,
    hooks: &Hooks,
) -> Result<(), SearchError> {
    (hooks.authorize)(
        path,
        if directory {
            Access::Directory
        } else {
            Access::File
        },
    )
    .map_err(SearchError::Denied)?;
    if directory && ignore {
        // ignore reads these before invoking its directory visitor. Authorization
        // therefore belongs in the admission filter, not the result callback.
        for name in [".ignore", ".gitignore"] {
            (hooks.authorize)(&path.join(name), Access::File).map_err(SearchError::Denied)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("fixture directory must be created")
    }

    #[test]
    fn name_discovery_requires_listing_not_file_read_authority() {
        let tmp = fixture_dir();
        std::fs::write(tmp.path().join("value"), "").expect("fixture file must be created");
        let mut plan = TraversalPlan::new(tmp.path().into());
        plan.respect_ignore = false;
        let hooks = Hooks {
            authorize: Arc::new(|_, access| match access {
                Access::Directory => Ok(()),
                Access::File => Err("file read denied".into()),
            }),
            ..Hooks::unrestricted()
        };
        walk(&plan, hooks.clone(), Arc::new(Control::default()), |_| {
            Ok(true)
        })
        .expect("name discovery must succeed with listing authority");
        plan.metadata = true;
        assert!(walk(&plan, hooks, Arc::new(Control::default()), |_| Ok(true)).is_err());
    }

    #[test]
    fn one_explicit_file_runs_inline_without_metadata() {
        let tmp = fixture_dir();
        let path = tmp.path().join("value");
        std::fs::write(&path, "").expect("fixture file must be created");
        let owner = std::thread::current().id();
        visit_files(
            &[path],
            Hooks::unrestricted(),
            Arc::new(Control::default()),
            8,
            || {
                |entry: FileEntry| {
                    assert_eq!(std::thread::current().id(), owner);
                    assert!(entry.metadata.is_none());
                    Ok(true)
                }
            },
        )
        .expect("explicit file visit must succeed");
    }

    #[test]
    fn sorted_walks_are_deterministic() {
        let tmp = fixture_dir();
        for name in ["z.rs", "a.rs", "b.rs"] {
            std::fs::write(tmp.path().join(name), "").expect("fixture file must be created");
        }
        let mut plan = TraversalPlan::new(tmp.path().into());
        plan.sorted = true;
        let paths = Mutex::new(vec![]);
        walk(
            &plan,
            Hooks::unrestricted(),
            Arc::new(Control::default()),
            |entry| {
                paths
                    .lock()
                    .expect("result lock must not be poisoned")
                    .push(entry.path);
                Ok(true)
            },
        )
        .expect("sorted traversal must succeed");
        assert_eq!(
            paths
                .into_inner()
                .expect("result lock must not be poisoned"),
            ["a.rs", "b.rs", "z.rs"].map(|p| tmp.path().join(p))
        );
    }

    #[test]
    fn worker_finalization_errors_are_reported() {
        struct FailingSink;
        impl EntrySink for FailingSink {
            fn entry(&mut self, _: FileEntry) -> Result<bool, SearchError> {
                Ok(true)
            }
            fn finish(&mut self) -> Result<(), SearchError> {
                Err(SearchError::Plan("flush failed".into()))
            }
        }
        let tmp = fixture_dir();
        assert!(
            walk_with(
                &TraversalPlan::new(tmp.path().into()),
                Hooks::unrestricted(),
                Arc::new(Control::default()),
                || FailingSink
            )
            .is_err()
        );
    }

    #[test]
    fn hidden_subtrees_and_ignore_files_are_pruned() {
        let tmp = fixture_dir();
        std::fs::create_dir(tmp.path().join(".hidden"))
            .expect("hidden fixture directory must be created");
        for name in ["visible.rs", "ignored.rs", ".hidden/leak.rs"] {
            std::fs::write(tmp.path().join(name), "").expect("fixture file must be created");
        }
        std::fs::write(tmp.path().join(".gitignore"), "ignored.rs\n")
            .expect("ignore fixture must be created");
        let mut plan = TraversalPlan::new(tmp.path().into());
        plan.names.push(
            NamePredicate::compile("[vi]*.rs", false, false).expect("name matcher must compile"),
        );
        let output = Mutex::new(Vec::new());
        walk(
            &plan,
            Hooks::unrestricted(),
            Arc::new(Control::default()),
            |e| {
                output
                    .lock()
                    .expect("result lock must not be poisoned")
                    .push(e);
                Ok(true)
            },
        )
        .expect("ignored traversal must succeed");
        let output = output
            .into_inner()
            .expect("result lock must not be poisoned");
        assert_eq!(output.len(), 1);
        assert!(output[0].path.ends_with("visible.rs"));
        assert!(output[0].metadata.is_none());
    }

    #[test]
    fn results_are_unbounded_unless_an_explicit_limit_is_set() {
        let tmp = fixture_dir();
        for i in 0..1100 {
            std::fs::write(tmp.path().join(format!("{i}")), "")
                .expect("fixture file must be created");
        }
        for (limit, expected) in [(None, 1100), (Some(0), 0), (Some(10), 10)] {
            let mut plan = TraversalPlan::new(tmp.path().into());
            plan.max_results = limit;
            let count = std::sync::atomic::AtomicUsize::new(0);
            walk(
                &plan,
                Hooks::unrestricted(),
                Arc::new(Control::default()),
                |_| {
                    count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Ok(true)
                },
            )
            .expect("limited traversal must succeed");
            assert_eq!(count.into_inner(), expected);
        }
    }

    #[test]
    fn denied_directories_are_not_descended_into() {
        let tmp = fixture_dir();
        std::fs::create_dir(tmp.path().join("secret"))
            .expect("denied fixture directory must be created");
        std::fs::write(tmp.path().join("secret/value"), "").expect("fixture file must be created");
        let hooks = Hooks {
            authorize: Arc::new(|p, _| {
                if p.ends_with("secret") {
                    Err("denied".into())
                } else {
                    Ok(())
                }
            }),
            ..Hooks::unrestricted()
        };
        assert!(
            walk(
                &TraversalPlan::new(tmp.path().into()),
                hooks,
                Arc::new(Control::default()),
                |_| Ok(true)
            )
            .is_err()
        );
    }

    #[test]
    fn cancellation_before_any_match_stops_without_output() {
        let tmp = fixture_dir();
        let hooks = Hooks {
            cancelled: Arc::new(|| true),
            ..Hooks::unrestricted()
        };
        walk(
            &TraversalPlan::new(tmp.path().into()),
            hooks,
            Arc::new(Control::default()),
            |_| {
                Err(SearchError::Plan(
                    "cancelled traversal emitted a result".into(),
                ))
            },
        )
        .expect("cancelled traversal must stop without output");
    }
}
