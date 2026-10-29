//! Transport-independent POSIX process ownership for foreground tool commands.

pub(crate) const PREPARE: &str = r#"
for tool in setsid awk ps mktemp flock sort; do command -v "$tool" >/dev/null || exit 125; done
umask 077
mktemp -d /tmp/android-tool-process.XXXXXXXXXXXX
"#;

pub(crate) const LEADER: &str = r#"
root=$1; shift
exec 4>"$root/start.lock" || exit 125
flock -x 4 || exit 125
start=$(awk '{print $22}' /proc/$$/stat) || exit 125
printf '%s %s\n' "$$" "$start" > "$root/identity.tmp" &&
mv "$root/identity.tmp" "$root/identity" || exit 125
test ! -e "$root/cancel" || exit 125
exec 3<&0
"$@" <&3 3<&- 4>&- &
worker=$!
exec 3<&-
flock -u 4
exec 4>&-
wait "$worker"
code=$?
printf '%s\n' "$code" > "$root/exit.tmp" && mv "$root/exit.tmp" "$root/exit"
kill -KILL "-$$"
exit 125
"#;

pub(crate) const SUPERVISOR: &str = r#"
root=$1; leader=$2; shift 2
test ! -e "$root/cancel" || exit 125
exec 3<&0
setsid sh -c "$leader" android-tool-owned-group "$root" "$@" <&3 3<&- &
launcher=$!
exec 3<&-
wait "$launcher" 2>/dev/null
test -r "$root/exit" || exit 125
read -r code < "$root/exit"
case "$code" in ''|*[!0-9]*) exit 125;; esac
exit "$code"
"#;

pub(crate) const DRAIN: &str = r#"
root=$1
test -d "$root" || exit 125
: > "$root/cancel" || exit 125
exec 4>"$root/start.lock" || exit 125
flock -x 4 || exit 125
if test -r "$root/identity"; then
    read -r pid start < "$root/identity" || exit 125
    case "$pid:$start" in *[!0-9:]*|:*|*:) exit 125;; esac
    test "$pid" -gt 1 || exit 125
    if test -r "/proc/$pid/stat"; then
        actual=$(awk '{print $22}' "/proc/$pid/stat") || exit 125
        group=$(ps -o pgid= -p "$pid") || exit 125
        test "$actual" = "$start" && test "$group" -eq "$pid" || exit 125
        kill -KILL "-$pid" 2>/dev/null || true
    fi
    count=0
    while :; do
        processes=$(ps -eo pgid=,stat=) || exit 125
        if printf '%s\n' "$processes" | awk -v group="$pid" '$1 == group && $2 !~ /^Z/ {live=1} END {exit !live}'; then
            :
        else
            state=$?
            test "$state" -eq 1 || exit 125
            break
        fi
        count=$((count + 1))
        test "$count" -lt 100 || exit 124
        sleep 0.05
    done
fi
"#;

pub(crate) const REMOVE: &str = r#"
root=$1
case "$root" in /tmp/android-tool-process.*) ;; *) exit 125;; esac
test -d "$root" && test ! -L "$root" || exit 125
rm -f -- "$root/cancel" "$root/identity" "$root/identity.tmp" "$root/exit" "$root/exit.tmp" "$root/start.lock" "$root/drained" "$root/cleanup.lock"
rmdir -- "$root"
"#;

pub(crate) fn valid_control_root(root: &str) -> bool {
    root.strip_prefix("/tmp/android-tool-process.")
        .is_some_and(|suffix| {
            suffix.len() == 12 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

pub(crate) fn shell_command(root: &str, executable: &str, args: &[String]) -> String {
    let mut tokens = vec![
        "sh".to_string(),
        "-c".into(),
        SUPERVISOR.into(),
        "android-tool-supervisor".into(),
        root.into(),
        LEADER.into(),
        executable.into(),
    ];
    tokens.extend(args.iter().cloned());
    tokens
        .iter()
        .map(|value| format!("'{}'", value.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

// SSH's PTY child already owns its controlling session. Keep it rather than
// using setsid again, which would sever /dev/tty from the command.
pub(crate) const PTY_LEADER: &str = r#"
root=$1; shift
exec 4>"$root/start.lock" || exit 125
flock -x 4 || exit 125
group=$(ps -o pgid= -p "$$") || exit 125
session=$(ps -o sid= -p "$$") || exit 125
test "$group" -eq "$$" && test "$session" -eq "$$" || exit 125
test -t 0 && test -t 1 && test -r /dev/tty || exit 125
stty rows 40 cols 120 </dev/tty || exit 125
start=$(awk '{print $22}' /proc/$$/stat) || exit 125
printf '%s %s\n' "$$" "$start" > "$root/identity.tmp" &&
mv "$root/identity.tmp" "$root/identity" || exit 125
test ! -e "$root/cancel" || exit 125
exec 3<&0
"$@" <&3 3<&- 4>&- &
worker=$!
exec 3<&-
flock -u 4
exec 4>&-
wait "$worker"
code=$?
printf '%s\n' "$code" > "$root/exit.tmp" && mv "$root/exit.tmp" "$root/exit" || exit 125
while :; do sleep 0.05; done
"#;

pub(crate) const DRAIN_PTY: &str = r#"
root=$1
test -d "$root" || exit 125
: > "$root/cancel" || exit 125
exec 4>"$root/start.lock" || exit 125
flock -x 4 || exit 125
if test -r "$root/identity"; then
    read -r pid start < "$root/identity" || exit 125
    case "$pid:$start" in *[!0-9:]*|:*|*:) exit 125;; esac
    test "$pid" -gt 1 || exit 125
    # A previous cleanup may have killed the leader before its reply arrived.
    # Only accept that case after verifying the entire session has stopped.
    if test ! -r "/proc/$pid/stat"; then
        processes=$(ps -eo sid=,stat=) || exit 125
        if printf '%s\n' "$processes" | awk -v session="$pid" '$1 == session && $2 !~ /^Z/ {live=1} END {exit !live}'; then
            exit 125
        else
            state=$?
            test "$state" -eq 1 || exit 125
        fi
        exit 0
    fi
    actual=$(awk '{print $22}' "/proc/$pid/stat") || exit 125
    session=$(ps -o sid= -p "$pid") || exit 125
    test "$actual" = "$start" && test "$session" -eq "$pid" || exit 125
    # Job control can create other groups in this dedicated terminal session.
    # Keep its identified leader alive until those groups have been signalled.
    processes=$(ps -eo sid=,pgid=,stat=) || exit 125
    groups=$(printf '%s\n' "$processes" | awk -v session="$pid" '$1 == session && $2 != session && $3 !~ /^Z/ {print $2}') || exit 125
    groups=$(printf '%s\n' "$groups" | sort -u) || exit 125
    for group in $groups; do
        case "$group" in ''|*[!0-9]*) exit 125;; esac
        test "$group" -gt 1 || exit 125
        kill -KILL "-$group" 2>/dev/null || true
    done
    kill -KILL "-$pid" 2>/dev/null || true
    count=0
    while :; do
        processes=$(ps -eo sid=,stat=) || exit 125
        if printf '%s\n' "$processes" | awk -v session="$pid" '$1 == session && $2 !~ /^Z/ {live=1} END {exit !live}'; then
            :
        else
            state=$?
            test "$state" -eq 1 || exit 125
            break
        fi
        count=$((count + 1))
        test "$count" -lt 100 || exit 124
        sleep 0.05
    done
fi
"#;

pub(crate) fn pty_command(root: &str, command: &str) -> String {
    let tokens = [
        "sh",
        "-c",
        PTY_LEADER,
        "android-tool-pty-session",
        root,
        "sh",
        "-c",
        command,
    ];
    format!(
        "exec {}",
        tokens
            .iter()
            .map(|value| format!("'{}'", value.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;

    #[test]
    #[ignore = "Requires ANDROID_DSH_TEST_WSL; races a real blocked startup with cancellation and directory removal"]
    fn real_owned_startup_cannot_outlive_cancel_fence_removal() {
        let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
        let script = r#"
prepare=$1; leader=$2; drain=$3; remove=$4
fixture=$(mktemp -d /tmp/android-tool-startup-test.XXXXXXXXXXXX) || exit 125
trap 'rm -f -- "$fixture/started"; rmdir -- "$fixture"' EXIT
root=$(sh -c "$prepare") || exit 125
exec 4>"$root/start.lock" || exit 125
flock -x 4 || exit 125
setsid sh -c "$leader" startup-test "$root" sh -c 'printf started > "$1"' child "$fixture/started" 4>&- &
launcher=$!
sleep 0.1
sh -c "$drain
$remove" startup-cleanup "$root" 4>&- &
cleanup=$!
count=0
while test ! -e "$root/cancel"; do
    count=$((count + 1)); test "$count" -lt 100 || exit 125
    sleep 0.01
done
flock -u 4
exec 4>&-
wait "$cleanup" || exit 125
if wait "$launcher"; then exit 125; fi
test ! -e "$fixture/started" && test ! -d "$root"
"#;
        let output = std::process::Command::new("wsl.exe")
            .creation_flags(0x08000000)
            .args([
                "-d",
                &distribution,
                "--exec",
                "sh",
                "-c",
                script,
                "startup-test",
                PREPARE,
                LEADER,
                DRAIN,
                REMOVE,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "startup cancellation fence failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
