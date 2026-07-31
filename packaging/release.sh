#!/usr/bin/env bash
#
# Makes a whole release of Peruse, in the right order, from one command.
#
# This is release.ps1, for a POSIX shell. The two do the same steps, in the
# same order, with the same flags. Use the one that your shell likes.
#
# Usage:
#   ./packaging/release.sh 0.2.0                 look, and change nothing
#   ./packaging/release.sh 0.2.0 --execute       do it
#   ./packaging/release.sh --help                all of it, with an example
#
# The script needs git, cargo and curl. gh makes step 4 better but is not
# necessary. It runs on Git Bash on Windows, and on Linux and macOS.
#
# --------------------------------------------------------------------------
# The steps of a release, in order. release.ps1 holds the same list. Keep the
# two in step: a reader must be able to put them side by side.
#
#   1 preflight  The tree is clean, the branch is main, main is level with
#                origin/main, the tag is free, CHANGELOG.md holds a section
#                for the version, the version is higher than the one on
#                crates.io, the tests pass, clippy is clean, and both
#                crates package.
#   2 bump       Write the version into Cargo.toml, into the peruse-core line
#                of crates/peruse-tui/Cargo.toml, and into Cargo.lock.
#   3 tag        Commit the bump, tag vX.Y.Z, push main, then push the tag.
#   4 workflow   Wait for release.yml. It builds four platforms and compiles
#                DuckDB for each, so 30 to 60 minutes is normal.
#   5 verify     The release holds four archives and four .sha256 files.
#   6 render     Run render.sh, which fills the package manifests from the
#                checksums on the release.
#   7 publish    Publish peruse-core, wait for crates.io to index it, then
#                publish peruse-tui. The order is not free, because
#                peruse-tui names peruse-core by version.
#   8 handoff    Say which rendered file goes to which package repository.
# --------------------------------------------------------------------------
#
# The script uses no array of names and no `${x^^}`, so it runs on the bash
# 3.2 that macOS ships as well as on a new one.
set -euo pipefail

STEPS="preflight bump tag workflow verify render publish handoff"

REPO_NAME="JohnGrey0/peruse"
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$HERE/.." && pwd)

# The four targets that release.yml builds, and the form of archive that each
# one gives. The names here must agree with release.yml, or step 5 looks for
# files that no release holds.
TARGETS="x86_64-unknown-linux-gnu:tar.gz x86_64-apple-darwin:tar.gz aarch64-apple-darwin:tar.gz x86_64-pc-windows-msvc:zip"

# How long each wait may take before the script gives up.
WORKFLOW_TIMEOUT_MINUTES=120
INDEX_TIMEOUT_MINUTES=20

ROOT_MANIFEST="$ROOT/Cargo.toml"
CORE_MANIFEST="$ROOT/crates/peruse-core/Cargo.toml"
TUI_MANIFEST="$ROOT/crates/peruse-tui/Cargo.toml"
LOCK_FILE="$ROOT/Cargo.lock"

EXECUTE=0
SKIP_TESTS=0
NO_PUBLISH=0
YES=0
FROM=""
VER=""
TAG=""

PROBLEMS=0
PROBLEM_LIST=""
CONFIRMED=0
RESULT=""
CURRENT=""
CURRENT_INDEX=0

# The outcome of each step, one variable for each, because bash 3.2 has no
# array with names.
OUT_1="not reached"; OUT_2="not reached"; OUT_3="not reached"; OUT_4="not reached"
OUT_5="not reached"; OUT_6="not reached"; OUT_7="not reached"; OUT_8="not reached"
PLAN_1=""; PLAN_2=""; PLAN_3=""; PLAN_4=""; PLAN_5=""; PLAN_6=""; PLAN_7=""; PLAN_8=""

usage() {
    cat <<'TEXT'
release.sh -- makes a whole release of Peruse, in the right order, from one
command. This is release.ps1, for a POSIX shell. The two do the same steps,
in the same order, with the same flags.

usage: ./packaging/release.sh <version> [--execute] [--skip-tests]
                              [--from <step>] [--no-publish] [--yes] [--help]

  <version>       The version to release, as X.Y.Z, with or without the
                  leading `v`. A name such as 0.2.0-rc1 is not supported,
                  because the package templates and the manifests carry a
                  plain version only.

  --execute       Do the work. Without this flag the script is a dry run: it
                  reads the state of the repository, of the release and of
                  crates.io, prints each step that a real run would take, and
                  changes not one file.

  --skip-tests    Leave out the long checks of the preflight: the tests,
                  clippy and the two package dry runs. Use this on
                  a second run, when the same commit passed them a few minutes
                  ago. The quick checks still run.

  --from <step>   Start at a step, and leave out the ones before it. The names
                  are preflight, bump, tag, workflow, verify, render, publish
                  and handoff.

  --yes           Stand for the words that a person types to confirm. It needs
                  --execute as well. Give this only when you have taken the
                  decision already: a version on crates.io stays there for ever.
  --no-publish    Stop before crates.io. Every other step runs, so the tag,
                  the release and the package manifests are all in place. Add
                  `--from publish` later to finish.

  --help          Print this.

The steps, in order:

  1 preflight  The tree is clean, the branch is main, main is level with
               origin/main, the tag is free, CHANGELOG.md holds a section for
               the version, the version is higher than the one on crates.io,
               the tests pass, clippy is clean, and both crates package.
  2 bump       Write the version into Cargo.toml, into the peruse-core line of
               crates/peruse-tui/Cargo.toml, and into Cargo.lock.
  3 tag        Commit the bump, tag vX.Y.Z, push main, then push the tag.
  4 workflow   Wait for release.yml. It builds four platforms and compiles
               DuckDB for each, so 30 to 60 minutes is normal.
  5 verify     The release holds four archives and four .sha256 files.
  6 render     Run render.sh, which fills the package manifests.
  7 publish    Publish peruse-core, wait for crates.io to index it, then
               publish peruse-tui. The order is not free.
  8 handoff    Say which rendered file goes to which package repository.

Every step looks first to see whether it is done already, so a run after a
failure part way through says "already done" and goes on.

A worked example. A whole release of 0.2.0:

  Write the section for 0.2.0 in CHANGELOG.md first, and commit it. Then look
  at what a release would do. This changes nothing:

      ./packaging/release.sh 0.2.0

  Read the plan, then do it. The script asks you to type `v0.2.0`:

      ./packaging/release.sh 0.2.0 --execute

  Say a runner ran out of time and step 4 failed. Start the workflow again
  from the Actions page, then take the release up from there. The long checks
  passed a moment ago, so leave them out:

      ./packaging/release.sh 0.2.0 --execute --from workflow --skip-tests

  Say you want the tag and the release today and crates.io tomorrow:

      ./packaging/release.sh 0.2.0 --execute --no-publish
      ./packaging/release.sh 0.2.0 --execute --from publish --skip-tests

The crates.io token comes from CARGO_REGISTRY_TOKEN or from the credentials
that `cargo login` wrote. It is never an argument, where the history of the
shell would keep it.
TEXT
}

# --------------------------------------------------------------------------
# Words on the terminal
# --------------------------------------------------------------------------

line() { printf '%s\n' "$*"; }
note() { printf '      %s\n' "$*"; }
ok() { printf '  ok  %s\n' "$*"; }
warn() { printf '  --  %s\n' "$*"; }
fail() { printf '  NO  %s\n' "$*"; }

step_head() { printf '\n== %s %s -- %s\n' "$1" "$2" "$3"; }

# Holds a failed check. The preflight runs every check that it can before it
# stops, so that one run shows a person all the work, and not the first fault
# only.
problem() {
    PROBLEMS=$((PROBLEMS + 1))
    PROBLEM_LIST="$PROBLEM_LIST$1
"
    fail "$1"
}

# Ends the run, and says the summary first. The summary matters most when a
# step fails, because it says what is already out in the world and what a
# second run must still do.
die() {
    set_outcome "$CURRENT_INDEX" "FAILED"
    print_summary
    printf '\nStopped: %s\n' "$*"
    if [ "$EXECUTE" = 1 ] && [ -n "$CURRENT" ]; then
        printf 'Take it up again with:  ./packaging/release.sh %s --execute --from %s --skip-tests\n\n' "$VER" "$CURRENT"
    else
        printf '\n'
    fi
    exit 1
}

set_outcome() {
    case "$1" in
        1) OUT_1="$2" ;; 2) OUT_2="$2" ;; 3) OUT_3="$2" ;; 4) OUT_4="$2" ;;
        5) OUT_5="$2" ;; 6) OUT_6="$2" ;; 7) OUT_7="$2" ;; 8) OUT_8="$2" ;;
    esac
}

get_outcome() {
    case "$1" in
        1) printf '%s' "$OUT_1" ;; 2) printf '%s' "$OUT_2" ;; 3) printf '%s' "$OUT_3" ;;
        4) printf '%s' "$OUT_4" ;; 5) printf '%s' "$OUT_5" ;; 6) printf '%s' "$OUT_6" ;;
        7) printf '%s' "$OUT_7" ;; 8) printf '%s' "$OUT_8" ;;
    esac
}

set_plan() {
    case "$1" in
        1) PLAN_1="$2" ;; 2) PLAN_2="$2" ;; 3) PLAN_3="$2" ;; 4) PLAN_4="$2" ;;
        5) PLAN_5="$2" ;; 6) PLAN_6="$2" ;; 7) PLAN_7="$2" ;; 8) PLAN_8="$2" ;;
    esac
}

get_plan() {
    case "$1" in
        1) printf '%s' "$PLAN_1" ;; 2) printf '%s' "$PLAN_2" ;; 3) printf '%s' "$PLAN_3" ;;
        4) printf '%s' "$PLAN_4" ;; 5) printf '%s' "$PLAN_5" ;; 6) printf '%s' "$PLAN_6" ;;
        7) printf '%s' "$PLAN_7" ;; 8) printf '%s' "$PLAN_8" ;;
    esac
}

step_index() {
    n=0
    for s in $STEPS; do
        n=$((n + 1))
        if [ "$s" = "$1" ]; then printf '%s' "$n"; return 0; fi
    done
    return 1
}

# --------------------------------------------------------------------------
# Other programs
# --------------------------------------------------------------------------

# Runs a program, keeps each line that it wrote in READ_TEXT, and the exit
# code in READ_CODE. The failure of the program must not end the script here,
# because the caller decides what it means.
READ_TEXT=""
READ_CODE=0
run_read() {
    set +e
    READ_TEXT=$("$@" 2>&1)
    READ_CODE=$?
    set -e
}

# Runs a program and lets it write to the terminal. A long command, such as
# the tests or a publish, must show its progress. Gives the exit code.
run_loud() {
    note "run: $*"
    set +e
    "$@"
    LOUD_CODE=$?
    set -e
    return $LOUD_CODE
}

have() { command -v "$1" > /dev/null 2>&1; }

# The commit that a tag names, or an empty text.
#
# Step 3 makes the tag with `git tag -a`, and such a tag is an object of its
# own. `refs/tags/X` therefore gives the tag and never the commit, so a
# comparison with HEAD must go through `^{commit}` first.
tag_commit() { first_line git rev-parse -q --verify "refs/tags/$1^{commit}"; }

# The first line of the output of a program that reads state, with nothing
# around it. Gives an empty text when the program failed.
first_line() {
    run_read "$@"
    if [ "$READ_CODE" != 0 ]; then printf ''; return 0; fi
    printf '%s' "$READ_TEXT" | head -n 1 | tr -d '\r' |
        sed -e 's/^[[:blank:]]*//' -e 's/[[:blank:]]*$//'
}

# --------------------------------------------------------------------------
# Versions
# --------------------------------------------------------------------------

# Compares two versions of the form X.Y.Z. Says 1, 0 or -1.
compare_version() {
    IFS=. read -r a1 a2 a3 <<EOF
$1
EOF
    IFS=. read -r b1 b2 b3 <<EOF
$2
EOF
    for pair in "$a1:$b1" "$a2:$b2" "$a3:$b3"; do
        x=${pair%%:*}
        y=${pair#*:}
        if [ "$x" -gt "$y" ]; then printf '1'; return 0; fi
        if [ "$x" -lt "$y" ]; then printf -- '-1'; return 0; fi
    done
    printf '0'
}

# The place of a crate in the sparse index of crates.io. The rule comes from
# the length of the name.
index_path() {
    # The index holds each name in small letters. PowerShell does the same, so
    # the two scripts must ask for the same path.
    n=$(printf '%s' "$1" | tr 'A-Z' 'a-z')
    len=${#n}
    if [ "$len" -le 2 ]; then printf '%s/%s' "$len" "$n"; return 0; fi
    if [ "$len" -eq 3 ]; then printf '3/%s/%s' "$(printf '%s' "$n" | cut -c1)" "$n"; return 0; fi
    printf '%s/%s/%s' "$(printf '%s' "$n" | cut -c1-2)" "$(printf '%s' "$n" | cut -c3-4)" "$n"
}

# Every version of a crate that crates.io has indexed, one to a line.
#
# The index gives one line of JSON for each version, and this needs one field
# of it, so a pattern is enough and no JSON reader is needed. A crate that was
# never published gives 404, which is an empty list and not a fault. Anything
# else is a fault, and gives 1.
#
# A store sits in front of the index. Step 7 waits for a version to appear
# there, and must not read an old answer, so each call asks for a fresh one.
published_versions() {
    crate="$1"
    fresh="${2:-0}"
    url="https://index.crates.io/$(index_path "$crate")?fresh=$fresh"
    body=$(curl -sS -L --max-time 60 -H 'Cache-Control: no-cache' -H 'Pragma: no-cache' \
        -w '\n%{http_code}' "$url" 2> /dev/null) || {
        printf 'could not reach %s\n' "$url" >&2
        return 1
    }
    code=$(printf '%s' "$body" | tail -n 1)
    if [ "$code" = "404" ]; then return 0; fi
    if [ "$code" != "200" ]; then
        printf 'the crates.io index answered %s for %s\n' "$code" "$url" >&2
        return 1
    fi
    printf '%s' "$body" | sed -n 's/.*"vers"[ ]*:[ ]*"\([^"]*\)".*/\1/p'
}

# The highest version of a crate on crates.io, or an empty text.
#
# A name with a letter in it, such as a release candidate, takes no part in the
# comparison, because this script releases plain versions only.
highest_published() {
    best=""
    if ! all=$(published_versions "$1"); then return 1; fi
    for v in $all; do
        if ! printf '%s' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then continue; fi
        if [ -z "$best" ] || [ "$(compare_version "$v" "$best")" = "1" ]; then best="$v"; fi
    done
    printf '%s' "$best"
}

# --------------------------------------------------------------------------
# The version in the manifests and in the lock file
# --------------------------------------------------------------------------

# How many `version = "..."` lines the [workspace.package] section holds.
count_workspace_version_lines() {
    awk '
        /^[[:blank:]]*\[/ { insec = ($0 ~ /^[[:blank:]]*\[workspace\.package\]/); next }
        insec && /^[[:blank:]]*version[[:blank:]]*=[[:blank:]]*"[^"]+"/ { n++ }
        END { print n + 0 }
    ' "$1"
}

# How many lines name peruse-core with a version written out in full.
#
# crates/peruse-tui/Cargo.toml holds one such line. It does not say
# `version.workspace = true`, so a bump that misses it leaves peruse-tui
# asking for an old peruse-core, and the publish then fails or publishes
# something wrong. This is the reason the script looks for the line, counts
# it, and stops when the count is not one.
count_dep_version_lines() {
    awk '
        /^[[:blank:]]*peruse-core[[:blank:]]*=/ && /version[[:blank:]]*=[[:blank:]]*"[^"]+"/ { n++ }
        END { print n + 0 }
    ' "$1"
}

workspace_version() {
    awk '
        /^[[:blank:]]*\[/ { insec = ($0 ~ /^[[:blank:]]*\[workspace\.package\]/); next }
        insec && match($0, /^[[:blank:]]*version[[:blank:]]*=[[:blank:]]*"[^"]*"/) {
            s = substr($0, RSTART, RLENGTH)
            match(s, /"[^"]*"/)
            print substr(s, RSTART + 1, RLENGTH - 2)
            exit
        }
    ' "$1"
}

dep_version() {
    awk '
        /^[[:blank:]]*peruse-core[[:blank:]]*=/ && match($0, /version[[:blank:]]*=[[:blank:]]*"[^"]*"/) {
            s = substr($0, RSTART, RLENGTH)
            match(s, /"[^"]*"/)
            print substr(s, RSTART + 1, RLENGTH - 2)
            exit
        }
    ' "$1"
}

# The version of a package in Cargo.lock.
lock_version() {
    awk -v want="$2" '
        index($0, "name = \"" want "\"") == 1 {
            getline
            if (match($0, /"[^"]*"/)) { print substr($0, RSTART + 1, RLENGTH - 2) }
            exit
        }
    ' "$1"
}

# Reads the version out of every place that carries it. The bump and the
# preflight both work from this one answer.
STATE_WS=""; STATE_WS_HITS=0; STATE_DEP=""; STATE_DEP_HITS=0; STATE_STRAY=0
STATE_LOCK_CORE=""; STATE_LOCK_TUI=""
read_version_state() {
    STATE_WS_HITS=$(count_workspace_version_lines "$ROOT_MANIFEST")
    STATE_DEP_HITS=$(count_dep_version_lines "$TUI_MANIFEST")
    # A line of this form belongs in the front end only. The count covers the
    # other two manifests as well, so that a line which moves is not missed.
    STATE_STRAY=$(( $(count_dep_version_lines "$ROOT_MANIFEST") + $(count_dep_version_lines "$CORE_MANIFEST") ))
    STATE_WS=""
    if [ "$STATE_WS_HITS" = "1" ]; then STATE_WS=$(workspace_version "$ROOT_MANIFEST"); fi
    STATE_DEP=""
    if [ "$STATE_DEP_HITS" = "1" ]; then STATE_DEP=$(dep_version "$TUI_MANIFEST"); fi
    STATE_LOCK_CORE=$(lock_version "$LOCK_FILE" peruse-core)
    STATE_LOCK_TUI=$(lock_version "$LOCK_FILE" peruse-tui)
}

# Puts a new version in the quoted value of one line, and leaves the rest of
# the line, the spaces and the line ending as they are.
set_line_version() {
    file="$1"
    new="$2"
    which_line="$3"
    awk -v new="$new" -v which="$which_line" '
        BEGIN { done = 0 }
        {
            hit = 0
            if (which == "workspace") {
                if ($0 ~ /^[[:blank:]]*\[/) { insec = ($0 ~ /^[[:blank:]]*\[workspace\.package\]/); print; next }
                if (insec && !done && $0 ~ /^[[:blank:]]*version[[:blank:]]*=[[:blank:]]*"[^"]+"/) { hit = 1 }
            } else {
                if (!done && $0 ~ /^[[:blank:]]*peruse-core[[:blank:]]*=/ && $0 ~ /version[[:blank:]]*=[[:blank:]]*"[^"]+"/) { hit = 1 }
            }
            if (hit && match($0, /version[[:blank:]]*=[[:blank:]]*"[^"]*"/)) {
                seg = substr($0, RSTART, RLENGTH)
                head = substr($0, 1, RSTART - 1)
                tail = substr($0, RSTART + RLENGTH)
                sub(/"[^"]*"/, "\"" new "\"", seg)
                print head seg tail
                done = 1
                next
            }
            print
        }
    ' "$file"
}

# Writes one version into the root manifest and into the peruse-core line of
# the front end. Nothing else in either file changes.
write_version_state() {
    new="$1"

    # Both files are read and both are checked before either one is written. A
    # write that stopped half way would leave the workspace at the new version
    # and the front end still asking for the old peruse-core, which is the
    # very fault that this step is here to stop.
    ws_hits=$(count_workspace_version_lines "$ROOT_MANIFEST")
    if [ "$ws_hits" != "1" ]; then
        die "Cargo.toml holds $ws_hits version lines in [workspace.package]. It must hold one. Fix the file, then run again."
    fi
    dep_hits=$(count_dep_version_lines "$TUI_MANIFEST")
    if [ "$dep_hits" != "1" ]; then
        die "crates/peruse-tui/Cargo.toml holds $dep_hits lines that name peruse-core with a version written out in full. It must hold one. Fix the file, then run again."
    fi
    stray=$(( $(count_dep_version_lines "$ROOT_MANIFEST") + $(count_dep_version_lines "$CORE_MANIFEST") ))
    if [ "$stray" != "0" ]; then
        die "Another manifest names peruse-core with a version written out in full. Only crates/peruse-tui/Cargo.toml may. Look at Cargo.toml and crates/peruse-core/Cargo.toml."
    fi

    tmp_root="$ROOT_MANIFEST.release-tmp"
    tmp_tui="$TUI_MANIFEST.release-tmp"
    set_line_version "$ROOT_MANIFEST" "$new" workspace > "$tmp_root"
    set_line_version "$TUI_MANIFEST" "$new" dep > "$tmp_tui"
    mv "$tmp_root" "$ROOT_MANIFEST"
    mv "$tmp_tui" "$TUI_MANIFEST"

    note "Cargo.toml [workspace.package] version = \"$new\""
    note "crates/peruse-tui/Cargo.toml $(grep -m1 '^[[:blank:]]*peruse-core[[:blank:]]*=' "$TUI_MANIFEST")"
}

# --------------------------------------------------------------------------
# The release on GitHub
# --------------------------------------------------------------------------

release_file_names() {
    for pair in $TARGETS; do
        target=${pair%%:*}
        archive=${pair#*:}
        printf 'peruse-%s-%s.%s\n' "$1" "$target" "$archive"
        printf 'peruse-%s-%s.%s.sha256\n' "$1" "$target" "$archive"
    done
}

# Looks for one file on the release. A release that is not there, and a file
# that is not on it, both give 404, and both mean the same thing here: the
# workflow has not put the file up yet.
release_file_there() {
    url="https://github.com/$REPO_NAME/releases/download/v$1/$2"
    code=$(curl -sS -L -I -o /dev/null --max-time 60 -w '%{http_code}' "$url" 2> /dev/null) || code="000"
    case "$code" in
        200) return 0 ;;
        404) return 1 ;;
        *) printf 'could not ask about %s: the answer was %s\n' "$url" "$code" >&2; return 2 ;;
    esac
}

# The names of the files that the release does not hold yet.
missing_release_files() {
    for name in $(release_file_names "$1"); do
        set +e
        release_file_there "$1" "$name"
        rc=$?
        set -e
        if [ "$rc" = "2" ]; then return 1; fi
        if [ "$rc" = "1" ]; then printf '%s\n' "$name"; fi
    done
}

count_lines() {
    if [ -z "$1" ]; then printf '0'; return 0; fi
    printf '%s\n' "$1" | grep -c .
}

# The number of files that a whole release holds. The count comes from the list
# of targets, so another platform in that list needs no change anywhere else.
release_expected_count() {
    count_lines "$(release_file_names "$1")"
}

# The names of the archives on the release, with no .sha256 file among them.
release_archive_names() {
    release_file_names "$1" | grep -v '\.sha256$'
}

# Reads one .sha256 file from the release and checks what it holds.
#
# A file of the right name can still hold the wrong thing: an upload that
# stopped, a page of error text, or nothing at all. The file holds the output of
# `sha256sum`, which is the checksum, then a space or two, then the name of the
# archive. This function checks the two parts. It writes the reason when one of
# them is wrong, and it writes nothing when the file is good.
#
# A .sha256 file is about one hundred bytes, so this check costs nothing. A check
# of the archive itself would read about one hundred megabytes for each platform,
# and it would find almost nothing that this check does not.
release_checksum_why() {
    ver="$1"
    archive="$2"
    url="https://github.com/$REPO_NAME/releases/download/v$ver/$archive.sha256"

    if ! body=$(curl -sS -L --max-time 60 "$url" 2> /dev/null); then
        printf 'could not read %s.sha256\n' "$archive"
        return 0
    fi

    first=$(printf '%s\n' "$body" | head -n 1)
    sum=$(printf '%s' "$first" | awk '{print $1}')
    named=$(printf '%s' "$first" | awk '{print $NF}')

    if [ ${#sum} -ne 64 ]; then
        printf '%s.sha256 does not hold 64 characters of hexadecimal\n' "$archive"
        return 0
    fi
    case "$sum" in
        *[!0-9a-fA-F]*)
            printf '%s.sha256 does not hold 64 characters of hexadecimal\n' "$archive"
            return 0
            ;;
    esac

    # `sha256sum` writes a star in front of the name of a file that it read as
    # bytes, and the name may carry a directory. A file with the checksum alone
    # names nothing, and that is not a fault.
    if [ "$named" != "$sum" ]; then
        named=${named#\*}
        named=${named##*/}
        if [ "$named" != "$archive" ]; then
            printf '%s.sha256 names %s\n' "$archive" "$named"
            return 0
        fi
    fi
    return 0
}

# --------------------------------------------------------------------------
# The words that a person must type
# --------------------------------------------------------------------------

# Asks once, before the first step that cannot be taken back.
#
# The word to type is the tag itself. A person who is part way through
# something else cannot answer it from memory of another release.
confirm_release() {
    if [ "$CONFIRMED" = 1 ]; then return 0; fi

    printf '\nThese things cannot be taken back:\n'
    for a in "$@"; do printf '  * %s\n' "$a"; done
    printf '\nA version on crates.io stays there for ever. It cannot be replaced.\n\n'

    # The option --yes takes the place of the words. It is for a person who
    # cannot type into this run, such as a tool that drives the release. It needs
    # --execute as well, so a run with no option still changes nothing.
    #
    # The option is not a way to release with nobody there. The person who gives
    # --yes takes the same decision as the person who types the words, and takes
    # it before the run starts.
    if [ "$YES" = 1 ]; then
        printf 'The option --yes stands for the words. Going on.
'
        CONFIRMED=1
        printf '
'
        return 0
    fi

    if [ ! -t 0 ]; then
        die "This needs a terminal, because a person must confirm the release. Run it in a terminal, or give --yes."
    fi
    printf 'Type %s to go on, or anything else to stop: ' "$TAG"
    typed=""
    if ! IFS= read -r typed; then typed=""; fi
    typed=$(printf '%s' "$typed" | tr -d ' \t\r')
    if [ "$typed" != "$TAG" ]; then
        die "You typed something else, so nothing was done."
    fi
    CONFIRMED=1
    printf '\n'
}

# --------------------------------------------------------------------------
# 1 preflight
# --------------------------------------------------------------------------

step_preflight() {
    step_head 1 preflight 'the state of the repository, of the tag and of crates.io'

    if ! have git; then problem 'git is not on the path.'; fi
    if ! have cargo; then problem 'cargo is not on the path.'; fi
    if ! have curl; then problem 'curl is not on the path.'; fi
    if [ "$PROBLEMS" != 0 ]; then RESULT="failed"; return 0; fi

    # The tree must hold nothing of its own, because step 3 commits the three
    # files that the bump touches, and a commit that carries other work makes
    # a release that nobody can read later.
    run_read git status --porcelain
    if [ -z "$READ_TEXT" ]; then
        ok 'The working tree is clean.'
    else
        problem 'The working tree holds changes. Commit them or put them away first.'
        printf '%s\n' "$READ_TEXT" | while IFS= read -r l; do note "$l"; done
    fi

    branch=$(first_line git rev-parse --abbrev-ref HEAD)
    if [ "$branch" = "main" ]; then
        ok 'The branch is main.'
    else
        problem "The branch is '$branch' and a release comes from main."
    fi

    # `ls-remote` asks the remote and writes nothing here, so a dry run leaves
    # even the git directory as it was.
    head_sha=$(first_line git rev-parse HEAD)
    remote_main=""
    run_read git ls-remote origin refs/heads/main
    if [ "$READ_CODE" = 0 ]; then
        remote_main=$(printf '%s' "$READ_TEXT" | awk '/refs\/heads\/main/ { print $1; exit }')
    fi
    if [ -z "$remote_main" ]; then
        problem 'Could not read refs/heads/main from origin. Is there a network?'
    elif [ "$remote_main" = "$head_sha" ]; then
        ok "main is level with origin/main, at $(printf '%s' "$head_sha" | cut -c1-7)."
    else
        problem "main is not level with origin/main. Here is $(printf '%s' "$head_sha" | cut -c1-7) and there is $(printf '%s' "$remote_main" | cut -c1-7). Pull or push first."
    fi

    # The tag must be free. A tag that is there already means that this version
    # went out, or that a release stopped part way. Neither is a thing to walk
    # over: use --from to take up a release that stopped.
    local_tag=$(first_line git rev-parse -q --verify "refs/tags/$TAG")
    if [ -z "$local_tag" ]; then
        ok "There is no tag $TAG here."
    else
        at=$(tag_commit "$TAG")
        if [ -z "$at" ]; then at="$local_tag"; fi
        problem "The tag $TAG is here already, at $(printf '%s' "$at" | cut -c1-7). Give --from to take up a release that stopped part way."
    fi

    run_read git ls-remote --tags origin "refs/tags/$TAG"
    if [ "$READ_CODE" != 0 ]; then
        problem 'Could not read the tags of origin. Is there a network?'
    elif [ -z "$READ_TEXT" ]; then
        ok "There is no tag $TAG on origin."
    else
        problem "The tag $TAG is on origin already. release.yml has run for it."
    fi

    # release.yml builds with --locked, so the lock file must hold the version
    # that goes out. Step 2 sees to that.
    read_version_state
    if [ "$STATE_WS_HITS" != "1" ]; then
        problem "Cargo.toml holds $STATE_WS_HITS version lines in [workspace.package]. It must hold one."
    fi
    if [ "$STATE_DEP_HITS" != "1" ]; then
        problem "crates/peruse-tui/Cargo.toml holds $STATE_DEP_HITS lines that name peruse-core with a version written out in full. It must hold one."
    fi
    if [ "$STATE_STRAY" != "0" ]; then
        problem "Another manifest names peruse-core with a version written out in full. Only crates/peruse-tui/Cargo.toml may. Look at Cargo.toml and crates/peruse-core/Cargo.toml."
    fi
    if [ "$STATE_WS_HITS" = "1" ] && [ "$STATE_DEP_HITS" = "1" ]; then
        ok "The manifests say $STATE_WS, and peruse-tui asks for peruse-core $STATE_DEP."
    fi

    if [ -n "$STATE_WS" ]; then
        c=$(compare_version "$VER" "$STATE_WS")
        if [ "$c" = "-1" ]; then
            problem "The manifests say $STATE_WS, which is higher than $VER. A release does not go backwards."
        elif [ "$c" = "0" ]; then
            warn "The manifests say $VER already, so step 2 has nothing to do."
        fi
    fi

    if grep -Eq "^##[[:blank:]]+\[?v?$(printf '%s' "$VER" | sed 's/\./\\./g')\]?([[:blank:]]|$)" "$ROOT/CHANGELOG.md"; then
        ok "CHANGELOG.md holds a section for $VER."
    else
        problem "CHANGELOG.md holds no section for $VER. Add '## $VER' with the changes, and commit it."
    fi

    # A release that is lower than the one on crates.io cannot be published,
    # and finding that out after the tag is on the remote is too late.
    # A preflight that cannot read the index must say so and go on, so that one
    # run shows every other fault as well.
    index_ok=1
    if ! core_high=$(highest_published peruse-core); then index_ok=0; core_high=""; fi
    if ! tui_high=$(highest_published peruse-tui); then index_ok=0; tui_high=""; fi
    if [ "$index_ok" = 0 ]; then
        problem 'Could not read the crates.io index. Is there a network?'
    fi
    if [ -z "$core_high" ]; then warn 'peruse-core is not on crates.io yet.'; fi
    if [ -z "$tui_high" ]; then warn 'peruse-tui is not on crates.io yet.'; fi
    if [ -n "$core_high" ] && [ -n "$tui_high" ] && [ "$core_high" != "$tui_high" ]; then
        warn "crates.io holds peruse-core $core_high and peruse-tui $tui_high. A release stopped between the two."
    fi
    for pair in "peruse-core:$core_high" "peruse-tui:$tui_high"; do
        name=${pair%%:*}
        high=${pair#*:}
        if [ -z "$high" ]; then continue; fi
        if [ "$(compare_version "$VER" "$high")" = "1" ]; then
            ok "$VER is higher than $name $high on crates.io."
        else
            problem "crates.io holds $name $high already, and $VER is not higher. A version on crates.io can never be replaced."
        fi
    done

    # gh gives the state of the workflow run. Step 4 can wait without it, by
    # watching the files appear on the release, but the message when a build
    # fails is much poorer.
    if have gh; then
        ok "gh is here: $(first_line gh --version)"
    else
        warn 'gh is not on the path. Step 4 will watch the release files instead, and cannot show you a log.'
    fi

    if [ "$(get_plan 7)" = "run" ]; then
        token=0
        if [ -n "${CARGO_REGISTRY_TOKEN:-}" ]; then
            ok 'A crates.io token is in CARGO_REGISTRY_TOKEN.'
            token=1
        else
            cargo_home="${CARGO_HOME:-$HOME/.cargo}"
            for f in credentials.toml credentials; do
                if [ -f "$cargo_home/$f" ]; then
                    # The path only. The token itself never goes on the
                    # terminal, where it would stay in the buffer.
                    ok "cargo holds credentials at $cargo_home/$f."
                    token=1
                    break
                fi
            done
        fi
        if [ "$token" = 0 ]; then
            problem 'No crates.io token. Run `cargo login`, or set CARGO_REGISTRY_TOKEN, before step 7.'
        fi
    fi

    if [ -f "$HERE/render.sh" ]; then
        ok 'render.sh is here, for step 6.'
    else
        problem "There is no $HERE/render.sh, and step 6 needs it."
    fi

    if [ "$PROBLEMS" != 0 ]; then
        printf '\n'
        warn 'The quick checks found something, so the long ones did not run.'
        RESULT="failed"
        return 0
    fi

    if [ "$SKIP_TESTS" = 1 ]; then
        warn '--skip-tests: the tests, clippy and the two package dry runs did not run.'
        RESULT="done"
        return 0
    fi

    # The long checks. Each one is the same command that CI runs, so a fault
    # here is a fault that the release workflow would meet as well.
    printf '\n'
    note 'The long checks now. The tests and clippy take some minutes.'

    # There is no check of the format here, and that is on purpose. This project
    # writes its code by hand and holds no rustfmt settings, and `ci.yml` runs the
    # tests and clippy and no format check. A format check would refuse every
    # release over 259 places that no change of today touched, and a gate that the
    # project does not hold itself to is a wall and not a gate.
    printf '\n'
    if run_loud cargo test --workspace; then ok 'the tests are clean.'; else problem 'the tests failed. Read the output above.'; fi
    printf '\n'
    if run_loud cargo clippy --workspace --all-targets -- -D warnings; then ok 'clippy is clean.'; else problem 'clippy failed. Read the output above.'; fi
    printf '\n'
    if run_loud cargo publish --dry-run --locked -p peruse-core; then
        ok 'the package of peruse-core is clean.'
    else
        problem 'the package of peruse-core failed. Read the output above.'
    fi
    printf '\n'
    if run_loud cargo publish --dry-run --locked -p peruse-tui; then
        ok 'the package of peruse-tui is clean.'
    else
        # The front end names the engine by version. Before the engine of that
        # version is on crates.io, the dry run of the front end cannot resolve
        # it, and CI leaves the front end out for the same reason. That is not
        # a fault of this release.
        core_all=""
        if ! core_all=$(published_versions peruse-core); then core_all=""; fi
        if printf '%s\n' "$core_all" | grep -qx "$STATE_DEP"; then
            problem 'the package of peruse-tui failed. Read the output above.'
        else
            warn "The dry run of peruse-tui could not resolve peruse-core $STATE_DEP, which is not on crates.io. Step 7 publishes the engine first, and that is when this can be checked."
        fi
    fi

    if [ "$PROBLEMS" != 0 ]; then RESULT="failed"; else RESULT="done"; fi
}

# --------------------------------------------------------------------------
# 2 bump
# --------------------------------------------------------------------------

step_bump() {
    step_head 2 bump "write $VER into the manifests and the lock file"

    read_version_state
    if [ "$STATE_WS" = "$VER" ] && [ "$STATE_DEP" = "$VER" ] &&
        [ "$STATE_LOCK_CORE" = "$VER" ] && [ "$STATE_LOCK_TUI" = "$VER" ]; then
        ok "Already done, going on. Everything says $VER."
        RESULT="already done"
        return 0
    fi

    note "Cargo.toml [workspace.package] version: $STATE_WS -> $VER"
    note "crates/peruse-tui/Cargo.toml peruse-core version: $STATE_DEP -> $VER"
    note "Cargo.lock peruse-core $STATE_LOCK_CORE and peruse-tui $STATE_LOCK_TUI -> $VER"

    if [ "$EXECUTE" != 1 ]; then RESULT="would do"; return 0; fi

    # The first step that writes anything, so the question comes here. Step 3
    # asks nothing more, because a person answers once for the whole release.
    confirm_release \
        "commit the version bump and push it to origin/main" \
        "push the tag $TAG, which starts release.yml and makes a public release"

    write_version_state "$VER"

    # release.yml builds with --locked. A lock file that still holds the old
    # version therefore stops the release, so the lock must move with the
    # manifests.
    if ! run_loud cargo update -p peruse-core -p peruse-tui; then
        warn 'That did not work. Trying the whole workspace.'
        if ! run_loud cargo update --workspace; then
            die "Could not write Cargo.lock. Run \`cargo check\` by hand, then run this again with --from tag."
        fi
    fi

    read_version_state
    if [ "$STATE_WS" != "$VER" ]; then die "Cargo.toml still says $STATE_WS."; fi
    if [ "$STATE_DEP" != "$VER" ]; then die "crates/peruse-tui/Cargo.toml still asks for peruse-core $STATE_DEP."; fi
    if [ "$STATE_LOCK_CORE" != "$VER" ]; then die "Cargo.lock still holds peruse-core $STATE_LOCK_CORE."; fi
    if [ "$STATE_LOCK_TUI" != "$VER" ]; then die "Cargo.lock still holds peruse-tui $STATE_LOCK_TUI."; fi
    ok "The manifests and the lock file all say $VER."
    RESULT="done"
}

# --------------------------------------------------------------------------
# 3 tag
# --------------------------------------------------------------------------

step_tag() {
    step_head 3 tag "commit the bump, tag $TAG, push main, then push the tag"

    # The three files of the bump only. Another file in the tree is not a thing
    # for this step to commit, and a run that takes a release up from here with
    # --from leaves the preflight out, so the tree can hold work of its own. A
    # commit with nothing in it fails, so the question this step must ask is
    # whether these three files differ from HEAD.
    run_read git status --porcelain -- Cargo.toml Cargo.lock crates/peruse-tui/Cargo.toml
    dirty="$READ_TEXT"
    head_sha=$(first_line git rev-parse HEAD)
    local_tag=$(first_line git rev-parse -q --verify "refs/tags/$TAG")
    local_commit=$(tag_commit "$TAG")
    if [ -z "$local_commit" ]; then local_commit="$local_tag"; fi

    run_read git ls-remote --tags origin "refs/tags/$TAG"
    if [ "$READ_CODE" != 0 ]; then die 'Could not read the tags of origin. Is there a network?'; fi
    remote_tag=$(printf '%s' "$READ_TEXT" | awk '{ print $1; exit }')

    run_read git ls-remote origin refs/heads/main
    if [ "$READ_CODE" != 0 ]; then die 'Could not read refs/heads/main from origin. Is there a network?'; fi
    remote_main=$(printf '%s' "$READ_TEXT" | awk '/refs\/heads\/main/ { print $1; exit }')

    # A tag that is on the remote at another commit is the worst thing that can
    # happen here, because the release and the crates would come from
    # different code. The script never moves a tag.
    if [ -n "$remote_tag" ] && [ -n "$local_tag" ] && [ "$remote_tag" != "$local_tag" ]; then
        die "The tag $TAG is at $(printf '%s' "$local_tag" | cut -c1-7) here and at $(printf '%s' "$remote_tag" | cut -c1-7) on origin. Sort that out by hand."
    fi

    if [ "$EXECUTE" != 1 ]; then
        # A dry run reads a tree that step 2 has not written to, so the answer
        # to "is there anything to commit" must count the bump that is to come.
        coming=0
        if [ -n "$dirty" ]; then coming=1; fi
        read_version_state
        if [ "$STATE_WS" != "$VER" ] || [ "$STATE_DEP" != "$VER" ] ||
            [ "$STATE_LOCK_CORE" != "$VER" ] || [ "$STATE_LOCK_TUI" != "$VER" ]; then coming=1; fi

        if [ "$coming" = 0 ]; then note 'There is nothing to commit.'
        else note "git add and git commit -m \"Release $TAG\", for: Cargo.toml, Cargo.lock, crates/peruse-tui/Cargo.toml"; fi
        if [ -z "$local_tag" ]; then note "git tag -a $TAG"; else note "The tag $TAG is here already."; fi
        if [ "$remote_main" = "$head_sha" ] && [ "$coming" = 0 ]; then note 'origin/main is level already.'
        else note 'git push origin main'; fi
        if [ -z "$remote_tag" ]; then note "git push origin refs/tags/$TAG"; else note "The tag $TAG is on origin already."; fi
        RESULT="would do"
        return 0
    fi

    confirm_release \
        "commit the version bump and push it to origin/main" \
        "push the tag $TAG, which starts release.yml and makes a public release"

    if [ -z "$dirty" ]; then
        ok 'Already done, going on. There is nothing to commit.'
    else
        # Only the three files of the bump go in. Anything else in the tree
        # stays out, and the preflight has said that there is nothing else.
        if ! run_loud git add -- Cargo.toml Cargo.lock crates/peruse-tui/Cargo.toml; then
            die 'git add failed.'
        fi
        if ! run_loud git commit -m "Release $TAG"; then die 'git commit failed.'; fi
        head_sha=$(first_line git rev-parse HEAD)
        ok "Committed the bump at $(printf '%s' "$head_sha" | cut -c1-7)."
    fi

    if [ -n "$local_tag" ]; then
        if [ "$local_commit" != "$head_sha" ]; then
            die "The tag $TAG is at $(printf '%s' "$local_commit" | cut -c1-7) and HEAD is at $(printf '%s' "$head_sha" | cut -c1-7). Sort that out by hand."
        fi
        ok "Already done, going on. The tag $TAG is at HEAD."
    else
        if ! run_loud git tag -a "$TAG" -m "peruse $TAG"; then die 'git tag failed.'; fi
        ok "Made the tag $TAG."
    fi

    if [ "$remote_main" = "$head_sha" ]; then
        ok 'Already done, going on. origin/main is level.'
    else
        # The branch goes first. A tag that arrives before the commit that it
        # names leaves the release pointing at nothing.
        if ! run_loud git push origin main; then die 'git push origin main failed.'; fi
        ok 'Pushed main.'
    fi

    if [ -n "$remote_tag" ]; then
        ok "Already done, going on. The tag $TAG is on origin."
    else
        if ! run_loud git push origin "refs/tags/$TAG"; then die "git push origin refs/tags/$TAG failed."; fi
        ok "Pushed the tag $TAG. release.yml starts now."
    fi
    RESULT="done"
}

# --------------------------------------------------------------------------
# 4 workflow
# --------------------------------------------------------------------------

# The run of release.yml for this tag, or an empty text.
workflow_run_id() {
    run_read gh run list --workflow release.yml --branch "$TAG" --limit 1 --json databaseId,status,conclusion,url
    if [ "$READ_CODE" != 0 ]; then return 0; fi
    printf '%s' "$READ_TEXT" | sed -n 's/.*"databaseId"[ ]*:[ ]*\([0-9]*\).*/\1/p' | head -n 1
}

step_workflow() {
    step_head 4 workflow 'wait for release.yml to build the four platforms'

    note 'release.yml builds for four platforms and compiles DuckDB for each.'
    note "30 to 60 minutes is normal, and this waits up to $WORKFLOW_TIMEOUT_MINUTES minutes."

    expected=$(release_expected_count "$VER")

    if ! missing=$(missing_release_files "$VER"); then die 'Could not ask GitHub about the release.'; fi
    if [ -z "$missing" ]; then
        ok "Already done, going on. The release v$VER holds all $expected files."
        RESULT="already done"
        return 0
    fi

    if [ "$EXECUTE" != 1 ]; then
        note "$(count_lines "$missing") of the 8 files are not up yet, so a real run would wait here."
        note "https://github.com/$REPO_NAME/actions/workflows/release.yml"
        RESULT="would do"
        return 0
    fi

    deadline=$(( $(date +%s) + WORKFLOW_TIMEOUT_MINUTES * 60 ))
    use_gh=0
    run_id=""
    if have gh; then
        use_gh=1
        # The run takes a few seconds to appear after the push.
        i=0
        while [ "$i" -lt 20 ]; do
            run_id=$(workflow_run_id)
            if [ -n "$run_id" ]; then break; fi
            i=$((i + 1))
            sleep 6
        done
        if [ -z "$run_id" ]; then
            warn "gh found no run for $TAG. Watching the release files instead."
            use_gh=0
        else
            ok "The run is https://github.com/$REPO_NAME/actions/runs/$run_id"
        fi
    fi

    # A poll, and not `gh run watch`, because a poll can hold a timeout and can
    # say something every time, and because the way without gh must work in
    # the same shape.
    while [ "$(date +%s)" -lt "$deadline" ]; do
        if [ "$use_gh" = 1 ]; then
            run_read gh run view "$run_id" --json status,conclusion
            if [ "$READ_CODE" = 0 ]; then
                status=$(printf '%s' "$READ_TEXT" | sed -n 's/.*"status"[ ]*:[ ]*"\([^"]*\)".*/\1/p' | head -n 1)
                conclusion=$(printf '%s' "$READ_TEXT" | sed -n 's/.*"conclusion"[ ]*:[ ]*"\([^"]*\)".*/\1/p' | head -n 1)
                if [ "$status" = "completed" ]; then
                    if [ "$conclusion" = "success" ]; then
                        ok 'The workflow finished well.'
                        RESULT="done"
                        return 0
                    fi
                    fail "The workflow ended as '$conclusion'."
                    note "Read the log with:  gh run view $run_id --log-failed"
                    note "Or open https://github.com/$REPO_NAME/actions/runs/$run_id"
                    die "release.yml did not succeed. The tag is on the remote, so fix the build, run the workflow again, and take this up with --from workflow."
                fi
                warn "$(date +%H:%M:%S) the run is $status."
            else
                warn 'gh could not read the run. Trying again.'
            fi
        else
            if ! missing=$(missing_release_files "$VER"); then die 'Could not ask GitHub about the release.'; fi
            if [ -z "$missing" ]; then
                ok "All $expected files are on the release."
                RESULT="done"
                return 0
            fi
            warn "$(date +%H:%M:%S) $((expected - $(count_lines "$missing"))) of $expected files are up."
        fi
        sleep 30
    done

    die "release.yml did not finish inside $WORKFLOW_TIMEOUT_MINUTES minutes. Look at https://github.com/$REPO_NAME/actions/workflows/release.yml and take this up with --from workflow."
}

# --------------------------------------------------------------------------
# 5 verify
# --------------------------------------------------------------------------

step_verify() {
    step_head 5 verify 'the release is whole and each checksum reads as a checksum'

    if ! missing=$(missing_release_files "$VER"); then die 'Could not ask GitHub about the release.'; fi
    for name in $(release_file_names "$VER"); do
        if printf '%s\n' "$missing" | grep -qx "$name"; then fail "$name"; else ok "$name"; fi
    done

    if [ -z "$missing" ]; then
        # A file that is there can still hold the wrong thing. Read each
        # checksum and check it before the manifests carry it to four package
        # repositories, where a wrong checksum stops every install.
        bad=0
        for name in $(release_archive_names "$VER"); do
            why=$(release_checksum_why "$VER" "$name")
            if [ -n "$why" ]; then
                bad=$((bad + 1))
                fail "$why"
            else
                ok "$name.sha256 holds a checksum"
            fi
        done
        if [ "$bad" -ne 0 ]; then
            die "The release v$VER holds $bad checksum files that are not right. Run the workflow again for the platforms that failed, then take this up with --from verify."
        fi
        ok "The release v$VER is whole, and each checksum reads as a checksum."
        RESULT="done"
        return 0
    fi

    if [ "$EXECUTE" != 1 ]; then
        note "$(count_lines "$missing") files are not there. In a real run this would stop here."
        note "The release v$VER does not exist yet, which is what a dry run before a release shows."
        RESULT="would do"
        return 0
    fi
    die "The release v$VER is short of $(count_lines "$missing") files. Look at the workflow, then take this up with --from workflow."
}

# --------------------------------------------------------------------------
# 6 render
# --------------------------------------------------------------------------

step_render() {
    step_head 6 render 'fill the package manifests from the checksums on the release'

    if [ "$EXECUTE" != 1 ]; then
        note "$HERE/render.sh $VER"
        note 'It reads the .sha256 files from the release and writes packaging/out/.'
        RESULT="would do"
        return 0
    fi

    # render.sh clears packaging/out/ each time, so a second run is safe and
    # there is nothing to look for first.
    if ! run_loud sh "$HERE/render.sh" "$VER"; then die 'render.sh failed.'; fi
    ok 'The manifests are in packaging/out/.'
    RESULT="done"
}

# --------------------------------------------------------------------------
# 7 publish
# --------------------------------------------------------------------------

# Waits until crates.io has indexed one version of a crate.
#
# cargo resolves from the index, so peruse-tui cannot build until the index
# holds the new peruse-core. The wait is a poll and not a sleep, because the
# time it takes is not known: it is a few seconds most days.
wait_for_index() {
    crate="$1"
    want="$2"
    deadline=$(( $(date +%s) + INDEX_TIMEOUT_MINUTES * 60 ))
    try=0
    while [ "$(date +%s)" -lt "$deadline" ]; do
        try=$((try + 1))
        if published_versions "$crate" "$try" | grep -qx "$want"; then
            ok "crates.io has indexed $crate $want."
            return 0
        fi
        warn "$(date +%H:%M:%S) waiting for $crate $want in the index."
        sleep 10
    done
    return 1
}

step_publish() {
    step_head 7 publish 'peruse-core first, then peruse-tui'

    core_has=0
    tui_has=0
    if ! core_all=$(published_versions peruse-core); then
        die 'Could not read the crates.io index. Is there a network?'
    fi
    if ! tui_all=$(published_versions peruse-tui); then
        die 'Could not read the crates.io index. Is there a network?'
    fi
    if printf '%s\n' "$core_all" | grep -qx "$VER"; then core_has=1; fi
    if printf '%s\n' "$tui_all" | grep -qx "$VER"; then tui_has=1; fi

    if [ "$core_has" = 1 ] && [ "$tui_has" = 1 ]; then
        ok "Already done, going on. crates.io holds both crates at $VER."
        RESULT="already done"
        return 0
    fi

    if [ "$EXECUTE" != 1 ]; then
        if [ "$core_has" = 1 ]; then note "peruse-core $VER is on crates.io already."
        else note 'cargo publish -p peruse-core --locked'; fi
        note 'then wait for the index'
        if [ "$tui_has" = 1 ]; then note "peruse-tui $VER is on crates.io already."
        else note 'cargo publish -p peruse-tui --locked'; fi
        RESULT="would do"
        return 0
    fi

    confirm_release \
        "publish peruse-core $VER to crates.io" \
        "publish peruse-tui $VER to crates.io" \
        "a version on crates.io can be yanked, but never replaced and never freed"

    if [ "$core_has" = 1 ]; then
        ok "Already done, going on. peruse-core $VER is on crates.io."
    else
        if ! run_loud cargo publish -p peruse-core --locked; then
            die 'cargo publish -p peruse-core failed. Nothing went out. Read the output above.'
        fi
        ok "Published peruse-core $VER."
    fi

    # peruse-tui asks for peruse-core by version, so it cannot even be
    # packaged until the index holds that version.
    if ! wait_for_index peruse-core "$VER"; then
        die "crates.io did not index peruse-core $VER inside $INDEX_TIMEOUT_MINUTES minutes. peruse-core is published and that cannot be taken back. Wait, then run again with --from publish."
    fi

    if [ "$tui_has" = 1 ]; then
        ok "Already done, going on. peruse-tui $VER is on crates.io."
    else
        if ! run_loud cargo publish -p peruse-tui --locked; then
            fail 'cargo publish -p peruse-tui failed.'
            note 'peruse-core is published already, so run this again with --from publish when it is fixed.'
            note 'If the message names the lock file, try the same command without --locked and say so in the release notes.'
            die 'cargo publish -p peruse-tui failed.'
        fi
        ok "Published peruse-tui $VER."
    fi

    # Both crates are out by now, so a slow index is a thing to know about and
    # not a reason to end the run badly.
    if ! wait_for_index peruse-tui "$VER"; then
        warn "crates.io has not indexed peruse-tui $VER yet."
        warn 'Both crates are published. This was only the last look at the index.'
    fi
    RESULT="done"
}

# --------------------------------------------------------------------------
# 8 handoff
# --------------------------------------------------------------------------

step_handoff() {
    step_head 8 handoff 'what is left for a person to do'

    out="$HERE/out"
    note 'Each package tool takes the file in a different place, and none of them'
    note 'takes it from this repository. packaging/README.md says how, under the'
    note 'name of each tool.'
    printf '\n'
    note "Scoop        $out/scoop/peruse.json      -> the scoop-peruse bucket, in bucket/"
    note "Homebrew     $out/homebrew/peruse.rb     -> the homebrew-peruse tap, in Formula/"
    note "WinGet       $out/winget/*.yaml          -> a pull request to microsoft/winget-pkgs"
    note "Chocolatey   $out/chocolatey/            -> choco pack, then choco push"
    printf '\n'
    note 'Do Scoop and Homebrew first. They need no review, so a user can install'
    note 'Peruse the same day. WinGet and Chocolatey both wait for a person.'
    printf '\n'
    note "The release is at https://github.com/$REPO_NAME/releases/tag/v$VER"
    note 'The crates are at https://crates.io/crates/peruse-tui'
    RESULT="done"
}

# --------------------------------------------------------------------------
# The run
# --------------------------------------------------------------------------

while [ $# -gt 0 ]; do
    case "$1" in
        --execute) EXECUTE=1 ;;
        --skip-tests) SKIP_TESTS=1 ;;
        --no-publish) NO_PUBLISH=1 ;;
        --yes) YES=1 ;;
        --from)
            if [ $# -lt 2 ]; then
                printf -- '--from needs the name of a step. Give --help.\n' >&2
                exit 2
            fi
            FROM="$2"
            shift
            ;;
        --from=*) FROM="${1#--from=}" ;;
        -h | --help | -\?) usage; exit 0 ;;
        -*) printf 'There is no flag %s. Give --help.\n' "$1" >&2; exit 2 ;;
        *)
            if [ -n "$VER" ]; then
                printf 'Give one version only. Give --help.\n' >&2
                exit 2
            fi
            VER="$1"
            ;;
    esac
    shift
done

if [ -z "$VER" ]; then
    line 'usage: ./packaging/release.sh <version> [--execute] [--skip-tests] [--from <step>] [--no-publish] [--yes]'
    line ''
    line 'for example: ./packaging/release.sh 0.2.0             look, change nothing'
    line '             ./packaging/release.sh 0.2.0 --execute    do it'
    line ''
    line 'Give --help for all of it, with a worked example.'
    exit 2
fi

# A tag carries `v`, a version does not. Take either.
GIVEN="$VER"
VER="${VER#v}"
if ! printf '%s' "$VER" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    printf "The version must be X.Y.Z, and '%s' is not.\n" "$GIVEN" >&2
    printf 'A name such as 0.2.0-rc1 is not supported: the package templates and the\n' >&2
    printf 'manifests carry a plain version only.\n' >&2
    exit 2
fi
TAG="v$VER"

START=1
if [ -n "$FROM" ]; then
    if ! START=$(step_index "$FROM"); then
        printf "There is no step '%s'. The steps are: %s.\n" "$FROM" "$(printf '%s' "$STEPS" | tr ' ' ',' | sed 's/,/, /g')" >&2
        exit 2
    fi
fi

n=0
for name in $STEPS; do
    n=$((n + 1))
    if [ "$n" -lt "$START" ]; then
        set_plan "$n" "skip, before --from"
    elif [ "$name" = "publish" ] && [ "$NO_PUBLISH" = 1 ]; then
        set_plan "$n" "skip, --no-publish"
    elif [ "$name" = "preflight" ] && [ "$SKIP_TESTS" = 1 ]; then
        set_plan "$n" "run, quick checks only"
    else
        set_plan "$n" "run"
    fi
done

print_summary() {
    printf '\n'
    if [ "$EXECUTE" = 1 ]; then
        line "Release $VER: what was done"
    else
        line "Release $VER: what a real run would do"
    fi
    n=0
    for name in $STEPS; do
        n=$((n + 1))
        printf '  %s %-10s %s\n' "$n" "$name" "$(get_outcome "$n")"
    done
}

printf '\n'
if [ "$EXECUTE" = 1 ]; then
    line "Peruse release $VER   FOR REAL"
else
    line "Peruse release $VER   DRY RUN, nothing will change. Give --execute to do it."
fi
note "the repository is $ROOT"
note "the tag will be $TAG"
printf '\n'
line 'The plan:'
n=0
for name in $STEPS; do
    n=$((n + 1))
    printf '  %s %-10s %s\n' "$n" "$name" "$(get_plan "$n")"
done

cd "$ROOT"

# The dry run must leave the tree as it found it, and the run itself says so at
# the end.
BEFORE=""
if [ "$EXECUTE" != 1 ]; then
    BEFORE=$(git status --porcelain)
fi

n=0
for name in $STEPS; do
    n=$((n + 1))
    case "$(get_plan "$n")" in
        skip*) set_outcome "$n" "skipped"; continue ;;
    esac
    CURRENT="$name"
    CURRENT_INDEX="$n"
    RESULT=""
    case "$name" in
        preflight)
            step_preflight
            if [ "$RESULT" = "failed" ]; then
                printf '\n'
                line "The preflight found $PROBLEMS things:"
                printf '%s' "$PROBLEM_LIST" | while IFS= read -r p; do
                    if [ -n "$p" ]; then line "  * $p"; fi
                done
                if [ "$EXECUTE" = 1 ]; then
                    set_outcome "$n" "failed"
                    die 'The preflight failed, so nothing was done.'
                fi
                printf '\n'
                warn 'A real run would stop here. The dry run goes on, to show you the rest.'
            fi
            ;;
        bump) step_bump ;;
        tag) step_tag ;;
        workflow) step_workflow ;;
        verify) step_verify ;;
        render) step_render ;;
        publish) step_publish ;;
        handoff) step_handoff ;;
    esac
    set_outcome "$n" "$RESULT"
done

print_summary

if [ "$EXECUTE" != 1 ]; then
    printf '\n'
    AFTER=$(git status --porcelain)
    if [ "$BEFORE" = "$AFTER" ]; then
        ok 'The dry run changed nothing: git status says the same as before it.'
    else
        fail 'The working tree is not as it was. That is a fault in this script.'
        exit 1
    fi
    printf '\n'
    line "To do it:  ./packaging/release.sh $VER --execute"
fi
printf '\n'
