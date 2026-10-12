//! Finding `claude` and the environment to run it in. The user's login shell is asked once per
//! window; tabs then start `claude` directly with that environment, so no shell parses their
//! arguments (fish and csh quote differently) and no shell stays between the tab and claude.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// A login shell that hangs (prompt in an rc file, network mount) must not hang the app.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
/// After the login shell exits, how long its output may stay silent before reading stops.
const OUTPUT_GRACE: Duration = Duration::from_millis(200);
/// Printed between the lookup and the environment.
const MARKER: &str = "__claude_control_plane_env__";
/// Variables describing the asking shell, not the tab: its working directory and depth.
const SHELL_ONLY_ENV: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];
/// What tabs advertise. The grid is alacritty's, but `alacritty` terminfo isn't installed
/// everywhere and claude would take the tab for an Alacritty window.
const TAB_ENV: [(&str, &str); 2] = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")];

/// `claude` and the environment the user's login shell gives it.
#[derive(Clone, Debug)]
pub struct Claude {
    pub path: PathBuf,
    pub env: HashMap<String, String>,
}

/// Asks the login shell for `claude` and its environment. The error says why, for the sidebar.
pub fn resolve() -> Result<Claude, String> {
    let shell = login_shell();
    let csh = Path::new(&shell)
        .file_name()
        .is_some_and(|name| name == "csh" || name == "tcsh");
    // csh rejects `-l` with other flags and has no `command -v`; `-c` still reads its rc file.
    let (flags, lookup): (&[&str], &str) = if csh {
        (&["-c"], "which claude")
    } else {
        (&["-l", "-c"], "command -v claude")
    };
    let mut child = Command::new(&shell)
        .args(flags)
        .arg(format!("{lookup}; echo {MARKER}; env -0"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Can't run {shell}: {error}"))?;
    // Drained on its own thread: rc output past the pipe buffer would block the shell, and a
    // process an rc file backgrounds can hold the pipe open long after the shell exits.
    let Some(output) = child.stdout.take().map(drain) else {
        return Err(format!("{shell}: no stdout"));
    };
    let deadline = Instant::now() + RESOLVE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                child.kill().ok();
                child.wait().ok();
                return Err(format!(
                    "{shell} took over {}s to start",
                    RESOLVE_TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("{shell}: {error}")),
        }
    }
    parse_output(&collect_output(&output))
}

/// The shell's output: banners and the claude path, the marker, then `env -0`.
fn parse_output(lines: &[String]) -> Result<Claude, String> {
    let not_found = || "Claude Code not found on the login shell's PATH.".to_string();
    let marker = lines
        .iter()
        .position(|line| line == MARKER)
        .ok_or_else(not_found)?;
    // rc files may print banners first; the path is the last line before the marker.
    let path = lines[..marker]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .filter(|path| path.starts_with('/'))
        .ok_or_else(not_found)?;
    // `env -0` separates entries with NUL; values may span lines.
    let env = lines[marker + 1..]
        .join("\n")
        .split('\0')
        .filter_map(|entry| entry.split_once('='))
        .filter(|(name, _)| !name.is_empty() && !SHELL_ONLY_ENV.contains(name))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .chain(TAB_ENV.map(|(name, value)| (name.to_string(), value.to_string())))
        .collect();
    Ok(Claude {
        path: PathBuf::from(path),
        env,
    })
}

/// Lines of `pipe`, read on their own thread so a reader never blocks on it.
fn drain(pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (lines, output) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    output
}

/// Everything an exited process printed is already in its pipe: read until EOF or until it
/// goes quiet, whichever comes first.
fn collect_output(output: &mpsc::Receiver<String>) -> Vec<String> {
    std::iter::from_fn(|| output.recv_timeout(OUTPUT_GRACE).ok()).collect()
}

fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_read_survives_a_shell_that_leaves_stdout_open() {
        // `sleep` inherits stdout and outlives the shell; reading to EOF would wait for it.
        let started = Instant::now();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "echo /bin/true; sleep 30 &"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let output = drain(child.stdout.take().unwrap());
        child.wait().unwrap();
        assert_eq!(collect_output(&output), ["/bin/true"]);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn output_gives_path_and_env() {
        let lines: Vec<String> = [
            "Welcome banner",
            "/opt/bin/claude",
            MARKER,
            "PATH=/opt/bin:/usr/bin\0MULTI=a",
            "b\0PWD=/somewhere\0TERM=alacritty\0",
        ]
        .map(String::from)
        .to_vec();
        let claude = parse_output(&lines).unwrap();
        assert_eq!(claude.path, Path::new("/opt/bin/claude"));
        assert_eq!(claude.env["PATH"], "/opt/bin:/usr/bin");
        assert_eq!(claude.env["MULTI"], "a\nb");
        assert_eq!(claude.env["TERM"], "xterm-256color");
        assert!(!claude.env.contains_key("PWD"));
        assert!(parse_output(&[MARKER.to_string()]).is_err());
        assert!(parse_output(&["claude: aliased".into(), MARKER.into()]).is_err());
    }

    /// The real login shell (and tcsh, which can't take `-l -c`) finds a program on PATH.
    #[test]
    fn shells_report_path_and_env() {
        for shell in ["/bin/zsh", "/bin/tcsh"] {
            if !Path::new(shell).exists() {
                continue;
            }
            let csh = shell.ends_with("csh");
            let (flags, lookup): (&[&str], &str) = if csh {
                (&["-c"], "which ls")
            } else {
                (&["-l", "-c"], "command -v ls")
            };
            let out = Command::new(shell)
                .args(flags)
                .arg(format!("{lookup}; echo {MARKER}; env -0"))
                .output()
                .unwrap();
            let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(String::from)
                .collect();
            let found = parse_output(&lines).unwrap();
            assert!(found.path.ends_with("ls"), "{shell}: {:?}", found.path);
            assert!(found.env.contains_key("PATH"), "{shell}");
        }
    }
}
