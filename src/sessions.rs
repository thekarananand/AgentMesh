//! Reads Claude Code's own records under `~/.claude` into one row per session.
//! Read-only: Claude Code owns all of this state. Unknown or unparseable data degrades, never errors.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

/// How much of a transcript's tail is scanned for titles, cwd and branch.
const TAIL_BYTES: u64 = 96 * 1024;
/// Characters of a row title; the sidebar ellipsizes long before this.
const TITLE_MAX: usize = 120;
/// Characters of a first prompt kept for its title.
const PROMPT_MAX: usize = 200;
/// Ended sessions shown under HISTORY at first; "Show more" adds this many each time.
pub const RECENT_LIMIT: usize = 40;
/// Claude Code truncates longer project slugs and appends a hash.
const SLUG_MAX: usize = 200;
const SOCKET_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Busy,
    Waiting,
    Idle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    /// Some while the process is alive.
    pub pid: Option<u32>,
    /// None once the session has ended.
    pub status: Option<Status>,
    pub background: bool,
    pub cwd: PathBuf,
    pub branch: Option<String>,
    /// Deliberate name (not Claude Code's cwd-derived autoname).
    pub name: Option<String>,
    /// The name in the pid file right now, whatever its source.
    pub live_name: Option<String>,
    /// Claude Code's own summary title; it never promotes it to the name.
    pub ai_title: Option<String>,
    pub first_prompt: Option<String>,
    /// Control socket of a live session (accepts rename).
    pub socket: Option<PathBuf>,
    /// The session's transcript, once it has one.
    pub transcript: Option<PathBuf>,
    pub title: String,
    pub updated: SystemTime,
}

impl Row {
    pub fn live(&self) -> bool {
        self.status.is_some()
    }
}

#[derive(Clone, Default)]
struct Transcript {
    name: Option<String>,
    ai_title: Option<String>,
    cwd: Option<PathBuf>,
    branch: Option<String>,
    modified: Option<SystemTime>,
}

impl Transcript {
    fn complete(&self) -> bool {
        self.name.is_some() && self.ai_title.is_some() && self.cwd.is_some()
    }
}

/// State kept between polls so unchanged files are not parsed again.
#[derive(Default)]
pub struct Cache {
    prompts: Prompts,
    /// Transcript path to its parse, valid while the mtime matches.
    transcripts: HashMap<PathBuf, (SystemTime, Transcript)>,
}

impl Cache {
    fn transcript(&mut self, path: &Path, modified: SystemTime) -> Transcript {
        if let Some((cached, transcript)) = self.transcripts.get(path)
            && *cached == modified
        {
            return transcript.clone();
        }
        let transcript = read_transcript(path, modified);
        self.transcripts
            .insert(path.to_path_buf(), (modified, transcript.clone()));
        transcript
    }
}

/// Home, or `/` when unknown, so `.claude` never resolves against the process cwd.
pub fn home_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| "/".into())
}

fn claude_dir() -> PathBuf {
    home_dir().join(".claude")
}

/// Where Claude Code keeps one pid file per running session, symlinks resolved (FSEvents
/// reports real paths).
pub(crate) fn sessions_dir() -> PathBuf {
    let dir = claude_dir().join("sessions");
    fs::canonicalize(&dir).unwrap_or(dir)
}

/// One project's sessions: live ones first, then its most recently touched transcripts.
/// At most `limit` ended sessions.
pub fn list(folder: &Path, limit: usize, cache: &mut Cache) -> Vec<Row> {
    let folder = &fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    let dir = claude_dir();
    let live = read_live(&dir.join("sessions"));
    let transcripts = project_transcripts(&dir.join("projects"), folder);
    cache.prompts.refresh(&dir.join("history.jsonl"), folder);

    let mut rows: Vec<Row> = Vec::new();
    for entry in &live {
        let transcript = match transcripts.get(&entry.id) {
            Some((path, modified)) => cache.transcript(path, *modified),
            None => Transcript::default(),
        };
        let mut row = build_row(&entry.id, Some(entry), transcript, &cache.prompts);
        row.transcript = transcripts.get(&entry.id).map(|(path, _)| path.clone());
        // Live sessions started in another folder belong to another window. One with a
        // transcript in this project may have `cd`ed into a subfolder.
        if same_dir(&row.cwd, folder)
            || (transcripts.contains_key(&entry.id) && inside(&row.cwd, folder))
        {
            rows.push(row);
        }
    }
    let live_ids: HashSet<&str> = live.iter().map(|entry| entry.id.as_str()).collect();
    let mut ended: Vec<(&String, &PathBuf, SystemTime)> = transcripts
        .iter()
        .filter(|(id, _)| !live_ids.contains(id.as_str()))
        .map(|(id, (path, modified))| (id, path, *modified))
        .collect();
    ended.sort_by_key(|(_, _, time)| std::cmp::Reverse(*time));
    // A slug can be shared by two folders (`a-b` and `a/b`); the recorded cwd decides. It is
    // the last one, often a subfolder after a `cd`, and absent from a short transcript's tail.
    // Lazy: transcripts past the limit are never parsed.
    let kept: Vec<(&String, &PathBuf, Transcript)> = ended
        .into_iter()
        .map(|(id, path, modified)| (id, path, cache.transcript(path, modified)))
        .filter(|(_, _, transcript)| {
            transcript
                .cwd
                .as_deref()
                .is_none_or(|cwd| inside(cwd, folder))
        })
        .take(limit)
        .collect();
    rows.extend(kept.into_iter().map(|(id, path, transcript)| Row {
        transcript: Some(path.clone()),
        ..build_row(id, None, transcript, &cache.prompts)
    }));
    // Forget transcripts that dropped out of the list.
    let known: HashSet<&PathBuf> = transcripts.values().map(|(path, _)| path).collect();
    cache.transcripts.retain(|path, _| known.contains(path));
    rows.sort_by_key(|row| (!row.live(), std::cmp::Reverse(row.updated)));
    rows
}

/// Whether the session is running right now, read fresh (the polled rows can be 2s old).
pub fn is_live(session_id: &str) -> bool {
    read_live(&claude_dir().join("sessions"))
        .iter()
        .any(|entry| entry.id == session_id)
}

fn build_row(
    id: &str,
    entry: Option<&LiveEntry>,
    transcript: Transcript,
    prompts: &Prompts,
) -> Row {
    // The registry is fresher than the transcript while the session is live.
    let name = entry
        .and_then(|entry| entry.name.clone())
        .or(transcript.name);
    let first_prompt = prompts.first.get(id).cloned();
    let title = name
        .clone()
        .or_else(|| transcript.ai_title.clone())
        .or_else(|| first_prompt.clone())
        .unwrap_or_else(|| short_id(id).to_string());
    Row {
        id: id.to_string(),
        pid: entry.map(|entry| entry.pid),
        status: entry.map(|entry| entry.status),
        background: entry.is_some_and(|entry| entry.background),
        cwd: entry
            .and_then(|entry| entry.cwd.clone())
            .or(transcript.cwd)
            .unwrap_or_else(home_dir),
        branch: transcript.branch,
        name,
        live_name: entry.and_then(|entry| entry.live_name.clone()),
        ai_title: transcript.ai_title,
        first_prompt,
        socket: entry.and_then(|entry| entry.socket.clone()),
        transcript: None,
        title: one_line_title(&title),
        updated: entry
            .and_then(|entry| entry.updated)
            .into_iter()
            .chain(transcript.modified)
            .max()
            .unwrap_or(UNIX_EPOCH),
    }
}

/// Background agents' names and first prompts can be a whole prompt: one line, at most
/// `TITLE_MAX` chars. Control and bidi-override chars are dropped so a title can't reorder
/// itself (U+202E) in the sidebar or the window title.
fn one_line_title(title: &str) -> String {
    let clean: String = title
        .chars()
        .filter(|c| !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    match words.char_indices().nth(TITLE_MAX) {
        Some((cut, _)) => format!("{}\u{2026}", &words[..cut]),
        None => words,
    }
}

/// Same folder as the canonical `folder`, also through symlinks and trailing slashes.
fn same_dir(path: &Path, folder: &Path) -> bool {
    path == folder || fs::canonicalize(path).is_ok_and(|path| path == folder)
}

/// `path` is the canonical `folder` or somewhere under it.
fn inside(path: &Path, folder: &Path) -> bool {
    path.starts_with(folder) || fs::canonicalize(path).is_ok_and(|path| path.starts_with(folder))
}

/// Claude Code's session ids are UUIDs. Anything else in a project dir (legacy `agent-*` files,
/// a stem starting with `-` that `--resume` would read as a flag) is not resumable.
pub fn is_session_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

pub(crate) fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

struct LiveEntry {
    id: String,
    pid: u32,
    status: Status,
    background: bool,
    cwd: Option<PathBuf>,
    name: Option<String>,
    live_name: Option<String>,
    socket: Option<PathBuf>,
    updated: Option<SystemTime>,
}

/// `~/.claude/sessions/<pid>.json`. A file can outlive a crashed process, so each pid is probed.
fn read_live(dir: &Path) -> Vec<LiveEntry> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut live: Vec<LiveEntry> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some(json) = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        let Some(entry) = parse_live(&json) else {
            continue;
        };
        if !alive(entry.pid, json["startedAt"].as_u64()) {
            continue;
        }
        // One session under two pids (a leftover file): the fresher record wins.
        match live.iter_mut().find(|known| known.id == entry.id) {
            Some(known) if known.updated < entry.updated => *known = entry,
            Some(_) => {}
            None => live.push(entry),
        }
    }
    live
}

/// One pid file's record; None without a usable pid and session id. Liveness is checked apart.
fn parse_live(json: &Value) -> Option<LiveEntry> {
    let pid = json["pid"].as_u64().and_then(valid_pid)?;
    let id = json["sessionId"].as_str()?;
    // `derived` (cwd-based) and `collision` are Claude Code's own placeholders.
    let deliberate = !matches!(json["nameSource"].as_str(), Some("derived" | "collision"));
    let live_name = json["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(String::from);
    Some(LiveEntry {
        id: id.to_string(),
        pid,
        // Unknown or future statuses count as working, not done.
        status: match json["status"].as_str() {
            Some("idle") => Status::Idle,
            Some("waiting") => Status::Waiting,
            _ => Status::Busy,
        },
        background: json["kind"].as_str() == Some("bg"),
        cwd: json["cwd"].as_str().map(PathBuf::from),
        name: live_name.clone().filter(|_| deliberate),
        live_name,
        socket: json["messagingSocketPath"].as_str().map(PathBuf::from),
        updated: json["updatedAt"]
            .as_u64()
            .map(|ms| UNIX_EPOCH + Duration::from_millis(ms)),
    })
}

/// A pid `kill` can probe: 0 and anything past `i32::MAX` would signal a process group.
fn valid_pid(pid: u64) -> Option<u32> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    Some(pid as u32)
}

/// The process exists and is not a later process that reused the pid.
fn alive(pid: u32, started_ms: Option<u64>) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as i32, 0) };
    // EPERM means another user's process: pid files are this user's, so the pid was reused.
    // A reused pid otherwise belongs to a process that started after the session did.
    result == 0
        && match (process_start(pid), started_ms) {
            (Some(start), Some(started_ms)) => start <= started_ms / 1000 + 1,
            // Start time unreadable on macOS: can't rule out reuse, so not live.
            (None, Some(_)) => !cfg!(target_os = "macos"),
            (_, None) => true,
        }
}

/// Process start time in Unix seconds.
#[cfg(target_os = "macos")]
fn process_start(pid: u32) -> Option<u64> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: the buffer is a zeroed `proc_bsdinfo` of exactly `size` bytes.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    (written == size).then_some(info.pbi_start_tvsec)
}

#[cfg(not(target_os = "macos"))]
fn process_start(_: u32) -> Option<u64> {
    None
}

/// Claude Code's directory name for a project: every non-alphanumeric UTF-16 unit becomes `-`
/// (a JS regex replace), so an emoji outside the BMP becomes two.
fn project_slug(folder: &Path) -> String {
    folder
        .to_string_lossy()
        .chars()
        .flat_map(|c| {
            let (c, count) = if c.is_ascii_alphanumeric() {
                (c, 1)
            } else {
                ('-', c.len_utf16())
            };
            std::iter::repeat_n(c, count)
        })
        .collect()
}

/// Session id to (transcript path, mtime) for the project's `projects/<slug>/<id>.jsonl`.
fn project_transcripts(dir: &Path, folder: &Path) -> HashMap<String, (PathBuf, SystemTime)> {
    let slug = project_slug(folder);
    let mut project_dirs = vec![dir.join(&slug)];
    if slug.len() > SLUG_MAX {
        // Long slugs are cut and suffixed with a hash; match on the cut prefix.
        let prefix = &slug[..SLUG_MAX];
        project_dirs = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
            .map(|entry| entry.path())
            .collect();
    }
    let mut transcripts = HashMap::new();
    for project in project_dirs {
        let Ok(files) = fs::read_dir(project) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(id) = path.file_stem().and_then(|stem| stem.to_str())
                && is_session_id(id)
            {
                let modified = file
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .unwrap_or(UNIX_EPOCH);
                transcripts.insert(id.to_string(), (path, modified));
            }
        }
    }
    transcripts
}

/// Scans the transcript tail backwards; the last record of each kind wins.
fn read_transcript(path: &Path, modified: SystemTime) -> Transcript {
    let mut transcript = Transcript {
        modified: Some(modified),
        ..Default::default()
    };
    let Some(tail) = read_tail(path) else {
        return transcript;
    };
    for line in tail.lines().rev() {
        if transcript.complete() {
            break;
        }
        // Most lines are messages and tool results, often huge. Once cwd and branch are known
        // only the small title records matter, so the rest skip the JSON parse.
        if transcript.cwd.is_some() && !line.contains("title\"") && !line.contains("\"agent-name\"")
        {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let text = |key: &str| record[key].as_str().map(String::from);
        match record["type"].as_str() {
            Some("custom-title") if transcript.name.is_none() => {
                transcript.name = text("customTitle")
            }
            Some("agent-name") if transcript.name.is_none() => transcript.name = text("agentName"),
            Some("ai-title") if transcript.ai_title.is_none() => {
                transcript.ai_title = text("aiTitle")
            }
            _ => {}
        }
        // Branch from the same record as the cwd: both describe where the session was last.
        if transcript.cwd.is_none()
            && let Some(cwd) = text("cwd")
        {
            transcript.cwd = Some(PathBuf::from(cwd));
            transcript.branch = text("gitBranch").filter(|branch| !branch.is_empty());
        }
    }
    transcript
}

fn read_tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    // The first line is likely cut mid-record; the JSON parse skips it.
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// First prompt per session from `history.jsonl`, for one project. The file only grows, so each
/// poll parses the appended lines only.
#[derive(Default)]
struct Prompts {
    first: HashMap<String, String>,
    /// Bytes parsed so far; always at a line boundary.
    offset: u64,
    /// The project these prompts are for; history holds every project's.
    project: PathBuf,
    /// Inode of the file parsed; an atomic rewrite swaps it, and the offset then means nothing.
    inode: u64,
}

impl Prompts {
    fn refresh(&mut self, path: &Path, project: &Path) {
        if self.project != project {
            *self = Self {
                project: project.to_path_buf(),
                ..Self::default()
            };
        }
        let Ok(mut file) = File::open(path) else {
            return;
        };
        let Ok(meta) = file.metadata() else {
            return;
        };
        let len = meta.len();
        if len < self.offset || meta.ino() != self.inode {
            // Replaced, rewritten or truncated: start over.
            self.first.clear();
            self.offset = 0;
            self.inode = meta.ino();
        }
        if len == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            return;
        }
        // A line still being written is left for the next poll.
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |i| i + 1);
        for line in bytes[..complete].split(|byte| *byte == b'\n') {
            let Ok(record) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if record["project"].as_str() != self.project.to_str() {
                continue;
            }
            if let (Some(id), Some(prompt)) =
                (record["sessionId"].as_str(), record["display"].as_str())
            {
                // Only ever a one-line title; pasted prompts can run to megabytes.
                self.first
                    .entry(id.to_string())
                    .or_insert_with(|| prompt.chars().take(PROMPT_MAX).collect());
            }
        }
        self.offset += complete as u64;
    }
}

/// Name guess in Claude Code's own style: kebab-case, at most 4 words.
pub fn kebab_name(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(4)
        .map(str::to_lowercase)
        .collect();
    (!words.is_empty()).then(|| words.join("-"))
}

/// A first prompt worth naming a session after (not a slash command or a paste placeholder).
pub fn nameable_prompt(prompt: &str) -> Option<&str> {
    let prompt = prompt.trim();
    (!prompt.is_empty() && !prompt.starts_with('/') && !prompt.starts_with("[Pasted"))
        .then_some(prompt)
}

/// Renames a live session over its control socket. Claude Code updates its banner, terminal
/// title, pid file and transcript itself. No ack; the next poll shows the result.
pub fn rename(socket: &Path, session_id: &str, name: &str) -> std::io::Result<()> {
    let message = serde_json::json!({
        "type": "control",
        "action": "rename",
        "name": name,
        "session_id": session_id,
    });
    let mut stream = UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    writeln!(stream, "{message}")
}

pub fn relative_time(time: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(time)
        .unwrap_or_default()
        .as_secs();
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

/// Started today, local time; for the Today / Earlier split in HISTORY.
pub fn is_today(time: SystemTime) -> bool {
    let day = |time: SystemTime| {
        let seconds = time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as libc::time_t;
        // SAFETY: `localtime_r` writes only into the zeroed `tm` we pass.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&seconds, &mut tm) };
        (tm.tm_year, tm.tm_yday)
    };
    day(time) == day(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_dir_sees_through_symlinks_and_slashes() {
        assert!(same_dir(Path::new("/tmp"), Path::new("/private/tmp")));
        assert!(same_dir(Path::new("/usr/"), Path::new("/usr")));
        assert!(!same_dir(Path::new("/usr"), Path::new("/usr/bin")));
    }

    #[test]
    fn inside_accepts_subfolders_only() {
        assert!(inside(Path::new("/usr/bin"), Path::new("/usr")));
        assert!(inside(Path::new("/tmp"), Path::new("/private/tmp")));
        assert!(!inside(Path::new("/usr-local"), Path::new("/usr")));
        assert!(!inside(Path::new("/"), Path::new("/usr")));
    }

    #[test]
    fn only_uuids_are_session_ids() {
        assert!(is_session_id("c69e86d6-0f1a-4b2c-9d3e-4f5a6b7c8d9e"));
        assert!(!is_session_id("--dangerously-skip-permissions-xxxxxx"));
        assert!(!is_session_id("agent-1234"));
        assert!(!is_session_id("c69e86d6x0f1a-4b2c-9d3e-4f5a6b7c8d9e"));
    }

    #[test]
    fn names_are_short_kebab_case() {
        assert_eq!(
            kebab_name("Fix the login bug, then deploy").as_deref(),
            Some("fix-the-login-bug")
        );
        assert_eq!(kebab_name("Project PRD").as_deref(), Some("project-prd"));
        assert_eq!(kebab_name(" -- "), None);
        assert_eq!(nameable_prompt("/clear"), None);
        assert_eq!(nameable_prompt("[Pasted text #6 +8 lines]"), None);
    }

    #[test]
    fn bad_pids_are_rejected() {
        assert_eq!(valid_pid(0), None);
        assert_eq!(valid_pid(u64::from(u32::MAX)), None);
        assert_eq!(valid_pid(1 << 40), None);
        assert_eq!(valid_pid(4242), Some(4242));
    }

    #[test]
    fn reused_pid_is_not_alive() {
        let me = std::process::id();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(alive(me, Some(now_ms)));
        // A session that "started" an hour before this process did.
        assert!(!alive(me, Some(now_ms - 3_600_000 * 24)));
    }

    #[test]
    fn slug_matches_claude_code() {
        assert_eq!(
            project_slug(Path::new(
                "/Users/me/Desktop/AgentMesh-v2/code/Claude_Plane"
            )),
            "-Users-me-Desktop-AgentMesh-v2-code-Claude-Plane"
        );
        // JS replaces per UTF-16 unit: one for é, two for an emoji.
        assert_eq!(
            project_slug(Path::new("/caf\u{e9}/\u{1f680}")),
            "-caf--\u{2d}-"
        );
    }

    #[test]
    fn pid_files_parse() {
        let entry = parse_live(&serde_json::json!({
            "pid": 4242, "sessionId": "s", "status": "waiting", "kind": "bg",
            "name": "fix-login", "nameSource": "user", "cwd": "/p", "updatedAt": 1000,
        }))
        .unwrap();
        assert_eq!(entry.status, Status::Waiting);
        assert!(entry.background);
        assert_eq!(entry.name.as_deref(), Some("fix-login"));
        assert_eq!(entry.updated, Some(UNIX_EPOCH + Duration::from_secs(1)));
        // Claude Code's placeholder names are not deliberate; unknown statuses mean working.
        let entry = parse_live(&serde_json::json!({
            "pid": 1, "sessionId": "s", "status": "thinking", "name": "p", "nameSource": "derived",
        }))
        .unwrap();
        assert_eq!(entry.status, Status::Busy);
        assert_eq!((entry.name, entry.live_name.as_deref()), (None, Some("p")));
        assert!(parse_live(&serde_json::json!({ "pid": 0, "sessionId": "s" })).is_none());
        assert!(parse_live(&serde_json::json!({ "pid": 7 })).is_none());
    }

    #[test]
    fn row_titles_prefer_name_then_ai_title_then_prompt() {
        let mut prompts = Prompts::default();
        prompts.first.insert("s".into(), "first prompt".into());
        let title = |name: Option<&str>, ai: Option<&str>, prompts: &Prompts| {
            let transcript = Transcript {
                name: name.map(String::from),
                ai_title: ai.map(String::from),
                ..Default::default()
            };
            build_row("s", None, transcript, prompts).title
        };
        assert_eq!(title(Some("named"), Some("AI"), &prompts), "named");
        assert_eq!(title(None, Some("AI"), &prompts), "AI");
        assert_eq!(title(None, None, &prompts), "first prompt");
        assert_eq!(title(None, None, &Prompts::default()), "s");
    }

    #[test]
    fn relative_times_round_down() {
        let ago = |seconds| relative_time(SystemTime::now() - Duration::from_secs(seconds));
        assert_eq!(ago(59), "now");
        assert_eq!(ago(60), "1m");
        assert_eq!(ago(3599), "59m");
        assert_eq!(ago(3600), "1h");
        assert_eq!(ago(86400 * 3), "3d");
        assert!(is_today(SystemTime::now()));
        assert!(!is_today(
            SystemTime::now() - Duration::from_secs(86400 * 2)
        ));
    }

    #[test]
    fn titles_are_one_clean_line() {
        assert_eq!(one_line_title("fix\n  the\tbug"), "fix the bug");
        assert_eq!(one_line_title("abc\u{202e}def\u{2066}"), "abcdef");
        let long = "word ".repeat(100);
        let title = one_line_title(&long);
        assert_eq!(title.chars().count(), TITLE_MAX + 1);
        assert!(title.ends_with('\u{2026}'));
    }

    #[test]
    fn transcript_reads_titles_and_last_cwd() {
        let dir = std::env::temp_dir().join(format!("ccp-transcript-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"custom-title\",\"customTitle\":\"mine\"}\n",
                "{\"type\":\"user\",\"cwd\":\"/old\",\"gitBranch\":\"old\"}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Summary\"}\n",
                "{\"type\":\"assistant\",\"cwd\":\"/new\",\"gitBranch\":\"\"}\n",
                "{\"type\":\"assistant\",\"message\":\"no cwd\"}\n",
            ),
        )
        .unwrap();
        let transcript = read_transcript(&path, UNIX_EPOCH);
        assert_eq!(transcript.name.as_deref(), Some("mine"));
        assert_eq!(transcript.ai_title.as_deref(), Some("Summary"));
        assert_eq!(transcript.cwd.as_deref(), Some(Path::new("/new")));
        // Empty branch on the cwd's record: none, not an older record's.
        assert_eq!(transcript.branch, None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prompts_parse_only_appended_lines() {
        let dir = std::env::temp_dir().join(format!("ccp-prompts-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.jsonl");
        fs::write(
            &path,
            "{\"project\":\"/p\",\"sessionId\":\"a\",\"display\":\"first\"}\n{\"project\":\"/p\",\"sessionId\":\"a\",",
        )
        .unwrap();
        let mut prompts = Prompts::default();
        let project = Path::new("/p");
        prompts.refresh(&path, project);
        assert_eq!(prompts.first.get("a").map(String::as_str), Some("first"));
        // The half-written line completes; a bad byte elsewhere wipes nothing.
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(
            b"\"display\":\"second\"}\n\xff\xfe\n{\"project\":\"/p\",\"sessionId\":\"b\",\"display\":\"hi\"}\n{\"project\":\"/q\",\"sessionId\":\"c\",\"display\":\"other\"}\n",
        )
        .unwrap();
        prompts.refresh(&path, project);
        assert_eq!(prompts.first.get("a").map(String::as_str), Some("first"));
        assert_eq!(prompts.first.get("b").map(String::as_str), Some("hi"));
        // Other projects' prompts are not kept.
        assert_eq!(prompts.first.get("c"), None);
        fs::remove_dir_all(&dir).ok();
    }
}
