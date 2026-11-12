//! Watching the home folder for the files people and agents edit (settings,
//! keys, programs, themes, layouts, mods), so changes apply live without
//! looking at the disk on a timer. One OS watcher (FSEvents, inotify) for all.

use notify::{RecursiveMode, Watcher as _};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// What changed, relative to the home folder.
pub type Changed = Vec<PathBuf>;

#[derive(Clone)]
pub struct FileEvents {
    pub changes: async_channel::Receiver<Changed>,
    root: PathBuf,
    revisions: Arc<Mutex<Revisions>>,
}

#[derive(Default)]
struct Revisions {
    next: u64,
    all: u64,
    roots: std::collections::BTreeMap<PathBuf, u64>,
}

impl Revisions {
    fn changed(&mut self, changed: &Changed, rescan: bool) {
        if rescan {
            self.next += 1;
            self.all = self.next;
        }
        for root in changed.iter().filter_map(|path| mod_of(path).map(|name| PathBuf::from("mods").join(name))).collect::<HashSet<_>>() {
            self.next += 1;
            self.roots.insert(root, self.next);
        }
    }
}

impl FileEvents {
    pub fn revision_provider(&self) -> impl Fn(&Path) -> Option<u64> + 'static {
        let base = self.root.clone();
        let revisions = self.revisions.clone();
        move |root| {
            let relative = root.strip_prefix(&base).ok()?;
            if relative.components().count() != 2 || !relative.starts_with("mods") {
                return None;
            }
            let mut revisions = revisions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let all = revisions.all;
            Some((*revisions.roots.entry(relative.to_path_buf()).or_insert(all)).max(all))
        }
    }
}

fn changes(base: &Path, event: notify::Result<notify::Event>) -> (Changed, bool) {
    let Ok(event) = event else { return (Vec::new(), true) };
    if !event.need_rescan() && matches!(event.kind, notify::EventKind::Access(_)) {
        return (Vec::new(), false);
    }
    let rescan = event.need_rescan() || event.paths.iter().any(|path| {
        base.starts_with(path) || path.strip_prefix(base).is_ok_and(|relative| relative.starts_with("mods") && mod_of(relative).is_none())
    });
    let changed = event.paths.iter().filter_map(|path| path.strip_prefix(base).ok())
        .filter(|path| watched(path)).map(Path::to_path_buf).collect();
    (changed, rescan)
}

/// Starts watching `home`; None if the OS watcher can't start (the app then
/// polls as before).
pub fn watch(home: &Path, #[cfg(test)] executor: gpui::BackgroundExecutor) -> Option<FileEvents> {
    static WATCHER: OnceLock<std::sync::Mutex<notify::RecommendedWatcher>> = OnceLock::new();
    let (tx, rx) = async_channel::unbounded();
    #[cfg(test)]
    let tx = test_notifications(tx, executor);
    let root = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let base = root.clone();
    let revisions = Arc::new(Mutex::new(Revisions::default()));
    let event_revisions = revisions.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let mut revisions = event_revisions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (changed, rescan) = changes(&base, event);
        revisions.changed(&changed, rescan);
        drop(revisions);
        if !changed.is_empty() {
            #[cfg(not(test))]
            let _ = tx.send_blocking(changed);
            #[cfg(test)]
            let _ = tx.send(changed);
        }
    })
    .map_err(|e| tracing::warn!("file watching unavailable ({e}); polling instead"))
    .ok()?;
    watcher.watch(&root, RecursiveMode::Recursive).map_err(|e| tracing::warn!("cannot watch {} ({e}); polling instead", root.display())).ok()?;
    WATCHER.set(std::sync::Mutex::new(watcher)).ok()?;
    Some(FileEvents { changes: rx, root, revisions })
}

// Native callbacks cannot wake GPUI's deterministic test scheduler directly.
#[cfg(test)]
fn test_notifications(tx: async_channel::Sender<Changed>, executor: gpui::BackgroundExecutor) -> std::sync::mpsc::Sender<Changed> {
    let (pending, events) = std::sync::mpsc::channel();
    let clock = executor.clone();
    executor.spawn(async move {
        while !tx.is_closed() {
            clock.timer(std::time::Duration::from_millis(10)).await;
            while let Ok(changed) = events.try_recv() {
                if tx.try_send(changed).is_err() { return; }
            }
        }
    }).detach();
    pending
}

/// The files that change what the app shows. The app's own state files
/// (view.json, shared.json, data/) change all the time and aren't.
fn watched(rel: &Path) -> bool {
    let mut parts = rel.components();
    let first = parts.next().and_then(|c| c.as_os_str().to_str()).unwrap_or_default();
    match first {
        "mods" => mod_file_counts(parts),
        "layouts" | "themes" => true,
        "settings.json" | "keys.json" | "programs.json" => parts.next().is_none(),
        _ => false,
    }
}

/// A change inside `mods/` that should reload a mod. The runtime writes its
/// typings and jsconfig into each mod on load, and editors drop hidden and
/// backup files: none of those are the mod's code.
fn mod_file_counts(rest: std::path::Components) -> bool {
    let mut rest = rest;
    let Some(name) = rest.next() else { return true };
    gpui_shell::application_file_counts(Path::new(name.as_os_str()))
        && gpui_shell::application_file_counts(rest.as_path())
}

/// Folders mods watch (`watchFolder`): one OS watcher each, shared by every
/// mod waiting on it. Bounded invalidations collect even while nobody waits.
struct Folder {
    _watcher: std::sync::mpsc::Sender<()>,
    generation: Arc<()>,
    waiters: Vec<async_channel::Sender<Vec<String>>>,
    pending: HashSet<String>,
}

static FOLDERS: OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, Folder>>> = OnceLock::new();
/// Changes are batched this long (an editor's save is several events).
const BATCH: std::time::Duration = std::time::Duration::from_millis(150);
/// At most this many folders watched for mods at once.
const MAX_FOLDERS: usize = 64;
const MAX_PENDING: usize = 1024;
#[cfg(target_os = "linux")]
const MAX_DIRECTORIES: usize = 4096;

fn folders() -> std::sync::MutexGuard<'static, std::collections::HashMap<PathBuf, Folder>> {
    let registry = FOLDERS.get_or_init(Default::default);
    registry.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("recovering poisoned folder watch registry");
        registry.clear_poison();
        poisoned.into_inner()
    })
}

/// The next changes under `path` (recursive), as absolute paths: at once if
/// some came since the last call, else when the next batch lands.
pub async fn next_change(path: String) -> Result<Vec<String>, String> {
    let root = Path::new(&path).canonicalize().map_err(|e| format!("{path}: {e}"))?;
    let (rx, sender) = {
        let mut all = folders();
        if !all.contains_key(&root) {
            if all.len() >= MAX_FOLDERS {
                reclaim_idle(&mut all);
            }
            if all.len() >= MAX_FOLDERS {
                return Err(format!("already watching {MAX_FOLDERS} folders"));
            }
            let generation = Arc::new(());
            all.insert(root.clone(), Folder { _watcher: start_folder(&root, generation.clone())?, generation, waiters: Vec::new(), pending: HashSet::new() });
        }
        let f = all.get_mut(&root).unwrap();
        if !f.pending.is_empty() {
            return Ok(f.pending.drain().collect());
        }
        f.waiters.retain(|w| !w.is_closed());
        let (tx, rx) = async_channel::bounded(1);
        let weak = tx.downgrade();
        f.waiters.push(tx);
        (rx, weak)
    };
    let _waiting = FolderWait { root, sender };
    rx.recv().await.map_err(|_| "stopped watching".to_owned())
}

/// Weak: when the folder's watcher fails and lets go of its waiters, this wait
/// must end with an error, not hang on a sender it keeps alive itself.
struct FolderWait {
    root: PathBuf,
    sender: async_channel::WeakSender<Vec<String>>,
}

impl Drop for FolderWait {
    fn drop(&mut self) {
        let Some(sender) = self.sender.upgrade() else { return };
        let mut all = folders();
        if let Some(folder) = all.get_mut(&self.root) {
            let pending = folder.waiters.iter().any(|waiter| waiter.same_channel(&sender));
            folder.waiters.retain(|waiter| !waiter.same_channel(&sender));
            if pending && folder.waiters.is_empty() {
                all.remove(&self.root);
            }
        }
    }
}

fn reclaim_idle(all: &mut std::collections::HashMap<PathBuf, Folder>) {
    all.retain(|_, folder| {
        folder.waiters.retain(|waiter| !waiter.is_closed());
        !folder.waiters.is_empty()
    });
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    /// Git supplies its own ignore semantics, including global and nested rules.
    fn ignored_directories(root: &Path) -> HashSet<PathBuf> {
        let Ok(output) = std::process::Command::new("git").arg("-C").arg(root).args(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]).output() else {
            return HashSet::new();
        };
        if !output.status.success() {
            return HashSet::new();
        }
        output.stdout.split(|b| *b == 0).filter(|p| p.ends_with(b"/")).map(|p| root.join(path_from_bytes(p))).collect()
    }

    fn path_from_bytes(bytes: &[u8]) -> PathBuf {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }

    pub(super) fn watch_directories(root: &Path) -> Result<HashSet<PathBuf>, String> {
        let ignored = ignored_directories(root);
        let mut paths = HashSet::from([root.to_path_buf()]);
        let mut git_roots = HashSet::new();
        if let Ok(output) = std::process::Command::new("git").arg("-C").arg(root).args(["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"]).output()
            && output.status.success()
        {
            for line in output.stdout.split(|b| *b == b'\n').filter(|p| !p.is_empty()) {
                git_roots.insert(path_from_bytes(line));
            }
        }
        let mut todo = vec![root.to_path_buf()];
        for dir in &git_roots {
            if paths.insert(dir.clone()) {
                todo.push(dir.clone());
            }
        }
        while let Some(dir) = todo.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|t| t.is_dir()) || ignored.contains(&entry.path()) {
                    continue;
                }
                // Git's object database can dwarf the working tree. HEAD/index and refs
                // still invalidate git panels; objects themselves cannot change status.
                if (git_roots.contains(&dir) || dir.file_name().is_some_and(|n| n == ".git")) && entry.file_name() != "refs" && entry.file_name() != "info" {
                    continue;
                }
                if paths.insert(entry.path()) {
                    if paths.len() > MAX_DIRECTORIES {
                        return Err(format!("{} exceeds {MAX_DIRECTORIES} watch directories; live folder updates unavailable, use periodic refresh", root.display()));
                    }
                    todo.push(entry.path());
                }
            }
        }
        Ok(paths)
    }
}

fn watch_directories(root: &Path) -> Result<HashSet<PathBuf>, String> {
    #[cfg(target_os = "linux")]
    {
        linux::watch_directories(root)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(HashSet::from([root.to_path_buf()]))
    }
}

fn folder_watch_mode() -> RecursiveMode {
    // Inotify consumes a watch per directory; FSEvents and Windows recurse natively.
    if cfg!(target_os = "linux") { RecursiveMode::NonRecursive } else { RecursiveMode::Recursive }
}

fn add_pending(pending: &mut HashSet<String>, root: &Path, paths: impl IntoIterator<Item = PathBuf>) {
    let root = root.to_string_lossy().into_owned();
    for path in paths {
        if pending.contains(&root) {
            break;
        }
        if pending.len() >= MAX_PENDING {
            // A root invalidation means refresh everything, not a partial path list.
            pending.clear();
            pending.insert(root.clone());
            break;
        }
        pending.insert(path.to_string_lossy().into_owned());
    }
}

fn collect_event(event: notify::Event, root: &Path, directories: &HashSet<PathBuf>, pending: &mut HashSet<String>, removed: &mut HashSet<PathBuf>) -> bool {
    if event.need_rescan() {
        pending.clear();
        pending.insert(root.to_string_lossy().into_owned());
        // With dropped events any directory may have been replaced.
        removed.extend(directories.iter().cloned());
        return true;
    }
    if matches!(event.kind, notify::EventKind::Remove(_) | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))) {
        removed.extend(event.paths.iter().filter(|p| directories.contains(*p)).cloned());
    }
    let refresh = event.paths.iter().any(|p| {
        p.file_name().is_some_and(|n| n == ".gitignore" || n == "exclude")
            || (matches!(event.kind, notify::EventKind::Create(_)) && p.is_dir())
            || (matches!(event.kind, notify::EventKind::Remove(_)) && directories.contains(p))
    }) || matches!(event.kind, notify::EventKind::Modify(notify::event::ModifyKind::Name(_)));
    add_pending(pending, root, event.paths);
    refresh
}

fn start_folder(root: &Path, generation: Arc<()>) -> Result<std::sync::mpsc::Sender<()>, String> {
    let mut directories = watch_directories(root).inspect_err(|e| tracing::warn!("{e}"))?;
    let (tx, events) = std::sync::mpsc::sync_channel(1024);
    let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lost = overflow.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.as_ref().is_ok_and(|e| !e.need_rescan() && matches!(e.kind, notify::EventKind::Access(_))) {
            return;
        }
        if tx.try_send(event).is_err() {
            lost.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    })
    .map_err(|e| format!("cannot watch: {e}"))?;
    for path in &directories {
        watcher.watch(path, folder_watch_mode()).map_err(|e| format!("cannot watch {}: {e}", path.display()))?;
    }
    let root = root.to_path_buf();
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        'watching: loop {
            if matches!(stopped.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)) {
                break;
            }
            let deadline = std::time::Instant::now() + BATCH;
            let mut pending = HashSet::new();
            let mut refresh = false;
            let mut removed = HashSet::new();
            while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
                match events.recv_timeout(left) {
                    Ok(Ok(event)) => {
                        refresh |= collect_event(event, &root, &directories, &mut pending, &mut removed);
                    }
                    Ok(Err(_)) => {
                        removed.extend(directories.iter().cloned());
                        refresh = true;
                        pending.clear();
                        pending.insert(root.to_string_lossy().into_owned());
                    }
                    Err(_) => break,
                }
            }
            if overflow.swap(false, std::sync::atomic::Ordering::Relaxed) {
                removed.extend(directories.iter().cloned());
                refresh = true;
                pending.clear();
                pending.insert(root.to_string_lossy().into_owned());
            }
            if cfg!(target_os = "linux") && refresh {
                // Rescan off the notify callback thread: adding watches blocks on it.
                match watch_directories(&root) {
                    Ok(next) => {
                        // A removed inode can return at the same path before this batch
                        // finishes. Its descendants also lost their inotify watches.
                        let replaced = |path: &PathBuf| path.ancestors().any(|p| removed.contains(p));
                        for path in &directories {
                            if !next.contains(path) || replaced(path) {
                                let _ = watcher.unwatch(path);
                            }
                        }
                        for path in &next {
                            if directories.contains(path) && !replaced(path) {
                                continue;
                            }
                            if let Err(e) = watcher.watch(path, folder_watch_mode()) {
                                tracing::warn!("cannot watch {}: {e}; live folder updates unavailable, use periodic refresh", path.display());
                                remove_folder(&root, &generation);
                                break 'watching;
                            }
                        }
                        directories = next;
                    }
                    Err(e) => {
                        tracing::warn!("{e}");
                        // Do not leave a partial recursive watch pretending to be live.
                        remove_folder(&root, &generation);
                        break;
                    }
                }
            }
            if pending.is_empty() {
                continue;
            }
            if !deliver_pending(&root, &generation, pending) {
                break;
            }
        }
    });
    Ok(stop)
}

fn remove_folder(root: &Path, generation: &Arc<()>) {
    let mut all = folders();
    if all.get(root).is_some_and(|f| Arc::ptr_eq(&f.generation, generation)) {
        all.remove(root);
    }
}

fn deliver_pending(root: &Path, generation: &Arc<()>, pending: HashSet<String>) -> bool {
    let mut all = folders();
    let Some(f) = all.get_mut(root) else { return false };
    // A reclaimed path can already have a new watcher while the old worker exits.
    if !Arc::ptr_eq(&f.generation, generation) {
        return false;
    }
    add_pending(&mut f.pending, root, pending.into_iter().map(PathBuf::from));
    f.waiters.retain(|w| !w.is_closed());
    if !f.waiters.is_empty() {
        let batch: Vec<_> = f.pending.drain().collect();
        for w in f.waiters.drain(..) {
            let _ = w.try_send(batch.clone());
        }
    }
    true
}

// ---- the dirs a mod shows (`watchDirs` / `watchChanges`) ----------------------

/// What one mod shows: a few dirs watched one level deep each. A build in a folder
/// the mod isn't showing (`target/`, `node_modules/`) never wakes it, because those
/// dirs are not watched at all — no recursive watch over the whole project.
struct Shown {
    watcher: notify::RecommendedWatcher,
    state: Arc<std::sync::Mutex<ShownState>>,
    waiters: Arc<std::sync::Mutex<Vec<async_channel::Sender<Vec<String>>>>>,
    last_use: std::time::Instant,
}

#[derive(Default)]
struct ShownState {
    /// Dir as the mod spelled it -> the same dir resolved, for backends that
    /// name the real path (FSEvents resolves links).
    watched: std::collections::HashMap<PathBuf, PathBuf>,
    /// Dirs, spelled as the mod gave them, whose contents changed.
    pending: HashSet<PathBuf>,
}

static SHOWN: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Shown>>> = OnceLock::new();
/// A panel shows a handful of dirs; past this the OS refuses to watch anyway.
const MAX_SHOWN_DIRS: usize = 512;
/// A shown set with no waiter this long gives its watcher back (a closed panel).
const SHOWN_IDLE: std::time::Duration = std::time::Duration::from_secs(5);
/// Changes are batched this long before a waiter hears them (a save is several
/// events), but no longer than `SHOWN_MAX_WAIT` while events keep coming.
const SHOWN_BATCH: std::time::Duration = std::time::Duration::from_millis(150);
const SHOWN_MAX_WAIT: std::time::Duration = std::time::Duration::from_millis(1000);

fn shown_registry() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, Shown>> {
    let registry = SHOWN.get_or_init(|| {
        // One thread for every shown set: a set nobody waits on gives its watcher
        // back, so closing a panel (or leaving a space) frees the OS handles.
        let _ = std::thread::Builder::new().name("herdr-shown-idle".into()).spawn(|| loop {
            std::thread::sleep(SHOWN_IDLE / 2);
            let Some(all) = SHOWN.get() else { continue };
            let mut all = all.lock().unwrap_or_else(|p| p.into_inner());
            let now = std::time::Instant::now();
            all.retain(|_, s| {
                let idle = now.duration_since(s.last_use) >= SHOWN_IDLE;
                let waiting = s.waiters.lock().unwrap_or_else(|p| p.into_inner()).iter().any(|w| !w.is_closed());
                !idle || waiting
            });
        });
        std::sync::Mutex::new(std::collections::HashMap::new())
    });
    registry.lock().unwrap_or_else(|p| {
        tracing::warn!("recovering poisoned shown-dir registry");
        registry.clear_poison();
        p.into_inner()
    })
}

/// The dirs one mod shows now. The first is the root: a panel that can't watch the
/// folder it shows fails the call, so it can poll instead. The rest are the folders
/// it has open and (for a files panel) its git dir. The set is kept in step on every
/// call, so passing the same dirs again costs nothing.
pub fn set_watch_dirs(id: String, dirs: Vec<PathBuf>) -> Result<(), String> {
    let given: Vec<PathBuf> = dirs.into_iter().filter(|d| !d.as_os_str().is_empty()).collect();
    if given.len() > MAX_SHOWN_DIRS {
        return Err(format!("watching {} dirs is more than {MAX_SHOWN_DIRS}", given.len()));
    }
    let Some(root) = given.first().cloned() else {
        drop_watch(&id);
        return Ok(());
    };
    let mut all = shown_registry();
    if !all.contains_key(&id) {
        let state: Arc<std::sync::Mutex<ShownState>> = Arc::default();
        let waiters: Arc<std::sync::Mutex<Vec<async_channel::Sender<Vec<String>>>>> = Arc::default();
        let (wake, sleeping) = std::sync::mpsc::sync_channel::<()>(1);
        let watcher = notify::recommended_watcher({
            let state = state.clone();
            move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
                let mut changed = false;
                if event.need_rescan() {
                    for dir in s.watched.keys().cloned().collect::<Vec<_>>() {
                        changed |= s.pending.insert(dir);
                    }
                } else if !matches!(event.kind, notify::EventKind::Access(_)) {
                    for path in &event.paths {
                        if let Some(dir) = landed_in(&s.watched, path) {
                            changed |= s.pending.insert(dir);
                        }
                    }
                }
                drop(s);
                // A full wake channel is a batch already on its way.
                if changed {
                    let _ = wake.try_send(());
                }
            }
        })
        .map_err(|e| format!("cannot watch files: {e}"))?;
        let batch = {
            let (state, waiters) = (state.clone(), waiters.clone());
            move || batch_shown(state, waiters, sleeping)
        };
        std::thread::Builder::new()
            .name("herdr-shown-batch".into())
            .spawn(batch)
            .map_err(|e| format!("cannot start watching files: {e}"))?;
        all.insert(id.clone(), Shown { watcher, state, waiters, last_use: std::time::Instant::now() });
    }
    let entry = all.get_mut(&id).expect("the shown set was just made");
    entry.last_use = std::time::Instant::now();
    apply_dirs(entry, &given, &root)
}

/// The next changes in the dirs a mod shows, as it spelled them.
pub async fn next_watch_change(id: String) -> Result<Vec<String>, String> {
    let rx = {
        let mut all = shown_registry();
        let Some(entry) = all.get_mut(&id) else { return Err("these folders are no longer watched".to_owned()) };
        entry.last_use = std::time::Instant::now();
        let mut ws = entry.waiters.lock().unwrap_or_else(|p| p.into_inner());
        ws.retain(|w| !w.is_closed());
        let pending: Vec<String> = {
            let mut s = entry.state.lock().unwrap_or_else(|p| p.into_inner());
            s.pending.drain().map(|p| p.to_string_lossy().into_owned()).collect()
        };
        if !pending.is_empty() {
            return Ok(pending);
        }
        let (tx, rx) = async_channel::bounded(1);
        ws.push(tx);
        rx
    };
    rx.recv().await.map_err(|_| "these folders are no longer watched".to_owned())
}

/// Stops watching a mod's dirs: its pending waiter hears why, and the OS handles go.
pub fn drop_watch(id: &str) {
    let Some(entry) = shown_registry().remove(id) else { return };
    // Waking the loop that waited: a promise nobody resolves would keep a copy's task
    // alive for the rest of the session.
    entry.waiters.lock().unwrap_or_else(|p| p.into_inner()).clear();
}

fn apply_dirs(entry: &mut Shown, given: &[PathBuf], root: &Path) -> Result<(), String> {
    let mut want = std::collections::HashMap::new();
    for dir in given {
        // A folder that went away is dropped from the set like the row it had; the
        // root is the panel itself, so without it there is nothing to show.
        match dir.canonicalize() {
            Ok(real) => {
                want.insert(dir.clone(), real);
            }
            Err(e) if dir == root => return Err(format!("{}: {e}", dir.display())),
            Err(_) => {}
        }
    }
    let (gone, fresh) = {
        let s = entry.state.lock().unwrap_or_else(|p| p.into_inner());
        let gone: Vec<PathBuf> = s.watched.keys().filter(|d| !want.contains_key(*d)).cloned().collect();
        let fresh: Vec<(PathBuf, PathBuf)> = want.iter().filter(|(d, _)| !s.watched.contains_key(*d)).map(|(d, real)| (d.clone(), real.clone())).collect();
        (gone, fresh)
    };
    for dir in &gone {
        let _ = entry.watcher.unwatch(dir);
    }
    let mut watched = Vec::new();
    for (dir, real) in fresh {
        match entry.watcher.watch(&dir, RecursiveMode::NonRecursive) {
            Ok(()) => watched.push((dir, real)),
            Err(e) if dir == *root => return Err(format!("cannot watch {}: {e}", dir.display())),
            Err(e) => tracing::warn!("cannot watch {}: {e}", dir.display()),
        }
    }
    let mut s = entry.state.lock().unwrap_or_else(|p| p.into_inner());
    for dir in gone {
        s.watched.remove(&dir);
    }
    s.watched.extend(watched);
    Ok(())
}

/// The shown dir a changed path landed in: the path itself when a watched dir was
/// renamed or removed, else its parent — the dir whose contents changed.
fn landed_in(watched: &std::collections::HashMap<PathBuf, PathBuf>, path: &Path) -> Option<PathBuf> {
    if let Some((dir, _)) = watched.get_key_value(path) {
        return Some(dir.clone());
    }
    let parent = path.parent()?;
    if let Some((dir, _)) = watched.get_key_value(parent) {
        return Some(dir.clone());
    }
    // A backend that resolves links names the real path: match what it resolves to.
    let real = parent.canonicalize().ok()?;
    watched.iter().find(|(_, resolved)| **resolved == real).map(|(dir, _)| dir.clone())
}

/// Waits for a burst of changes to end, then hands the dirs it touched to whoever
/// waits; dirs collected while nobody waits stay until somebody does.
fn batch_shown(
    state: Arc<std::sync::Mutex<ShownState>>,
    waiters: Arc<std::sync::Mutex<Vec<async_channel::Sender<Vec<String>>>>>,
    wake: std::sync::mpsc::Receiver<()>,
) {
    while wake.recv().is_ok() {
        let started = std::time::Instant::now();
        loop {
            let left = SHOWN_MAX_WAIT.saturating_sub(started.elapsed()).min(SHOWN_BATCH);
            match wake.recv_timeout(left) {
                Ok(()) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        let mut ws = waiters.lock().unwrap_or_else(|p| p.into_inner());
        ws.retain(|w| !w.is_closed());
        if ws.is_empty() {
            continue;
        }
        let batch: Vec<String> = {
            let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
            s.pending.drain().map(|p| p.to_string_lossy().into_owned()).collect()
        };
        if batch.is_empty() {
            continue;
        }
        for waiter in ws.drain(..) {
            let _ = waiter.try_send(batch.clone());
        }
    }
}

/// The mod a changed path belongs to (`mods/<name>/...`).
pub fn mod_of(rel: &Path) -> Option<String> {
    let mut parts = rel.components();
    (parts.next()?.as_os_str() == "mods").then_some(())?;
    parts.next()?.as_os_str().to_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_revisions_survive_closed_copies_and_rescan_invalidates_all() {
        let (_, changes) = async_channel::unbounded();
        let events = FileEvents {
            changes,
            root: PathBuf::from("/home/probe"),
            revisions: Default::default(),
        };
        let revision = events.revision_provider();
        let first = Path::new("/home/probe/mods/one");
        let other = Path::new("/home/probe/mods/other");
        assert_eq!(revision(first), Some(0));
        assert_eq!(revision(other), Some(0));
        events.revisions.lock().unwrap().changed(&vec!["mods/one/main.js".into()], false);
        assert_eq!(revision(first), Some(1));
        assert_eq!(revision(other), Some(0));
        // No copy registration is needed for a root edited while closed.
        events.revisions.lock().unwrap().changed(&vec!["mods/closed/main.js".into()], false);
        assert_eq!(revision(Path::new("/home/probe/mods/closed")), Some(2));
        let changed = Vec::new();
        events.revisions.lock().unwrap().changed(&changed, true);
        assert!(revision(first).unwrap() > 2);
        assert!(revision(other).unwrap() > 2);
        assert!(changed.is_empty(), "a rescan must not request mod hot reloads");
        assert_eq!(revision(Path::new("/outside")), None);
    }

    #[test]
    fn unattributed_events_revalidate_code_without_reloading_mods() {
        let base = Path::new("/home/probe");
        let mut paths = vec![base.to_path_buf(), base.join("mods"), base.parent().unwrap().to_path_buf()];
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            paths.push(base.join("mods").join(std::ffi::OsString::from_vec(vec![0xff])));
        }
        let mut events: Vec<notify::Result<notify::Event>> = paths.into_iter()
            .map(|path| Ok(notify::Event::new(notify::EventKind::Any).add_path(path))).collect();
        events.push(Err(notify::Error::generic("missed events")));
        events.push(Ok(notify::Event::new(notify::EventKind::Any).set_flag(notify::event::Flag::Rescan)));
        for event in events {
            let mut revisions = Revisions::default();
            revisions.roots.insert("mods/one".into(), 0);
            revisions.roots.insert("mods/two".into(), 0);
            let (changed, rescan) = changes(base, event);
            revisions.changed(&changed, rescan);
            assert!(revisions.all > 0);
            assert!(!changed.contains(&PathBuf::from("mods/one")));
            assert!(!changed.contains(&PathBuf::from("mods/two")));
        }
    }

    #[test]
    fn rescan_keeps_real_edits_without_reloading_other_mods() {
        let base = Path::new("/home/probe");
        let mut revisions = Revisions::default();
        revisions.roots.insert("mods/one".into(), 0);
        revisions.roots.insert("mods/two".into(), 0);
        let event = notify::Event::new(notify::EventKind::Any)
            .set_flag(notify::event::Flag::Rescan)
            .add_path(base.join("mods/one/main.js"));
        let (changed, rescan) = changes(base, Ok(event));
        revisions.changed(&changed, rescan);
        assert_eq!(changed, vec![PathBuf::from("mods/one/main.js")]);
        assert!(revisions.all > 0);
        assert!(revisions.roots[Path::new("mods/one")] > revisions.all);
        assert_eq!(revisions.roots[Path::new("mods/two")], 0);
    }

    #[test]
    fn revision_provider_does_not_keep_receiver_alive_and_recovers_poison() {
        let (tx, changes) = async_channel::unbounded();
        let events = FileEvents {
            changes,
            root: PathBuf::from("/home/probe"),
            revisions: Default::default(),
        };
        let revision = events.revision_provider();
        let state = events.revisions.clone();
        drop(events);
        assert!(tx.is_closed());
        assert!(std::panic::catch_unwind(|| {
            let _lock = state.lock().unwrap();
            panic!("poison revision state");
        }).is_err());
        assert_eq!(revision(Path::new("/home/probe/mods/one")), Some(0));
    }

    #[gpui::test]
    fn a_wait_ends_with_an_error_when_its_watcher_stops(cx: &mut gpui::TestAppContext) {
        let dir = std::env::temp_dir().join(format!("herdr-watch-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let received = result.clone();
        let path = dir.to_string_lossy().into_owned();
        cx.background_executor.spawn(async move {
            *received.lock().unwrap() = Some(next_change(path).await);
        }).detach();
        cx.run_until_parked();
        let (root, generation) = {
            let all = folders();
            let (root, folder) = all.iter().find(|(root, _)| root.ends_with(dir.file_name().unwrap())).expect("the wait registered its folder");
            (root.clone(), folder.generation.clone())
        };
        assert!(result.lock().unwrap().is_none(), "nothing changed yet");
        // What a failed native watcher does.
        remove_folder(&root, &generation);
        cx.run_until_parked();
        assert_eq!(*result.lock().unwrap(), Some(Err("stopped watching".to_owned())));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn native_notifications_reach_a_waiting_test_receiver(cx: &mut gpui::TestAppContext) {
        let (tx, rx) = async_channel::unbounded();
        let pending = test_notifications(tx, cx.background_executor.clone());
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let received = result.clone();
        cx.background_executor.spawn(async move {
            *received.lock().unwrap() = rx.recv().await.ok();
        }).detach();
        cx.run_until_parked();
        let paths = vec![PathBuf::from("settings.json")];
        let sent = paths.clone();
        std::thread::spawn(move || pending.send(sent).unwrap()).join().unwrap();
        cx.executor().advance_clock(std::time::Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(*result.lock().unwrap(), Some(paths));
    }

    #[test]
    fn only_what_people_edit_counts() {
        assert!(watched(Path::new("settings.json")));
        assert!(watched(Path::new("mods/agents/main.js")));
        assert!(watched(Path::new("layouts/main.json")));
        assert!(!watched(Path::new("view.json")));
        assert!(!watched(Path::new("shared.json")));
        assert!(!watched(Path::new("data/x")));
        assert_eq!(mod_of(Path::new("mods/agents/main.js")).as_deref(), Some("agents"));
    }

    #[test]
    fn files_the_runtime_or_an_editor_writes_into_a_mod_do_not_reload_it() {
        assert!(watched(Path::new("mods/probe")));
        assert!(watched(Path::new("mods/probe/ui.js")));
        assert!(watched(Path::new("mods/probe/target/output.js")));
        assert!(watched(Path::new("mods/probe/lib/jsconfig.json")));
        assert!(!watched(Path::new("mods/probe/jsconfig.json")));
        assert!(!watched(Path::new("mods/probe/gpui-kit.d.ts")));
        assert!(!watched(Path::new("mods/probe/.main.js.swp")));
        assert!(!watched(Path::new("mods/probe/.vscode/settings.json")));
        assert!(!watched(Path::new("mods/probe/main.js~")));
        assert!(!watched(Path::new("mods/probe/lib/gpui-kit.d.ts")));
        assert!(!watched(Path::new("mods/probe/node_modules/x/index.js")));
        assert!(!watched(Path::new("mods/probe/.git/HEAD")));
        assert!(watched(Path::new("mods/probe/mod.json")));
        assert_eq!(mod_of(Path::new("layouts/main.json")), None);
    }

    #[test]
    fn pending_paths_deduplicate_and_overflow_to_a_root_invalidation() {
        let root = Path::new("/project");
        let mut pending = HashSet::new();
        add_pending(&mut pending, root, [root.join("same"), root.join("same")]);
        assert_eq!(pending.len(), 1);
        add_pending(&mut pending, root, (0..MAX_PENDING * 10).map(|i| root.join(i.to_string())));
        assert_eq!(pending, HashSet::from(["/project".to_owned()]));
    }

    fn test_repo(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("herdr-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(std::process::Command::new("git").arg("init").arg("-q").arg(&root).status().unwrap().success());
        root.canonicalize().unwrap()
    }

    // Backends may invalidate a parent rather than name every changed child.
    fn invalidates(changes: &[String], path: &Path) -> bool {
        changes.iter().any(|changed| path.starts_with(changed))
    }

    fn install_watch(root: &Path) {
        // These event tests need an idle watch that buffers between calls.
        let generation = Arc::new(());
        folders().insert(root.to_owned(), Folder {
            _watcher: start_folder(root, generation.clone()).unwrap(),
            generation, waiters: Vec::new(), pending: HashSet::new(),
        });
    }

    fn wait_for_change(root: &Path, path: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let changes = block_on_until(next_change(root.to_string_lossy().into_owned()), deadline).unwrap();
            if invalidates(&changes, path) {
                return;
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn git_ignored_directories_are_pruned_but_tracked_files_and_refs_stay_watched() {
        let root = test_repo("ignore");
        std::fs::create_dir_all(root.join("target/deep")).unwrap();
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::create_dir_all(root.join("tracked")).unwrap();
        std::fs::write(root.join("tracked/keep"), "tracked").unwrap();
        assert!(std::process::Command::new("git").arg("-C").arg(&root).args(["add", "tracked/keep"]).status().unwrap().success());
        std::fs::write(root.join(".gitignore"), "target/\ntracked/\n").unwrap();
        let paths = watch_directories(&root).unwrap();
        assert!(!paths.contains(&root.join("target")));
        assert!(paths.contains(&root.join("src/nested")));
        assert!(paths.contains(&root.join("tracked")));
        assert!(paths.contains(&root.join(".git/refs/heads")));
        assert!(!paths.contains(&root.join(".git/objects")));
        assert!(paths.contains(&root.join(".git/info")));
        // Starting directly in an ignored folder still watches what was requested.
        assert!(watch_directories(&root.join("target")).unwrap().contains(&root.join("target/deep")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn new_directories_and_changed_ignore_rules_update_live_watches() {
        use std::time::{Duration, Instant};
        let root = test_repo("dynamic");
        std::fs::create_dir_all(root.join("target/deep")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        // Poll once to install the watch, then abandon the waiter like a closed mod.
        install_watch(&root);
        std::fs::create_dir_all(root.join("fresh/nested")).unwrap();
        // Delivery follows watch registration, not an assumed backend delay.
        wait_for_change(&root, &root.join("fresh/nested"));
        std::fs::write(root.join("fresh/nested/file"), "one").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if folders().get(&root).unwrap().pending.iter().any(|p| p.ends_with("nested/file")) {
                break;
            }
            assert!(Instant::now() < deadline, "new directory was not watched");
            std::thread::sleep(BATCH);
        }
        folders().get_mut(&root).unwrap().pending.clear();
        std::fs::write(root.join("target/deep/ignored"), "ignored").unwrap();
        std::thread::sleep(BATCH * 3);
        assert!(folders().get(&root).unwrap().pending.is_empty());
        std::fs::write(root.join(".gitignore"), "").unwrap();
        wait_for_change(&root, &root.join(".gitignore"));
        std::fs::write(root.join("target/deep/now-visible"), "yes").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if folders().get(&root).unwrap().pending.iter().any(|p| p.ends_with("now-visible")) {
                break;
            }
            assert!(Instant::now() < deadline, "ignore change did not update watches");
            std::thread::sleep(BATCH);
        }
        folders().remove(&root);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_waits_release_quiet_folders_without_stopping_other_callers() {
        use std::task::{Context, Waker};
        let root = test_repo("cancel");
        let path = root.to_string_lossy().into_owned();
        let mut first = Box::pin(next_change(path.clone()));
        let mut second = Box::pin(next_change(path.clone()));
        let mut context = Context::from_waker(Waker::noop());
        assert!(first.as_mut().poll(&mut context).is_pending());
        assert!(second.as_mut().poll(&mut context).is_pending());
        drop(first);
        assert_eq!(folders().get(&root).unwrap().waiters.len(), 1);
        std::fs::write(root.join("changed"), "yes").unwrap();
        assert!(invalidates(&block_on(second).unwrap(), &root.join("changed")));
        folders().remove(&root);
        std::fs::remove_dir_all(root).unwrap();
        let root = test_repo("cancel-quiet");
        let path = root.to_string_lossy().into_owned();
        for _ in 0..75 {
            let mut wait = Box::pin(next_change(path.clone()));
            assert!(wait.as_mut().poll(&mut context).is_pending());
            drop(wait);
            assert!(!folders().contains_key(&root));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reclaiming_idle_folders_keeps_live_waiters_and_stops_idle_workers() {
        let mut all = std::collections::HashMap::new();
        let mut live = Vec::new();
        let mut workers = Vec::new();
        for i in 0..MAX_FOLDERS {
            let (stop, stopped) = std::sync::mpsc::channel();
            let (tx, rx) = async_channel::bounded(1);
            if i % 3 == 0 {
                drop(rx); // A mod copy closed or its host future was cancelled.
            } else if i % 3 == 1 {
                live.push(rx);
            } else {
                tx.try_send(vec!["changed".into()]).unwrap();
                drop(tx); // The batch was delivered; nothing awaits more changes.
                all.insert(PathBuf::from(i.to_string()), Folder {
                    _watcher: stop, generation: Arc::new(()), waiters: Vec::new(),
                    pending: HashSet::from(["another change".into()]),
                });
                workers.push(stopped);
                continue;
            }
            all.insert(PathBuf::from(i.to_string()), Folder {
                _watcher: stop, generation: Arc::new(()), waiters: vec![tx], pending: HashSet::new(),
            });
            workers.push(stopped);
        }
        reclaim_idle(&mut all);
        assert_eq!(all.len(), live.len());
        for (i, stopped) in workers.iter().enumerate() {
            assert_eq!(stopped.try_recv().unwrap_err(), if i % 3 == 1 {
                std::sync::mpsc::TryRecvError::Empty
            } else {
                std::sync::mpsc::TryRecvError::Disconnected
            });
        }
        // Genuine demand remains bounded; reclamation must not interrupt it.
        reclaim_idle(&mut all);
        assert_eq!(all.len(), live.len());
        drop(live);
        reclaim_idle(&mut all);
        assert!(all.is_empty());
    }

    #[test]
    fn retired_folder_workers_cannot_touch_replacements() {
        let root = test_repo("replacement");
        let retired = Arc::new(());
        let generation = Arc::new(());
        let (stop, _stopped) = std::sync::mpsc::channel();
        let (tx, rx) = async_channel::bounded(1);
        folders().insert(root.clone(), Folder {
            _watcher: stop, generation: generation.clone(), waiters: vec![tx], pending: HashSet::new(),
        });
        remove_folder(&root, &retired);
        assert!(!deliver_pending(&root, &retired, HashSet::from(["stale".into()])));
        assert!(rx.try_recv().is_err());
        assert!(deliver_pending(&root, &generation, HashSet::from(["fresh".into()])));
        assert_eq!(rx.try_recv().unwrap(), vec!["fresh".to_owned()]);
        remove_folder(&root, &generation);
        assert!(!folders().contains_key(&root));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn abandoned_waiters_do_not_leave_an_unbounded_queue() {
        let root = test_repo("overflow");
        install_watch(&root);
        // Inject worker batches: native backends may coalesce real writes, so a
        // filesystem flood cannot deterministically exercise queue overflow.
        let generation = folders().get(&root).unwrap().generation.clone();
        for i in 0..6000 {
            assert!(deliver_pending(&root, &generation, HashSet::from([root.join(format!("file-{i}")).to_string_lossy().into_owned()])));
        }
        let (pending, waiters) = {
            let all = folders();
            let folder = all.get(&root).unwrap();
            (folder.pending.clone(), folder.waiters.len())
        };
        assert_eq!(pending, HashSet::from([root.to_string_lossy().into_owned()]));
        assert_eq!(waiters, 0, "closed waiters must be removed on delivery");
        assert!(invalidates(&block_on(next_change(root.to_string_lossy().into_owned())).unwrap(), &root));
        folders().remove(&root);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn separate_git_metadata_is_watched_without_its_object_tree() {
        let root = test_repo("separate");
        let git = root.with_extension("metadata");
        let _ = std::fs::remove_dir_all(&git);
        assert!(std::process::Command::new("git").arg("init").arg("-q").arg("--separate-git-dir").arg(&git).arg(&root).status().unwrap().success());
        let paths = watch_directories(&root).unwrap();
        assert!(paths.contains(&git));
        assert!(paths.contains(&git.join("refs/heads")));
        assert!(!paths.contains(&git.join("objects")));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(git).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn oversized_trees_reject_instead_of_silently_missing_nested_changes() {
        let root = std::env::temp_dir().join(format!("herdr-watch-limit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..MAX_DIRECTORIES {
            std::fs::create_dir_all(root.join(i.to_string())).unwrap();
        }
        assert!(watch_directories(&root).unwrap_err().contains("live folder updates unavailable"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn native_recursive_backends_register_only_the_root() {
        let root = test_repo("native-recursive");
        std::fs::create_dir_all(root.join("target/deep")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        assert_eq!(folder_watch_mode(), RecursiveMode::Recursive);
        assert_eq!(watch_directories(&root).unwrap(), HashSet::from([root.clone()]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backend_rescan_invalidates_root_and_refreshes_watches() {
        let root = Path::new("/project");
        let mut pending = HashSet::from(["/project/stale".to_owned()]);
        let directories = HashSet::from([root.to_path_buf(), root.join("sub"), PathBuf::from("/external/git")]);
        let mut removed = HashSet::new();
        let event = notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan).add_path(root.join("partial"));
        assert!(collect_event(event, root, &directories, &mut pending, &mut removed));
        assert_eq!(pending, HashSet::from(["/project".to_owned()]));
        assert_eq!(removed, directories);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exact_cap_counts_each_git_directory_once() {
        let root = test_repo("exact-cap");
        let count = watch_directories(&root).unwrap().len();
        for i in count..MAX_DIRECTORIES {
            std::fs::create_dir(root.join(format!("dir-{i}"))).unwrap();
        }
        assert_eq!(watch_directories(&root).unwrap().len(), MAX_DIRECTORIES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renamed_directory_and_descendants_keep_reporting_changes() {
        check_replaced_directory(true);
    }

    #[test]
    fn replaced_directory_and_descendants_keep_reporting_changes() {
        check_replaced_directory(false);
    }

    fn check_replaced_directory(rename: bool) {
        let root = test_repo(if rename { "renamed" } else { "replaced" });
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        install_watch(&root);
        // The old paths survive in the selected set, but now name new inodes.
        if rename {
            std::fs::rename(root.join("sub"), root.join("moved")).unwrap();
        } else {
            std::fs::remove_dir_all(root.join("sub")).unwrap();
        }
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        wait_for_change(&root, &root.join("sub"));
        std::fs::write(root.join("sub/deep/after"), "yes").unwrap();
        wait_for_change(&root, &root.join("sub/deep/after"));
        if rename {
            std::fs::write(root.join("moved/deep/after"), "yes").unwrap();
            wait_for_change(&root, &root.join("moved/deep/after"));
        }
        folders().remove(&root);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Drives a future to its end on this thread (the tests have no executor).
    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        block_on_until(f, std::time::Instant::now() + std::time::Duration::from_secs(10))
    }

    fn block_on_until<F: std::future::Future>(f: F, deadline: std::time::Instant) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
        let mut f = std::pin::pin!(f);
        loop {
            match f.as_mut().poll(&mut Context::from_waker(&waker)) {
                Poll::Ready(v) => return v,
                Poll::Pending => {
                    assert!(std::time::Instant::now() < deadline, "timed out waiting for a folder invalidation");
                    std::thread::park_timeout(std::time::Duration::from_millis(10));
                }
            }
        }
    }

    #[test]
    fn a_watched_folder_reports_changes_and_misses_none_between_waits() {
        let dir = test_repo("between-waits");
        let mut first = Box::pin(next_change(dir.to_string_lossy().into_owned()));
        assert!(first.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())).is_pending());
        std::fs::write(dir.join("a.md"), "one").unwrap();
        let changes = block_on(first).unwrap();
        if !invalidates(&changes, &dir.join("a.md")) {
            wait_for_change(&dir, &dir.join("a.md"));
        }
        // No waiter exists during this write. Wait for its actual delivery,
        // not a fixed sleep or an unrelated leftover batch from the first save.
        std::fs::write(dir.join("b.md"), "two").unwrap();
        wait_for_change(&dir, &dir.join("b.md"));
        folders().remove(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ---- the dirs a mod shows (`watchDirs` / `watchChanges`) ------------------

    fn shown(id: &str, dirs: &[&Path]) {
        set_watch_dirs(id.to_owned(), dirs.iter().map(|p| p.to_path_buf()).collect()).unwrap();
    }

    /// The next change in a shown set, or None when `within` passes first.
    fn clock(id: &str, within: std::time::Duration) -> Vec<String> {
        let deadline = std::time::Instant::now() + within;
        poll_until(next_watch_change(id.to_owned()), deadline).and_then(Result::ok).unwrap_or_default()
    }

    /// Waits for a change in `dir`, touching it again when the backend was still
    /// starting (a watched dir exists before its watch does on some systems).
    fn shown_change(id: &str, dir: &Path, mut touch: impl FnMut()) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let changes = clock(id, std::time::Duration::from_millis(300));
            if changes.iter().any(|c| Path::new(c) == dir) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "no change reported in {}", dir.display());
            touch();
        }
    }

    #[test]
    fn a_changed_path_names_the_shown_dir_it_landed_in() {
        let root = PathBuf::from("/project");
        let watched = std::collections::HashMap::from([
            (root.clone(), root.clone()),
            (root.join("src"), root.join("src")),
        ]);
        assert_eq!(landed_in(&watched, Path::new("/project/src/main.rs")), Some(root.join("src")));
        assert_eq!(landed_in(&watched, Path::new("/project/README.md")), Some(root.clone()));
        // A dir that isn't shown, and its contents, are nobody's business.
        assert_eq!(landed_in(&watched, Path::new("/project/target/deep/x")), None);
        assert_eq!(landed_in(&watched, Path::new("/project/src/deep/x")), None);
        // A watched dir renamed or removed reports itself.
        assert_eq!(landed_in(&watched, Path::new("/project/src")), Some(root.join("src")));
    }

    /// Eats whatever was already reported, so a quiet check starts clean.
    fn drain(id: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if clock(id, std::time::Duration::from_millis(200)).is_empty() {
                return;
            }
        }
        panic!("changes never stopped arriving for {id}");
    }

    #[test]
    fn shown_dirs_report_changes_one_level_deep_only() {
        let root = test_repo("shown-level");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/deep")).unwrap();
        shown("shown-level", &[&root, &root.join("src")]);
        let file = root.join("src/a.txt");
        shown_change("shown-level", &root.join("src"), || std::fs::write(&file, "one").unwrap());
        shown_change("shown-level", &root, || std::fs::write(root.join("b.txt"), "two").unwrap());
        drain("shown-level");
        // `target/` isn't shown, so nothing there wakes the panel that isn't showing it.
        let deep = root.join("target/deep/loud.txt");
        for i in 0..3 {
            std::fs::write(&deep, i.to_string()).unwrap();
            assert!(clock("shown-level", std::time::Duration::from_millis(250)).is_empty(), "a hidden dir woke the panel");
        }
        drop_watch("shown-level");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opening_a_folder_adds_its_watch() {
        let root = test_repo("shown-expand");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        shown("shown-expand", &[&root]);
        // A change inside a folder that isn't open has nothing to redraw.
        let hidden = root.join("sub/x.txt");
        std::fs::write(&hidden, "one").unwrap();
        drain("shown-expand");
        assert!(clock("shown-expand", std::time::Duration::from_millis(300)).is_empty());
        // Opening it: the same call, one dir more.
        shown("shown-expand", &[&root, &root.join("sub")]);
        shown_change("shown-expand", &root.join("sub"), || std::fs::write(&hidden, "two").unwrap());
        drop_watch("shown-expand");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_git_dir_beside_the_checkout_is_watched() {
        let root = test_repo("shown-git");
        let git = root.with_extension("shown-git-dir");
        let _ = std::fs::remove_dir_all(&git);
        std::fs::create_dir_all(&git).unwrap();
        shown("shown-git", &[&root, &git]);
        // A worktree's git dir sits outside the folder; marks still follow it.
        shown_change("shown-git", &git, || std::fs::write(git.join("index"), "marks").unwrap());
        drop_watch("shown-git");
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(git).unwrap();
    }

    #[test]
    fn unwatching_ends_the_wait_and_frees_the_watcher() {
        let root = test_repo("shown-unwatch");
        shown("shown-unwatch", &[&root]);
        let mut wait = Box::pin(next_watch_change("shown-unwatch".to_owned()));
        assert!(wait.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())).is_pending());
        drop_watch("shown-unwatch");
        // A promise nobody resolves would keep a copy's task for the whole session.
        assert!(poll_until(&mut wait, std::time::Instant::now() + std::time::Duration::from_secs(5)).is_some_and(|r| r.is_err()));
        assert!(!shown_registry().contains_key("shown-unwatch"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_shown_dir_that_vanished_stops_being_watched() {
        let root = test_repo("shown-vanish");
        std::fs::create_dir_all(root.join("gone")).unwrap();
        shown("shown-vanish", &[&root, &root.join("gone")]);
        std::fs::remove_dir_all(root.join("gone")).unwrap();
        // The folder's row is gone; passing it again is a set change, not an error.
        set_watch_dirs("shown-vanish".to_owned(), vec![root.clone(), root.join("gone")]).unwrap();
        shown_change("shown-vanish", &root, || std::fs::write(root.join("after.txt"), "yes").unwrap());
        drop_watch("shown-vanish");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Polls a future until it is ready or `deadline` passes.
    fn poll_until<F: std::future::Future>(f: F, deadline: std::time::Instant) -> Option<F::Output> {
        use std::task::{Context, Poll, Wake, Waker};
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
        let mut f = std::pin::pin!(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut Context::from_waker(&waker)) {
                return Some(v);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::park_timeout(std::time::Duration::from_millis(10));
        }
    }
}
