//! Picks a name for a tab's session as early as possible: from the first prompt as soon as it is
//! sent, then once more from Claude Code's AI title when that lands. A name the user set (anything
//! other than the one we sent) is never touched.

use crate::sessions::{self, Row};

/// A name this app gave a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Given {
    pub name: String,
    /// From the AI title; nothing replaces it.
    pub ai: bool,
}

/// The name to send now, if any. `given` is what this app sent the session before.
pub fn next_name(row: &Row, given: Option<&Given>) -> Option<Given> {
    let ai_name = || row.ai_title.as_deref().and_then(sessions::kebab_name);
    let prompt_name =
        || sessions::kebab_name(sessions::nameable_prompt(row.first_prompt.as_deref()?)?);
    match given {
        // Deliberate name already (user `/rename`, `--name`, or resumed with one). After a
        // restart nothing is remembered, so a name equal to the prompt-derived one is ours.
        None if row.name.is_some() => {
            let ours = prompt_name().filter(|name| row.name.as_ref() == Some(name))?;
            next_name(
                row,
                Some(&Given {
                    name: ours,
                    ai: false,
                }),
            )
        }
        None => ai_name()
            .map(|name| Given { name, ai: true })
            .or_else(|| prompt_name().map(|name| Given { name, ai: false })),
        // Still ours (the user hasn't renamed it): upgrade to the AI title once it exists.
        Some(Given { name, ai: false }) if row.live_name.as_ref() == Some(name) => ai_name()
            .filter(|ai_name| ai_name != name)
            .map(|name| Given { name, ai: true }),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::UNIX_EPOCH};

    use super::*;

    fn row() -> Row {
        Row {
            id: "id".into(),
            pid: Some(1),
            status: None,
            background: false,
            cwd: PathBuf::new(),
            branch: None,
            name: None,
            live_name: None,
            ai_title: None,
            first_prompt: None,
            socket: None,
            transcript: None,
            title: String::new(),
            updated: UNIX_EPOCH,
        }
    }

    fn given(name: &str, ai: bool) -> Given {
        Given {
            name: name.into(),
            ai,
        }
    }

    #[test]
    fn prompt_first_then_ai_title() {
        let mut row = row();
        assert_eq!(next_name(&row, None), None);
        row.first_prompt = Some("Fix the login bug please".into());
        assert_eq!(
            next_name(&row, None),
            Some(given("fix-the-login-bug", false))
        );

        let ours = given("fix-the-login-bug", false);
        row.live_name = Some(ours.name.clone());
        assert_eq!(next_name(&row, Some(&ours)), None);
        row.ai_title = Some("Login bug fix".into());
        assert_eq!(
            next_name(&row, Some(&ours)),
            Some(given("login-bug-fix", true))
        );
        assert_eq!(next_name(&row, Some(&given("login-bug-fix", true))), None);
    }

    #[test]
    fn user_names_are_kept() {
        let mut row = row();
        row.name = Some("mine".into());
        row.first_prompt = Some("hello".into());
        assert_eq!(next_name(&row, None), None);

        // Renamed after we named it: the pid file no longer shows our name.
        let mut row = super::tests::row();
        row.live_name = Some("mine".into());
        row.ai_title = Some("Something".into());
        assert_eq!(next_name(&row, Some(&given("hello", false))), None);
    }

    #[test]
    fn prompt_name_upgrades_after_restart() {
        // Named from the prompt before a restart; `autonamed` is empty now.
        let mut row = row();
        row.first_prompt = Some("Fix the login bug please".into());
        row.name = Some("fix-the-login-bug".into());
        row.live_name = row.name.clone();
        assert_eq!(next_name(&row, None), None);
        row.ai_title = Some("Login bug fix".into());
        assert_eq!(next_name(&row, None), Some(given("login-bug-fix", true)));
        // A user name that differs stays.
        row.name = Some("mine".into());
        row.live_name = row.name.clone();
        assert_eq!(next_name(&row, None), None);
    }

    #[test]
    fn slash_commands_do_not_name() {
        let mut row = row();
        row.first_prompt = Some("/clear".into());
        assert_eq!(next_name(&row, None), None);
    }
}
