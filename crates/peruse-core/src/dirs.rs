//! Where Peruse keeps its files, and where the user lives.
//!
//! Each system puts the settings of a program in a different place, and each
//! system says where through an environment variable. This module reads those
//! variables. It calls no function of the operating system, so it needs no
//! crate of bindings and no code that is not safe.
//!
//! A crate for this work exists. Peruse does not use one, for a reason that
//! matters to a user who wants to run this program at work: the usual crate
//! reaches `option-ext`, which carries the Mozilla Public Licence. That
//! licence is weak copyleft. It does not reach the code of Peruse, but a
//! program that examines the licences of a project reports it, and somebody
//! then has to answer for it. Twenty lines of environment variables remove
//! that question. Each crate that Peruse uses now allows a program to be
//! given away with no such condition.
//!
//! The paths below are the same paths that the usual crate gives, so a user
//! who had an older version of Peruse keeps their settings.

use std::path::PathBuf;

/// Gives the value of an environment variable, when it holds something.
fn var(name: &str) -> Option<PathBuf> {
    let v = std::env::var_os(name)?;
    if v.is_empty() {
        return None;
    }
    Some(PathBuf::from(v))
}

/// Gives the directory of the user.
///
/// | System | The variable |
/// |---|---|
/// | Windows | `USERPROFILE` |
/// | Each other | `HOME` |
pub fn home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        var("USERPROFILE").or_else(|| {
            // Some systems give the two parts and not the whole path.
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            let mut s = drive;
            s.push(path);
            Some(PathBuf::from(s))
        })
    } else {
        var("HOME")
    }
}

/// Gives the directory that holds the settings of Peruse.
///
/// | System | The directory |
/// |---|---|
/// | Windows | `%APPDATA%\peruse\config` |
/// | macOS | `~/Library/Application Support/peruse` |
/// | Each other | `$XDG_CONFIG_HOME/peruse`, or `~/.config/peruse` |
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        // The part `config` at the end is the form that Peruse used before,
        // and a user keeps the settings that they have.
        return Some(var("APPDATA")?.join("peruse").join("config"));
    }
    if cfg!(target_os = "macos") {
        return Some(
            home_dir()?
                .join("Library")
                .join("Application Support")
                .join("peruse"),
        );
    }
    // The specification says that a path in XDG_CONFIG_HOME counts only when
    // it is a complete path. A path that is not complete is the same as no
    // value at all.
    let base = var("XDG_CONFIG_HOME")
        .filter(|p| p.is_absolute())
        .or_else(|| Some(home_dir()?.join(".config")))?;
    Some(base.join("peruse"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_of_the_settings_ends_with_the_name_of_the_program() {
        // A system with no variable at all gives nothing, and that is not a
        // fault: the caller then keeps the settings that Peruse builds in.
        if let Some(p) = config_dir() {
            assert!(p.is_absolute(), "{p:?} is not a complete path");
            let text = p.to_string_lossy().to_lowercase();
            assert!(text.contains("peruse"), "{p:?}");
            if cfg!(windows) {
                // The form that Peruse used before, so a user keeps their
                // settings.
                assert!(text.ends_with("peruse\\config"), "{p:?}");
            }
        }
    }

    #[test]
    fn the_directory_of_the_user_is_a_complete_path() {
        if let Some(p) = home_dir() {
            assert!(p.is_absolute(), "{p:?} is not a complete path");
        }
    }

    #[test]
    fn a_variable_with_no_value_is_the_same_as_no_variable() {
        // SAFETY: the test sets and removes one variable with a name that no
        // other part of the program reads.
        unsafe {
            std::env::set_var("PERUSE_TEST_EMPTY", "");
        }
        assert_eq!(var("PERUSE_TEST_EMPTY"), None);
        unsafe {
            std::env::remove_var("PERUSE_TEST_EMPTY");
        }
        assert_eq!(var("PERUSE_TEST_EMPTY"), None);
    }
}
