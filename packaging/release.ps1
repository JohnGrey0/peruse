<#
.SYNOPSIS
    Makes a whole release of Peruse, in the right order, from one command.

.DESCRIPTION
    This is release.sh, for PowerShell. The two do the same steps, in the same
    order, with the same flags. Use the one that your shell likes.

    The script does nothing until you give -Execute. Without it, the script
    reads the state of the repository, of the release and of crates.io, prints
    each step that a real run would take, and changes not one file.

    With -Execute the script prints the same plan, then asks you to type the
    tag. A tag on the remote, a release and a version on crates.io can never
    be taken back, so a person must say the words first.

    Every step looks first to see whether it is done already. A run after a
    failure part way through therefore says "already done" and goes on. Use
    -From to leave out the steps that finished.

.PARAMETER Version
    The version to release, as X.Y.Z, with or without the leading `v`. A name
    such as `0.2.0-rc1` is not supported, because the package templates and
    the manifests carry a plain version only.

.PARAMETER Execute
    Do the work. Without this flag the script is a dry run.

.PARAMETER SkipTests
    Leave out the long checks of the preflight: the tests, clippy
    and the two package dry runs. Use this on a second run, when the same
    commit passed them a few minutes ago. The quick checks still run.

.PARAMETER From
    Start at a step, and leave out the ones before it. The names are
    preflight, bump, tag, workflow, verify, render, publish and handoff.

.PARAMETER NoPublish
    Stop before crates.io. Every other step runs, so the tag, the release and
    the package manifests are all in place. Add `-From publish` later to
    finish.

.PARAMETER Yes
    Stand for the words that a person types to confirm. It needs -Execute as
    well. Give this only when you have taken the decision already: a version on
    crates.io stays there for ever.

.PARAMETER Help
    Print this help.

.EXAMPLE
    A whole release of 0.2.0.

    Write the section for 0.2.0 in CHANGELOG.md first, and commit it. Then
    look at what a release would do. This changes nothing:

        .\packaging\release.ps1 0.2.0

    Read the plan, then do it. The script asks you to type `v0.2.0`:

        .\packaging\release.ps1 0.2.0 -Execute

    Say a runner ran out of time and step 4 failed. Start the workflow again
    from the Actions page, then take the release up from there. The long
    checks passed a moment ago, so leave them out:

        .\packaging\release.ps1 0.2.0 -Execute -From workflow -SkipTests

    Say you want the tag and the release today and crates.io tomorrow:

        .\packaging\release.ps1 0.2.0 -Execute -NoPublish
        .\packaging\release.ps1 0.2.0 -Execute -From publish -SkipTests
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string] $Version,

    [switch] $Execute,

    [switch] $SkipTests,

    [string] $From,

    [switch] $NoPublish,

    [switch] $Yes,

    [switch] $Help
)

# --------------------------------------------------------------------------
# The steps of a release, in order. release.sh holds the same list. Keep the
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
#   5 verify     The release holds four archives and four .sha256 files, and
#                each .sha256 file holds 64 characters of hexadecimal and names
#                its own archive.
#   6 render     Run render.ps1, which fills the package manifests from the
#                checksums on the release.
#   7 publish    Publish peruse-core, wait for crates.io to index it, then
#                publish peruse-tui. The order is not free, because
#                peruse-tui names peruse-core by version.
#   8 handoff    Say which rendered file goes to which package repository.
# --------------------------------------------------------------------------

$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 offers an old TLS by default, and GitHub and
# crates.io both refuse it. Without this line every request fails, and the
# message does not say why.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$stepNames = @('preflight', 'bump', 'tag', 'workflow', 'verify', 'render', 'publish', 'handoff')

$repoName = 'JohnGrey0/peruse'
$here = $PSScriptRoot
$root = Split-Path -Parent $here

# The four targets that release.yml builds, and the form of archive that each
# one gives. The names here must agree with release.yml, or step 5 looks for
# files that no release holds.
$targets = @(
    @{ Target = 'x86_64-unknown-linux-gnu'; Archive = 'tar.gz' }
    @{ Target = 'x86_64-apple-darwin';      Archive = 'tar.gz' }
    @{ Target = 'aarch64-apple-darwin';     Archive = 'tar.gz' }
    @{ Target = 'x86_64-pc-windows-msvc';   Archive = 'zip' }
)

# How long each wait may take before the script gives up.
$workflowTimeoutMinutes = 120
$indexTimeoutMinutes = 20

$outcome = [ordered] @{}
foreach ($n in $stepNames) { $outcome[$n] = 'not reached' }

$problems = @()
$confirmed = $false

# --------------------------------------------------------------------------
# Words on the terminal
# --------------------------------------------------------------------------

function Write-Line { param([string] $Text = '') Write-Host $Text }
function Write-Note { param([string] $Text) Write-Host "      $Text" }
function Write-Ok { param([string] $Text) Write-Host "  ok  $Text" -ForegroundColor Green }
function Write-Warn { param([string] $Text) Write-Host "  --  $Text" -ForegroundColor Yellow }
function Write-Fail { param([string] $Text) Write-Host "  NO  $Text" -ForegroundColor Red }

function Write-Head {
    param([int] $Number, [string] $Name, [string] $Text)
    Write-Host ''
    Write-Host "== $Number $Name -- $Text" -ForegroundColor Cyan
}

# Holds a failed check. The preflight runs every check that it can before it
# stops, so that one run shows a person all the work, and not the first fault
# only.
function Add-Problem {
    param([string] $Text)
    $script:problems += $Text
    Write-Fail $Text
}

# --------------------------------------------------------------------------
# Other programs
# --------------------------------------------------------------------------

# Runs a program, keeps each line that it wrote, and gives the exit code.
#
# PowerShell 5.1 turns a line on the error stream of a program into an error
# record. With $ErrorActionPreference at Stop, that record ends the script,
# even when the program went on to succeed. The preference is therefore
# Continue for the length of the call, and the exit code is the only thing
# that decides.
function Invoke-Read {
    param([string] $Exe, [string[]] $Arguments = @())

    $keep = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $text = (& $Exe @Arguments 2>&1 | Out-String)
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $keep
    }
    return [pscustomobject] @{ Code = $code; Text = $text }
}

# Runs a program and lets it write to the terminal. A long command, such as
# the tests or a publish, must show its progress. Gives the exit code.
#
# Out-Host is necessary, and not a nicety. A program writes each of its lines
# to the success stream, and a function gives every line of that stream to its
# caller. Without Out-Host the caller would take those lines for the exit
# code, `git commit` would look like a failure, and the person would see
# nothing while the tests ran.
function Invoke-Loud {
    param([string] $Exe, [string[]] $Arguments = @())

    Write-Note "run: $Exe $($Arguments -join ' ')"
    $keep = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Exe @Arguments | Out-Host
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $keep
    }
    return $code
}

function Test-Program {
    param([string] $Name)
    return [bool] (Get-Command $Name -ErrorAction SilentlyContinue)
}

# The first line of the output of a program that reads state, with nothing
# around it. Gives an empty text when the program failed.
function Get-FirstLine {
    param([string] $Exe, [string[]] $Arguments = @())

    $r = Invoke-Read $Exe $Arguments
    if ($r.Code -ne 0) { return '' }
    $first = ($r.Text -split "`n")[0]
    if ($null -eq $first) { return '' }
    return $first.Trim()
}

# The commit that a tag names, or an empty text.
#
# Step 3 makes the tag with `git tag -a`, and such a tag is an object of its
# own. `refs/tags/X` therefore gives the tag and never the commit, so a
# comparison with HEAD must go through `^{commit}` first.
function Get-TagCommit {
    param([string] $Tag)
    return Get-FirstLine 'git' @('rev-parse', '-q', '--verify', "refs/tags/$Tag^{commit}")
}

# --------------------------------------------------------------------------
# Versions
# --------------------------------------------------------------------------

# Compares two versions of the form X.Y.Z. Gives 1, 0 or -1.
function Compare-Version {
    param([string] $A, [string] $B)

    $x = $A -split '\.'
    $y = $B -split '\.'
    for ($i = 0; $i -lt 3; $i++) {
        $a = [int] $x[$i]
        $b = [int] $y[$i]
        if ($a -gt $b) { return 1 }
        if ($a -lt $b) { return -1 }
    }
    return 0
}

# The place of a crate in the sparse index of crates.io. The rule comes from
# the length of the name.
function Get-IndexPath {
    param([string] $Crate)

    $n = $Crate.ToLowerInvariant()
    if ($n.Length -le 2) { return "$($n.Length)/$n" }
    if ($n.Length -eq 3) { return "3/$($n.Substring(0, 1))/$n" }
    return "$($n.Substring(0, 2))/$($n.Substring(2, 2))/$n"
}

# Every version of a crate that crates.io has indexed, newest last.
#
# The index gives one line of JSON for each version, and this needs one field
# of it, so a pattern is enough and no JSON reader is needed. A crate that
# was never published gives 404, which is an empty list and not a fault.
#
# A store sits in front of the index. Step 7 waits for a version to appear
# there, and must not read an old answer, so each call asks for a fresh one.
function Get-PublishedVersions {
    param([string] $Crate, [int] $Try = 0)

    $url = "https://index.crates.io/$(Get-IndexPath $Crate)?fresh=$Try"
    try {
        $res = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 60 `
            -Headers @{ 'Cache-Control' = 'no-cache'; 'Pragma' = 'no-cache' }
    }
    catch {
        $code = 0
        if ($_.Exception.Response) { $code = [int] $_.Exception.Response.StatusCode }
        if ($code -eq 404) { return @() }
        throw "Could not read the crates.io index at $url : $($_.Exception.Message)"
    }

    $found = @()
    foreach ($line in ($res.Content -split "`n")) {
        $m = [regex]::Match($line, '"vers"\s*:\s*"([^"]+)"')
        if ($m.Success) { $found += $m.Groups[1].Value }
    }
    return $found
}

# The highest version of a crate on crates.io, or an empty text.
#
# A name with a letter in it, such as a release candidate, takes no part in
# the comparison, because this script releases plain versions only.
function Get-HighestPublished {
    param([string] $Crate)

    $best = ''
    foreach ($v in @(Get-PublishedVersions $Crate)) {
        if ($v -notmatch '^\d+\.\d+\.\d+$') { continue }
        if ($best -eq '' -or (Compare-Version $v $best) -gt 0) { $best = $v }
    }
    return $best
}

# --------------------------------------------------------------------------
# The version in the manifests and in the lock file
# --------------------------------------------------------------------------

$rootManifest = Join-Path $root 'Cargo.toml'
$coreManifest = Join-Path $root 'crates\peruse-core\Cargo.toml'
$tuiManifest = Join-Path $root 'crates\peruse-tui\Cargo.toml'
$lockFile = Join-Path $root 'Cargo.lock'

# Splits text into lines and keeps the line ending on each one. A file that
# goes back through Join therefore comes out byte for byte the same, and one
# line can change on its own.
function Split-Keep {
    param([string] $Text)
    return [regex]::Split($Text, '(?<=\n)')
}

function Read-Whole {
    param([string] $Path)
    return [System.IO.File]::ReadAllText($Path)
}

function Write-Whole {
    param([string] $Path, [string] $Text)
    # No byte order mark: a mark at the start of a manifest makes cargo fail.
    $utf8NoBom = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $Text, $utf8NoBom)
}

# The numbers of the lines that hold `version = "..."` in the
# [workspace.package] section of the root manifest.
function Get-WorkspaceVersionLines {
    param([string[]] $Lines)

    $inSection = $false
    $hits = @()
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match '^\s*\[') {
            $inSection = ($Lines[$i] -match '^\s*\[workspace\.package\]')
            continue
        }
        if ($inSection -and $Lines[$i] -match '^\s*version\s*=\s*"[^"]+"') { $hits += $i }
    }
    return $hits
}

# Every line of a manifest that names peruse-core with a version written out
# in full.
#
# crates/peruse-tui/Cargo.toml holds one such line. It does not say
# `version.workspace = true`, so a bump that misses it leaves peruse-tui
# asking for an old peruse-core, and the publish then fails or publishes
# something wrong. This is the reason the script looks for the line, counts
# it, and stops when the count is not one.
function Get-DepVersionLines {
    param([string[]] $Lines)

    $hits = @()
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match '^\s*peruse-core\s*=' -and $Lines[$i] -match 'version\s*=\s*"[^"]+"') {
            $hits += $i
        }
    }
    return $hits
}

function Get-LineVersion {
    param([string] $Line)
    $m = [regex]::Match($Line, 'version\s*=\s*"([^"]+)"')
    if ($m.Success) { return $m.Groups[1].Value }
    return ''
}

# The version of a package in Cargo.lock.
function Get-LockVersion {
    param([string] $Text, [string] $Crate)

    $m = [regex]::Match($Text, "name = ""$([regex]::Escape($Crate))""\s*\r?\nversion = ""([^""]+)""")
    if ($m.Success) { return $m.Groups[1].Value }
    return ''
}

# Reads the version out of every place that carries it. The bump and the
# preflight both work from this one answer.
function Read-VersionState {
    $rootLines = Split-Keep (Read-Whole $rootManifest)
    $tuiLines = Split-Keep (Read-Whole $tuiManifest)
    $coreLines = Split-Keep (Read-Whole $coreManifest)
    $lockText = Read-Whole $lockFile

    # Each list comes back through @(), because a function that gives one
    # item, or none, gives a plain value or nothing at all, and neither of
    # those answers Count.
    $wsHits = @(Get-WorkspaceVersionLines $rootLines)
    $tuiHits = @(Get-DepVersionLines $tuiLines)
    # A line of this form belongs in the front end only. The count covers the
    # other two manifests as well, so that a line which moves is not missed.
    $strayHits = @(Get-DepVersionLines $rootLines).Count + @(Get-DepVersionLines $coreLines).Count

    $ws = ''
    if ($wsHits.Count -eq 1) { $ws = Get-LineVersion $rootLines[$wsHits[0]] }
    $dep = ''
    if ($tuiHits.Count -eq 1) { $dep = Get-LineVersion $tuiLines[$tuiHits[0]] }

    return [pscustomobject] @{
        Workspace      = $ws
        WorkspaceHits  = $wsHits.Count
        Dep            = $dep
        DepHits        = $tuiHits.Count
        StrayDepHits   = $strayHits
        LockCore       = Get-LockVersion $lockText 'peruse-core'
        LockTui        = Get-LockVersion $lockText 'peruse-tui'
    }
}

# Writes one version into the root manifest and into the peruse-core line of
# the front end. Nothing else in either file changes.
function Write-VersionState {
    param([string] $NewVersion)

    $repl = '${1}' + $NewVersion + '${2}'

    # Both files are read and both are checked before either one is written.
    # A write that stopped half way would leave the workspace at the new
    # version and the front end still asking for the old peruse-core, which
    # is the very fault that this step is here to stop.
    $rootLines = Split-Keep (Read-Whole $rootManifest)
    $wsHits = @(Get-WorkspaceVersionLines $rootLines)
    if ($wsHits.Count -ne 1) {
        throw "Cargo.toml holds $($wsHits.Count) version lines in [workspace.package]. It must hold one. Fix the file, then run again."
    }

    $tuiLines = Split-Keep (Read-Whole $tuiManifest)
    $tuiHits = @(Get-DepVersionLines $tuiLines)
    if ($tuiHits.Count -ne 1) {
        throw "crates/peruse-tui/Cargo.toml holds $($tuiHits.Count) lines that name peruse-core with a version written out in full. It must hold one. Fix the file, then run again."
    }

    $stray = @(Get-DepVersionLines $rootLines).Count + @(Get-DepVersionLines (Split-Keep (Read-Whole $coreManifest))).Count
    if ($stray -ne 0) {
        throw "Another manifest names peruse-core with a version written out in full. Only crates/peruse-tui/Cargo.toml may. Look at Cargo.toml and crates/peruse-core/Cargo.toml."
    }

    $rootLines[$wsHits[0]] = $rootLines[$wsHits[0]] -replace '^(\s*version\s*=\s*")[^"]+(")', $repl
    $tuiLines[$tuiHits[0]] = $tuiLines[$tuiHits[0]] -replace '(version\s*=\s*")[^"]+(")', $repl

    Write-Whole $rootManifest (-join $rootLines)
    Write-Note "Cargo.toml [workspace.package] version = ""$NewVersion"""
    Write-Whole $tuiManifest (-join $tuiLines)
    Write-Note ("crates/peruse-tui/Cargo.toml " + $tuiLines[$tuiHits[0]].Trim())
}

# --------------------------------------------------------------------------
# The release on GitHub
# --------------------------------------------------------------------------

function Get-ReleaseFileNames {
    param([string] $Ver)

    $names = @()
    foreach ($t in $targets) {
        $name = "peruse-$Ver-$($t.Target).$($t.Archive)"
        $names += $name
        $names += "$name.sha256"
    }
    return $names
}

# Looks for one file on the release. A release that is not there, and a file
# that is not on it, both give 404, and both mean the same thing here: the
# workflow has not put the file up yet.
function Test-ReleaseFile {
    param([string] $Ver, [string] $Name)

    $url = "https://github.com/$repoName/releases/download/v$Ver/$Name"
    try {
        $res = Invoke-WebRequest -Uri $url -Method Head -UseBasicParsing -TimeoutSec 60
        return ([int] $res.StatusCode -eq 200)
    }
    catch {
        $code = 0
        if ($_.Exception.Response) { $code = [int] $_.Exception.Response.StatusCode }
        if ($code -eq 404) { return $false }
        throw "Could not ask about $url : $($_.Exception.Message)"
    }
}

# Reads one .sha256 file from the release and checks what it holds.
#
# A file of the right name can still hold the wrong thing: an upload that
# stopped, a page of error text, or nothing at all. The file holds the output of
# `sha256sum`, which is the checksum, then a space or two, then the name of the
# archive. This function checks the two parts and gives the reason when one of
# them is wrong. It gives `$null` when the file is good.
#
# A .sha256 file is about one hundred bytes, so this check costs nothing. A check
# of the archive itself would read about one hundred megabytes for each platform,
# and it would find almost nothing that this check does not: GitHub gives the
# archive back with its own checksum.
function Test-ReleaseChecksum {
    param([string] $Ver, [string] $Archive)

    $url = "https://github.com/$repoName/releases/download/v$Ver/$Archive.sha256"
    try {
        $body = (Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 60).Content
    }
    catch {
        return "could not read $Archive.sha256"
    }

    $first = ($body -split "`n")[0].Trim()
    $parts = @($first -split '\s+' | Where-Object { $_ -ne '' })
    if ($parts.Count -lt 1 -or $parts[0] -notmatch '^[0-9a-fA-F]{64}$') {
        return "$Archive.sha256 does not hold 64 characters of hexadecimal"
    }
    # `sha256sum` writes a star in front of the name for a file that it read as
    # bytes. The name may also carry a directory.
    if ($parts.Count -ge 2) {
        $named = Split-Path -Leaf ($parts[-1].TrimStart('*'))
        if ($named -ne $Archive) {
            return "$Archive.sha256 names $named"
        }
    }
    return $null
}

# The names of the archives on the release, with no .sha256 file among them.
function Get-ReleaseArchiveNames {
    param([string] $Ver)

    return @(Get-ReleaseFileNames $Ver | Where-Object { $_ -notlike '*.sha256' })
}

# The names of the files that the release does not hold yet.
function Get-MissingReleaseFiles {
    param([string] $Ver)

    $missing = @()
    foreach ($name in (Get-ReleaseFileNames $Ver)) {
        if (-not (Test-ReleaseFile $Ver $name)) { $missing += $name }
    }
    return $missing
}

# --------------------------------------------------------------------------
# The words that a person must type
# --------------------------------------------------------------------------

# Asks once, before the first step that cannot be taken back.
#
# The word to type is the tag itself. A person who is part way through
# something else cannot answer it from memory of another release.
function Confirm-Release {
    param([string] $Tag, [string[]] $Actions)

    if ($script:confirmed) { return }

    Write-Line ''
    Write-Host 'These things cannot be taken back:' -ForegroundColor Yellow
    foreach ($a in $Actions) { Write-Host "  * $a" -ForegroundColor Yellow }
    Write-Line ''
    Write-Host "A version on crates.io stays there for ever. It cannot be replaced." -ForegroundColor Yellow
    Write-Line ''

    # The option -Yes takes the place of the words. It is for a person who
    # cannot type into this run, such as a tool that drives the release. It needs
    # -Execute as well, so a run with no option still changes nothing.
    #
    # The option is not a way to release with nobody there. The person who gives
    # -Yes takes the same decision as the person who types the words, and takes
    # it before the run starts.
    if ($Yes) {
        Write-Host 'The option -Yes stands for the words. Going on.' -ForegroundColor Yellow
        $script:confirmed = $true
        Write-Line ''
        return
    }

    # A person must type the words, so an answer that comes from a file or
    # from another program is not an answer. release.sh asks the same question
    # of its own input.
    if ([Console]::IsInputRedirected) {
        throw "This needs a terminal, because a person must confirm the release. Run it in a terminal, or give -Yes."
    }

    $typed = ''
    try {
        $typed = Read-Host "Type $Tag to go on, or anything else to stop"
    }
    catch {
        throw "This needs a terminal, because a person must confirm the release. Run it in a terminal."
    }

    if ($typed.Trim() -ne $Tag) {
        throw "You typed something else, so nothing was done."
    }
    $script:confirmed = $true
    Write-Line ''
}

# --------------------------------------------------------------------------
# 1 preflight
# --------------------------------------------------------------------------

function Invoke-Preflight {
    param([string] $Ver, [string] $Tag, [bool] $PublishPlanned)

    Write-Head 1 'preflight' 'the state of the repository, of the tag and of crates.io'

    if (-not (Test-Program 'git')) { Add-Problem 'git is not on the path.' }
    if (-not (Test-Program 'cargo')) { Add-Problem 'cargo is not on the path.' }
    if ($problems.Count -gt 0) { return 'failed' }

    # The tree must hold nothing of its own, because step 3 commits the three
    # files that the bump touches, and a commit that carries other work makes
    # a release that nobody can read later.
    $dirty = (Invoke-Read 'git' @('status', '--porcelain')).Text.Trim()
    if ($dirty -eq '') {
        Write-Ok 'The working tree is clean.'
    }
    else {
        Add-Problem 'The working tree holds changes. Commit them or put them away first.'
        foreach ($l in ($dirty -split "`n")) { Write-Note $l.Trim() }
    }

    $branch = Get-FirstLine 'git' @('rev-parse', '--abbrev-ref', 'HEAD')
    if ($branch -eq 'main') {
        Write-Ok 'The branch is main.'
    }
    else {
        Add-Problem "The branch is '$branch' and a release comes from main."
    }

    # `ls-remote` asks the remote and writes nothing here, so a dry run leaves
    # even the git directory as it was.
    $head = Get-FirstLine 'git' @('rev-parse', 'HEAD')
    $remoteMain = ''
    $r = Invoke-Read 'git' @('ls-remote', 'origin', 'refs/heads/main')
    if ($r.Code -eq 0) {
        $m = [regex]::Match($r.Text, '([0-9a-f]{40})\s+refs/heads/main')
        if ($m.Success) { $remoteMain = $m.Groups[1].Value }
    }
    if ($remoteMain -eq '') {
        Add-Problem 'Could not read refs/heads/main from origin. Is there a network?'
    }
    elseif ($remoteMain -eq $head) {
        Write-Ok "main is level with origin/main, at $($head.Substring(0, 7))."
    }
    else {
        Add-Problem "main is not level with origin/main. Here is $($head.Substring(0, 7)) and there is $($remoteMain.Substring(0, 7)). Pull or push first."
    }

    # The tag must be free. A tag that is there already means that this
    # version went out, or that a release stopped part way. Neither is a thing
    # to walk over: use -From to take up a release that stopped.
    $localTag = Get-FirstLine 'git' @('rev-parse', '-q', '--verify', "refs/tags/$Tag")
    if ($localTag -eq '') {
        Write-Ok "There is no tag $Tag here."
    }
    else {
        $at = Get-TagCommit $Tag
        if ($at -eq '') { $at = $localTag }
        Add-Problem "The tag $Tag is here already, at $($at.Substring(0, 7)). Give -From to take up a release that stopped part way."
    }

    $r = Invoke-Read 'git' @('ls-remote', '--tags', 'origin', "refs/tags/$Tag")
    if ($r.Code -ne 0) {
        Add-Problem 'Could not read the tags of origin. Is there a network?'
    }
    elseif ($r.Text.Trim() -eq '') {
        Write-Ok "There is no tag $Tag on origin."
    }
    else {
        Add-Problem "The tag $Tag is on origin already. release.yml has run for it."
    }

    # release.yml builds with --locked, so the lock file must hold the version
    # that goes out. Step 2 sees to that.
    $state = Read-VersionState
    if ($state.WorkspaceHits -ne 1) {
        Add-Problem "Cargo.toml holds $($state.WorkspaceHits) version lines in [workspace.package]. It must hold one."
    }
    if ($state.DepHits -ne 1) {
        Add-Problem "crates/peruse-tui/Cargo.toml holds $($state.DepHits) lines that name peruse-core with a version written out in full. It must hold one."
    }
    if ($state.StrayDepHits -ne 0) {
        Add-Problem "Another manifest names peruse-core with a version written out in full. Only crates/peruse-tui/Cargo.toml may. Look at Cargo.toml and crates/peruse-core/Cargo.toml."
    }
    if ($state.WorkspaceHits -eq 1 -and $state.DepHits -eq 1) {
        Write-Ok "The manifests say $($state.Workspace), and peruse-tui asks for peruse-core $($state.Dep)."
    }

    if ($state.Workspace -ne '') {
        $c = Compare-Version $Ver $state.Workspace
        if ($c -lt 0) {
            Add-Problem "The manifests say $($state.Workspace), which is higher than $Ver. A release does not go backwards."
        }
        elseif ($c -eq 0) {
            Write-Warn "The manifests say $Ver already, so step 2 has nothing to do."
        }
    }

    $changelog = Read-Whole (Join-Path $root 'CHANGELOG.md')
    if ($changelog -match "(?m)^##\s+\[?v?$([regex]::Escape($Ver))\]?(\s|$)") {
        Write-Ok "CHANGELOG.md holds a section for $Ver."
    }
    else {
        Add-Problem "CHANGELOG.md holds no section for $Ver. Add '## $Ver' with the changes, and commit it."
    }

    # A release that is lower than the one on crates.io cannot be published,
    # and finding that out after the tag is on the remote is too late.
    $core = ''
    $tui = ''
    try {
        $core = Get-HighestPublished 'peruse-core'
        $tui = Get-HighestPublished 'peruse-tui'
    }
    catch {
        # A preflight that cannot read the index must say so and go on, so
        # that one run shows every other fault as well.
        Add-Problem 'Could not read the crates.io index. Is there a network?'
    }
    if ($core -eq '') { Write-Warn 'peruse-core is not on crates.io yet.' }
    if ($tui -eq '') { Write-Warn 'peruse-tui is not on crates.io yet.' }
    if ($core -ne '' -and $tui -ne '' -and $core -ne $tui) {
        Write-Warn "crates.io holds peruse-core $core and peruse-tui $tui. A release stopped between the two."
    }
    foreach ($pair in @(@('peruse-core', $core), @('peruse-tui', $tui))) {
        $name = $pair[0]
        $high = $pair[1]
        if ($high -eq '') { continue }
        if ((Compare-Version $Ver $high) -gt 0) {
            Write-Ok "$Ver is higher than $name $high on crates.io."
        }
        else {
            Add-Problem "crates.io holds $name $high already, and $Ver is not higher. A version on crates.io can never be replaced."
        }
    }

    # gh gives the state of the workflow run. Step 4 can wait without it, by
    # watching the files appear on the release, but the message when a build
    # fails is much poorer.
    if (Test-Program 'gh') {
        $v = Get-FirstLine 'gh' @('--version')
        Write-Ok "gh is here: $v"
    }
    else {
        Write-Warn 'gh is not on the path. Step 4 will watch the release files instead, and cannot show you a log.'
    }

    if ($PublishPlanned) {
        $token = $false
        if ($env:CARGO_REGISTRY_TOKEN) {
            Write-Ok 'A crates.io token is in CARGO_REGISTRY_TOKEN.'
            $token = $true
        }
        else {
            $cargoHome = $env:CARGO_HOME
            if (-not $cargoHome) { $cargoHome = Join-Path $env:USERPROFILE '.cargo' }
            foreach ($f in @('credentials.toml', 'credentials')) {
                $p = Join-Path $cargoHome $f
                if (Test-Path $p) {
                    # The path only. The token itself never goes on the
                    # terminal, where it would stay in the buffer.
                    Write-Ok "cargo holds credentials at $p."
                    $token = $true
                    break
                }
            }
        }
        if (-not $token) {
            Add-Problem 'No crates.io token. Run `cargo login`, or set CARGO_REGISTRY_TOKEN, before step 7.'
        }
    }

    $renderScript = Join-Path $here 'render.ps1'
    if (Test-Path $renderScript) {
        Write-Ok 'render.ps1 is here, for step 6.'
    }
    else {
        Add-Problem "There is no $renderScript, and step 6 needs it."
    }

    if ($problems.Count -gt 0) {
        Write-Line ''
        Write-Warn 'The quick checks found something, so the long ones did not run.'
        return 'failed'
    }

    if ($SkipTests) {
        Write-Warn '-SkipTests: the tests, clippy and the two package dry runs did not run.'
        return 'done'
    }

    # The long checks. Each one is the same command that CI runs, so a fault
    # here is a fault that the release workflow would meet as well.
    Write-Line ''
    Write-Note 'The long checks now. The tests and clippy take some minutes.'

    # There is no check of the format here, and that is on purpose. This project
    # writes its code by hand and holds no rustfmt settings, and `ci.yml` runs the
    # tests and clippy and no format check. A format check would refuse every
    # release over 259 places that no change of today touched, and a gate that the
    # project does not hold itself to is a wall and not a gate.
    $checks = @(
        @{ What = 'the tests'; Exe = 'cargo'; Args = @('test', '--workspace') }
        @{ What = 'clippy'; Exe = 'cargo'; Args = @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings') }
        @{ What = 'the package of peruse-core'; Exe = 'cargo'; Args = @('publish', '--dry-run', '--locked', '-p', 'peruse-core') }
        @{ What = 'the package of peruse-tui'; Exe = 'cargo'; Args = @('publish', '--dry-run', '--locked', '-p', 'peruse-tui') }
    )
    foreach ($c in $checks) {
        Write-Line ''
        $code = Invoke-Loud $c.Exe $c.Args
        if ($code -eq 0) {
            Write-Ok "$($c.What) is clean."
            continue
        }
        # The front end names the engine by version. Before the engine of that
        # version is on crates.io, the dry run of the front end cannot resolve
        # it, and CI leaves the front end out for the same reason. That is not
        # a fault of this release.
        if ($c.What -eq 'the package of peruse-tui') {
            $have = @(Get-PublishedVersions 'peruse-core')
            if ($have -notcontains $state.Dep) {
                Write-Warn "The dry run of peruse-tui could not resolve peruse-core $($state.Dep), which is not on crates.io. Step 7 publishes the engine first, and that is when this can be checked."
                continue
            }
        }
        Add-Problem "$($c.What) failed. Read the output above."
    }

    if ($problems.Count -gt 0) { return 'failed' }
    return 'done'
}

# --------------------------------------------------------------------------
# 2 bump
# --------------------------------------------------------------------------

function Invoke-Bump {
    param([string] $Ver, [string] $Tag)

    Write-Head 2 'bump' "write $Ver into the manifests and the lock file"

    $state = Read-VersionState
    $lockOk = ($state.LockCore -eq $Ver -and $state.LockTui -eq $Ver)
    if ($state.Workspace -eq $Ver -and $state.Dep -eq $Ver -and $lockOk) {
        Write-Ok "Already done, going on. Everything says $Ver."
        return 'already done'
    }

    Write-Note "Cargo.toml [workspace.package] version: $($state.Workspace) -> $Ver"
    Write-Note "crates/peruse-tui/Cargo.toml peruse-core version: $($state.Dep) -> $Ver"
    Write-Note "Cargo.lock peruse-core $($state.LockCore) and peruse-tui $($state.LockTui) -> $Ver"

    if (-not $Execute) { return 'would do' }

    # The first step that writes anything, so the question comes here. Step 3
    # asks nothing more, because a person answers once for the whole release.
    Confirm-Release $Tag @(
        "commit the version bump and push it to origin/main",
        "push the tag $Tag, which starts release.yml and makes a public release"
    )

    Write-VersionState $Ver

    # release.yml builds with --locked. A lock file that still holds the old
    # version therefore stops the release, so the lock must move with the
    # manifests.
    $code = Invoke-Loud 'cargo' @('update', '-p', 'peruse-core', '-p', 'peruse-tui')
    if ($code -ne 0) {
        Write-Warn 'That did not work. Trying the whole workspace.'
        $code = Invoke-Loud 'cargo' @('update', '--workspace')
    }
    if ($code -ne 0) {
        throw "Could not write Cargo.lock. Run ``cargo check`` by hand, then run this again with -From tag."
    }

    $state = Read-VersionState
    if ($state.Workspace -ne $Ver) { throw "Cargo.toml still says $($state.Workspace)." }
    if ($state.Dep -ne $Ver) { throw "crates/peruse-tui/Cargo.toml still asks for peruse-core $($state.Dep)." }
    if ($state.LockCore -ne $Ver) { throw "Cargo.lock still holds peruse-core $($state.LockCore)." }
    if ($state.LockTui -ne $Ver) { throw "Cargo.lock still holds peruse-tui $($state.LockTui)." }
    Write-Ok "The manifests and the lock file all say $Ver."
    return 'done'
}

# --------------------------------------------------------------------------
# 3 tag
# --------------------------------------------------------------------------

function Invoke-Tag {
    param([string] $Ver, [string] $Tag)

    Write-Head 3 'tag' "commit the bump, tag $Tag, push main, then push the tag"

    $files = @('Cargo.toml', 'Cargo.lock', 'crates/peruse-tui/Cargo.toml')

    # The three files of the bump only. Another file in the tree is not a
    # thing for this step to commit, and a run that takes a release up from
    # here with -From leaves the preflight out, so the tree can hold work of
    # its own. A commit with nothing in it fails, so the question this step
    # must ask is whether these three files differ from HEAD.
    $dirty = (Invoke-Read 'git' (@('status', '--porcelain', '--') + $files)).Text.Trim()
    $head = Get-FirstLine 'git' @('rev-parse', 'HEAD')
    $localTag = Get-FirstLine 'git' @('rev-parse', '-q', '--verify', "refs/tags/$Tag")
    $localCommit = Get-TagCommit $Tag
    if ($localCommit -eq '') { $localCommit = $localTag }

    $r = Invoke-Read 'git' @('ls-remote', '--tags', 'origin', "refs/tags/$Tag")
    if ($r.Code -ne 0) { throw 'Could not read the tags of origin. Is there a network?' }
    $remoteTag = ''
    $m = [regex]::Match($r.Text, '([0-9a-f]{40})')
    if ($m.Success) { $remoteTag = $m.Groups[1].Value }

    $r = Invoke-Read 'git' @('ls-remote', 'origin', 'refs/heads/main')
    if ($r.Code -ne 0) { throw 'Could not read refs/heads/main from origin. Is there a network?' }
    $remoteMain = ''
    $m = [regex]::Match($r.Text, '([0-9a-f]{40})\s+refs/heads/main')
    if ($m.Success) { $remoteMain = $m.Groups[1].Value }

    # A tag that is on the remote at another commit is the worst thing that
    # can happen here, because the release and the crates would come from
    # different code. The script never moves a tag.
    if ($remoteTag -ne '' -and $localTag -ne '' -and $remoteTag -ne $localTag) {
        throw "The tag $Tag is at $($localTag.Substring(0,7)) here and at $($remoteTag.Substring(0,7)) on origin. Sort that out by hand."
    }

    if (-not $Execute) {
        # A dry run reads a tree that step 2 has not written to, so the answer
        # to "is there anything to commit" must count the bump that is to come.
        $state = Read-VersionState
        $coming = ($dirty -ne '' -or $state.Workspace -ne $Ver -or $state.Dep -ne $Ver -or
            $state.LockCore -ne $Ver -or $state.LockTui -ne $Ver)

        if (-not $coming) { Write-Note 'There is nothing to commit.' }
        else { Write-Note "git add and git commit -m ""Release $Tag"", for: $($files -join ', ')" }
        if ($localTag -eq '') { Write-Note "git tag -a $Tag" } else { Write-Note "The tag $Tag is here already." }
        if ($remoteMain -eq $head -and -not $coming) { Write-Note 'origin/main is level already.' } else { Write-Note 'git push origin main' }
        if ($remoteTag -eq '') { Write-Note "git push origin refs/tags/$Tag" } else { Write-Note "The tag $Tag is on origin already." }
        return 'would do'
    }

    Confirm-Release $Tag @(
        "commit the version bump and push it to origin/main",
        "push the tag $Tag, which starts release.yml and makes a public release"
    )

    if ($dirty -eq '') {
        Write-Ok 'Already done, going on. There is nothing to commit.'
    }
    else {
        $paths = @('add', '--') + $files
        $code = Invoke-Loud 'git' $paths
        if ($code -ne 0) { throw 'git add failed.' }
        # Only the three files of the bump go in. Anything else in the tree
        # stays out, and the preflight has said that there is nothing else.
        $code = Invoke-Loud 'git' @('commit', '-m', "Release $Tag")
        if ($code -ne 0) { throw 'git commit failed.' }
        $head = Get-FirstLine 'git' @('rev-parse', 'HEAD')
        Write-Ok "Committed the bump at $($head.Substring(0,7))."
    }

    if ($localTag -ne '') {
        if ($localCommit -ne $head) {
            throw "The tag $Tag is at $($localCommit.Substring(0,7)) and HEAD is at $($head.Substring(0,7)). Sort that out by hand."
        }
        Write-Ok "Already done, going on. The tag $Tag is at HEAD."
    }
    else {
        $code = Invoke-Loud 'git' @('tag', '-a', $Tag, '-m', "peruse $Tag")
        if ($code -ne 0) { throw 'git tag failed.' }
        Write-Ok "Made the tag $Tag."
    }

    if ($remoteMain -eq $head) {
        Write-Ok 'Already done, going on. origin/main is level.'
    }
    else {
        # The branch goes first. A tag that arrives before the commit that it
        # names leaves the release pointing at nothing.
        $code = Invoke-Loud 'git' @('push', 'origin', 'main')
        if ($code -ne 0) { throw 'git push origin main failed.' }
        Write-Ok 'Pushed main.'
    }

    if ($remoteTag -ne '') {
        Write-Ok "Already done, going on. The tag $Tag is on origin."
    }
    else {
        $code = Invoke-Loud 'git' @('push', 'origin', "refs/tags/$Tag")
        if ($code -ne 0) { throw "git push origin refs/tags/$Tag failed." }
        Write-Ok "Pushed the tag $Tag. release.yml starts now."
    }
    return 'done'
}

# --------------------------------------------------------------------------
# 4 workflow
# --------------------------------------------------------------------------

# The run of release.yml for this tag, or an empty text.
function Get-WorkflowRun {
    param([string] $Tag)

    $r = Invoke-Read 'gh' @('run', 'list', '--workflow', 'release.yml', '--branch', $Tag,
        '--limit', '1', '--json', 'databaseId,status,conclusion,url')
    if ($r.Code -ne 0) { return $null }
    $m = [regex]::Match($r.Text, '"databaseId"\s*:\s*(\d+)')
    if (-not $m.Success) { return $null }
    return $m.Groups[1].Value
}

function Invoke-Workflow {
    param([string] $Ver, [string] $Tag)

    Write-Head 4 'workflow' 'wait for release.yml to build the four platforms'

    Write-Note "release.yml builds for four platforms and compiles DuckDB for each."
    Write-Note "30 to 60 minutes is normal, and this waits up to $workflowTimeoutMinutes minutes."

    # The count comes from the list of targets. Another platform in that list
    # therefore needs no change here.
    $expected = @(Get-ReleaseFileNames $Ver).Count

    $missing = @(Get-MissingReleaseFiles $Ver)
    if ($missing.Count -eq 0) {
        Write-Ok "Already done, going on. The release v$Ver holds all $expected files."
        return 'already done'
    }

    if (-not $Execute) {
        Write-Note "$($missing.Count) of the $expected files are not up yet, so a real run would wait here."
        Write-Note "https://github.com/$repoName/actions/workflows/release.yml"
        return 'would do'
    }

    $deadline = (Get-Date).AddMinutes($workflowTimeoutMinutes)
    $useGh = Test-Program 'gh'
    $runId = $null

    if ($useGh) {
        # The run takes a few seconds to appear after the push.
        for ($i = 0; $i -lt 20; $i++) {
            $runId = Get-WorkflowRun $Tag
            if ($runId) { break }
            Start-Sleep -Seconds 6
        }
        if (-not $runId) {
            Write-Warn "gh found no run for $Tag. Watching the release files instead."
            $useGh = $false
        }
        else {
            Write-Ok "The run is https://github.com/$repoName/actions/runs/$runId"
        }
    }

    # A poll, and not `gh run watch`, because a poll can hold a timeout and
    # can say something every time, and because the way without gh must work
    # in the same shape.
    while ((Get-Date) -lt $deadline) {
        if ($useGh) {
            $r = Invoke-Read 'gh' @('run', 'view', $runId, '--json', 'status,conclusion')
            if ($r.Code -eq 0) {
                $status = ''
                $conclusion = ''
                $m = [regex]::Match($r.Text, '"status"\s*:\s*"([^"]*)"')
                if ($m.Success) { $status = $m.Groups[1].Value }
                $m = [regex]::Match($r.Text, '"conclusion"\s*:\s*"([^"]*)"')
                if ($m.Success) { $conclusion = $m.Groups[1].Value }

                if ($status -eq 'completed') {
                    if ($conclusion -eq 'success') {
                        Write-Ok 'The workflow finished well.'
                        return 'done'
                    }
                    Write-Fail "The workflow ended as '$conclusion'."
                    Write-Note "Read the log with:  gh run view $runId --log-failed"
                    Write-Note "Or open https://github.com/$repoName/actions/runs/$runId"
                    throw "release.yml did not succeed. The tag is on the remote, so fix the build, run the workflow again, and take this up with -From workflow."
                }
                Write-Warn "$(Get-Date -Format 'HH:mm:ss') the run is $status."
            }
            else {
                Write-Warn 'gh could not read the run. Trying again.'
            }
        }
        else {
            $missing = @(Get-MissingReleaseFiles $Ver)
            $have = $expected - $missing.Count
            if ($missing.Count -eq 0) {
                Write-Ok "All $expected files are on the release."
                return 'done'
            }
            Write-Warn "$(Get-Date -Format 'HH:mm:ss') $have of $expected files are up."
        }
        Start-Sleep -Seconds 30
    }

    throw "release.yml did not finish inside $workflowTimeoutMinutes minutes. Look at https://github.com/$repoName/actions/workflows/release.yml and take this up with -From workflow."
}

# --------------------------------------------------------------------------
# 5 verify
# --------------------------------------------------------------------------

function Invoke-Verify {
    param([string] $Ver)

    Write-Head 5 'verify' 'the release is whole and each checksum reads as a checksum'

    $missing = @(Get-MissingReleaseFiles $Ver)
    foreach ($name in @(Get-ReleaseFileNames $Ver)) {
        if ($missing -contains $name) { Write-Fail $name } else { Write-Ok $name }
    }

    if ($missing.Count -eq 0) {
        # A file that is there can still hold the wrong thing. Read each
        # checksum and check it before the manifests carry it to four package
        # repositories, where a wrong checksum stops every install.
        $bad = @()
        foreach ($archive in Get-ReleaseArchiveNames $Ver) {
            $why = Test-ReleaseChecksum $Ver $archive
            if ($why) { $bad += $why; Write-Fail $why } else { Write-Ok "$archive.sha256 holds a checksum" }
        }
        if ($bad.Count -gt 0) {
            throw "The release v$Ver holds $($bad.Count) checksum files that are not right. Run the workflow again for the platforms that failed, then take this up with -From verify."
        }
        Write-Ok "The release v$Ver is whole, and each checksum reads as a checksum."
        return 'done'
    }

    if (-not $Execute) {
        Write-Note "$($missing.Count) files are not there. In a real run this would stop here."
        Write-Note "The release v$Ver does not exist yet, which is what a dry run before a release shows."
        return 'would do'
    }
    throw "The release v$Ver is short of $($missing.Count) files. Look at the workflow, then take this up with -From workflow."
}

# --------------------------------------------------------------------------
# 6 render
# --------------------------------------------------------------------------

function Invoke-Render {
    param([string] $Ver)

    Write-Head 6 'render' 'fill the package manifests from the checksums on the release'

    $renderScript = Join-Path $here 'render.ps1'
    if (-not $Execute) {
        Write-Note "$renderScript $Ver"
        Write-Note 'It reads the .sha256 files from the release and writes packaging/out/.'
        return 'would do'
    }

    # render.ps1 clears packaging/out/ each time, so a second run is safe and
    # there is nothing to look for first.
    & $renderScript $Ver
    Write-Ok 'The manifests are in packaging/out/.'
    return 'done'
}

# --------------------------------------------------------------------------
# 7 publish
# --------------------------------------------------------------------------

# Waits until crates.io has indexed one version of a crate.
#
# cargo resolves from the index, so peruse-tui cannot build until the index
# holds the new peruse-core. The wait is a poll and not a sleep, because the
# time it takes is not known: it is a few seconds most days.
function Wait-ForIndex {
    param([string] $Crate, [string] $Ver)

    $deadline = (Get-Date).AddMinutes($indexTimeoutMinutes)
    $try = 0
    while ((Get-Date) -lt $deadline) {
        $try++
        if (@(Get-PublishedVersions $Crate $try) -contains $Ver) {
            Write-Ok "crates.io has indexed $Crate $Ver."
            return
        }
        Write-Warn "$(Get-Date -Format 'HH:mm:ss') waiting for $Crate $Ver in the index."
        Start-Sleep -Seconds 10
    }
    throw "crates.io did not index $Crate $Ver inside $indexTimeoutMinutes minutes. peruse-core is published and that cannot be taken back. Wait, then run again with -From publish."
}

function Invoke-Publish {
    param([string] $Ver, [string] $Tag)

    Write-Head 7 'publish' 'peruse-core first, then peruse-tui'

    $coreHas = @(Get-PublishedVersions 'peruse-core') -contains $Ver
    $tuiHas = @(Get-PublishedVersions 'peruse-tui') -contains $Ver

    if ($coreHas -and $tuiHas) {
        Write-Ok "Already done, going on. crates.io holds both crates at $Ver."
        return 'already done'
    }

    if (-not $Execute) {
        if ($coreHas) { Write-Note "peruse-core $Ver is on crates.io already." }
        else { Write-Note "cargo publish -p peruse-core --locked" }
        Write-Note 'then wait for the index'
        if ($tuiHas) { Write-Note "peruse-tui $Ver is on crates.io already." }
        else { Write-Note "cargo publish -p peruse-tui --locked" }
        return 'would do'
    }

    Confirm-Release $Tag @(
        "publish peruse-core $Ver to crates.io",
        "publish peruse-tui $Ver to crates.io",
        "a version on crates.io can be yanked, but never replaced and never freed"
    )

    if ($coreHas) {
        Write-Ok "Already done, going on. peruse-core $Ver is on crates.io."
    }
    else {
        $code = Invoke-Loud 'cargo' @('publish', '-p', 'peruse-core', '--locked')
        if ($code -ne 0) { throw 'cargo publish -p peruse-core failed. Nothing went out. Read the output above.' }
        Write-Ok "Published peruse-core $Ver."
    }

    # peruse-tui asks for peruse-core by version, so it cannot even be
    # packaged until the index holds that version.
    Wait-ForIndex 'peruse-core' $Ver

    if ($tuiHas) {
        Write-Ok "Already done, going on. peruse-tui $Ver is on crates.io."
    }
    else {
        $code = Invoke-Loud 'cargo' @('publish', '-p', 'peruse-tui', '--locked')
        if ($code -ne 0) {
            Write-Fail 'cargo publish -p peruse-tui failed.'
            Write-Note 'peruse-core is published already, so run this again with -From publish when it is fixed.'
            Write-Note 'If the message names the lock file, try the same command without --locked and say so in the release notes.'
            throw 'cargo publish -p peruse-tui failed.'
        }
        Write-Ok "Published peruse-tui $Ver."
    }

    # Both crates are out by now, so a slow index is a thing to know about and
    # not a reason to end the run badly.
    try {
        Wait-ForIndex 'peruse-tui' $Ver
    }
    catch {
        Write-Warn $_.Exception.Message
        Write-Warn 'Both crates are published. This was only the last look at the index.'
    }
    return 'done'
}

# --------------------------------------------------------------------------
# 8 handoff
# --------------------------------------------------------------------------

function Invoke-Handoff {
    param([string] $Ver)

    Write-Head 8 'handoff' 'what is left for a person to do'

    $out = Join-Path $here 'out'
    Write-Note 'Each package tool takes the file in a different place, and none of them'
    Write-Note 'takes it from this repository. packaging/README.md says how, under the'
    Write-Note 'name of each tool.'
    Write-Line ''
    Write-Note "Scoop        $out\scoop\peruse.json         -> the scoop-peruse bucket, in bucket/"
    Write-Note "Homebrew     $out\homebrew\peruse.rb        -> the homebrew-peruse tap, in Formula/"
    Write-Note "WinGet       $out\winget\*.yaml             -> a pull request to microsoft/winget-pkgs"
    Write-Note "Chocolatey   $out\chocolatey\               -> choco pack, then choco push"
    Write-Line ''
    Write-Note 'Do Scoop and Homebrew first. They need no review, so a user can install'
    Write-Note 'Peruse the same day. WinGet and Chocolatey both wait for a person.'
    Write-Line ''
    Write-Note "The release is at https://github.com/$repoName/releases/tag/v$Ver"
    Write-Note "The crates are at https://crates.io/crates/peruse-tui"
    return 'done'
}

# --------------------------------------------------------------------------
# The run
# --------------------------------------------------------------------------

function Show-Help {
    Get-Help -Full $PSCommandPath | Out-String | Write-Host
}

if ($Help) {
    Show-Help
    exit 0
}

if (-not $Version) {
    Write-Host 'usage: .\packaging\release.ps1 <version> [-Execute] [-SkipTests] [-From <step>] [-NoPublish]'
    Write-Host ''
    Write-Host 'for example: .\packaging\release.ps1 0.2.0            look, change nothing'
    Write-Host '             .\packaging\release.ps1 0.2.0 -Execute   do it'
    Write-Host ''
    Write-Host 'Give -Help, or run `Get-Help .\packaging\release.ps1 -Full`, for all of it.'
    exit 2
}

# A tag carries `v`, a version does not. Take either. The name of the
# variable is not $version, because PowerShell would then hold one variable
# for it and for the parameter $Version, and a message about what a person
# typed would show the changed text.
$ver = $Version -replace '^v', ''
if ($ver -notmatch '^\d+\.\d+\.\d+$') {
    Write-Host "The version must be X.Y.Z, and '$Version' is not." -ForegroundColor Red
    Write-Host 'A name such as 0.2.0-rc1 is not supported: the package templates and the'
    Write-Host 'manifests carry a plain version only.'
    exit 2
}
$tag = "v$ver"

$start = 1
if ($From) {
    $i = [array]::IndexOf($stepNames, $From.ToLowerInvariant())
    if ($i -lt 0) {
        Write-Host "There is no step '$From'. The steps are: $($stepNames -join ', ')." -ForegroundColor Red
        exit 2
    }
    $start = $i + 1
}

$plan = [ordered] @{}
for ($i = 0; $i -lt $stepNames.Count; $i++) {
    $name = $stepNames[$i]
    $number = $i + 1
    if ($number -lt $start) { $plan[$name] = 'skip, before -From' }
    elseif ($name -eq 'publish' -and $NoPublish) { $plan[$name] = 'skip, -NoPublish' }
    elseif ($name -eq 'preflight' -and $SkipTests) { $plan[$name] = 'run, quick checks only' }
    else { $plan[$name] = 'run' }
}
$publishPlanned = ($plan['publish'] -eq 'run')

Write-Line ''
Write-Host "Peruse release $ver" -ForegroundColor Cyan -NoNewline
if ($Execute) {
    Write-Host '   FOR REAL' -ForegroundColor Red
}
else {
    Write-Host '   DRY RUN, nothing will change. Give -Execute to do it.' -ForegroundColor Yellow
}
Write-Note "the repository is $root"
Write-Note "the tag will be $tag"
Write-Line ''
Write-Line 'The plan:'
$n = 0
foreach ($name in $stepNames) {
    $n++
    Write-Line ("  {0} {1,-10} {2}" -f $n, $name, $plan[$name])
}

# The dry run must leave the tree as it found it, and the run itself says so
# at the end. `git -C` reads the repository from wherever the person stands.
$before = ''
if (-not $Execute) {
    $before = (Invoke-Read 'git' @('-C', $root, 'status', '--porcelain')).Text
}

$fatal = ''
$current = ''
Push-Location $root
try {
    foreach ($name in $stepNames) {
        $current = $name
        if ($plan[$name] -like 'skip*') {
            $outcome[$name] = 'skipped'
            continue
        }
        switch ($name) {
            'preflight' {
                $outcome[$name] = Invoke-Preflight $ver $tag $publishPlanned
                if ($outcome[$name] -eq 'failed') {
                    Write-Line ''
                    Write-Host "The preflight found $($problems.Count) things:" -ForegroundColor Red
                    foreach ($p in $problems) { Write-Host "  * $p" -ForegroundColor Red }
                    if ($Execute) {
                        throw 'The preflight failed, so nothing was done.'
                    }
                    Write-Line ''
                    Write-Warn 'A real run would stop here. The dry run goes on, to show you the rest.'
                }
            }
            'bump' { $outcome[$name] = Invoke-Bump $ver $tag }
            'tag' { $outcome[$name] = Invoke-Tag $ver $tag }
            'workflow' { $outcome[$name] = Invoke-Workflow $ver $tag }
            'verify' { $outcome[$name] = Invoke-Verify $ver }
            'render' { $outcome[$name] = Invoke-Render $ver }
            'publish' { $outcome[$name] = Invoke-Publish $ver $tag }
            'handoff' { $outcome[$name] = Invoke-Handoff $ver }
        }
    }
}
catch {
    # The summary matters most when a step fails, because it says what is
    # already out in the world and what a second run must still do.
    $fatal = $_.Exception.Message
    if ($outcome[$current] -eq 'not reached') { $outcome[$current] = 'FAILED' }
}
finally {
    Pop-Location
}

Write-Line ''
if ($Execute) {
    Write-Host "Release ${ver}: what was done" -ForegroundColor Cyan
}
else {
    Write-Host "Release ${ver}: what a real run would do" -ForegroundColor Cyan
}
$n = 0
foreach ($name in $stepNames) {
    $n++
    Write-Line ("  {0} {1,-10} {2}" -f $n, $name, $outcome[$name])
}

if (-not $Execute) {
    Write-Line ''
    $after = (Invoke-Read 'git' @('-C', $root, 'status', '--porcelain')).Text
    if ($before -eq $after) {
        Write-Ok 'The dry run changed nothing: git status says the same as before it.'
    }
    else {
        Write-Fail 'The working tree is not as it was. That is a fault in this script.'
        exit 1
    }
}

if ($fatal) {
    Write-Line ''
    Write-Host "Stopped: $fatal" -ForegroundColor Red
    if ($Execute) {
        Write-Host "Take it up again with:  .\packaging\release.ps1 $ver -Execute -From $current -SkipTests" -ForegroundColor Red
    }
    Write-Line ''
    exit 1
}

Write-Line ''
if (-not $Execute) {
    Write-Line "To do it:  .\packaging\release.ps1 $ver -Execute"
    Write-Line ''
}
