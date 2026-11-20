//! The process groups of a run's agents, recorded in its run directory, so that a later orchestrator of the run can
//! end a group its predecessor left behind (it was SIGKILLed, or its session ended) before it starts the role's agent
//! again, and no orphan writes beside its replacement.
//!
//! A record, `<role>.acp.group`, is `{"pgid", "started"}`: the group's id, its leader's pid, and the leader's start
//! time as start_time gives it. Whether a group is still the recorded one is told by the group, not by its leader:
//! PR_SET_PDEATHSIG kills the leader and leaves its children, as claude's adapter's own claude. While any member
//! lives the kernel gives no new process the pgid as its pid, so a group with members and no process whose pid is
//! the pgid is the recorded one; a group whose leader exists is the recorded one only when the leader's start time
//! matches the record.

use nix::errno::Errno;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};

use crate::error::OrchestratorError;

/// The file name of a group record, after the role's name.
pub(super) const GROUP_SUFFIX: &str = ".acp.group";

/// Record at path the group that pid leads, as an agent just spawned in a group of its own leads it. Errors: the
/// leader's start time cannot be told (it has exited already), or the record cannot be written.
pub(super) fn record(path: &str, pid: u32) -> Result<(), OrchestratorError> {
    let pgid = i32::try_from(pid).map_err(|_| OrchestratorError(format!("pid {pid} is out of range")))?;
    let started = start_time(pgid)
        .ok_or_else(|| OrchestratorError(format!("cannot tell when process {pid} started; it has exited")))?;
    std::fs::write(path, format!("{}\n", json!({"pgid": pgid, "started": started})))
        .map_err(|e| OrchestratorError(format!("cannot write {path}: {e}")))
}

/// The group recorded at path, when it still has members and is the recorded one, as the module's comment tells it:
/// its pgid. None for no record at path, a group with no member left, and another user's or another leader's group.
/// Errors: the record cannot be read or is not one record wrote.
fn recorded_group(path: &str) -> Result<Option<i32>, OrchestratorError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(OrchestratorError(format!("cannot read {path}: {e}"))),
    };
    let saved: Option<(i32, String)> = serde_json::from_str::<Value>(&text).ok().and_then(|record| {
        let pgid = i32::try_from(record.get("pgid")?.as_i64()?).ok().filter(|p| *p > 1)?;
        Some((pgid, record.get("started")?.as_str()?.to_string()))
    });
    let Some((pgid, started)) = saved else {
        return Err(OrchestratorError(format!("{path} is not a process group record")));
    };
    let ours = match kill(Pid::from_raw(-pgid), None) {
        // ESRCH: no member left; EPERM: another user's group.
        Err(_) => false,
        Ok(()) => match kill(Pid::from_raw(pgid), None) {
            Err(Errno::ESRCH) => true,
            _ => start_time(pgid).is_some_and(|t| t == started),
        },
    };
    Ok(ours.then_some(pgid))
}

/// Kill the group recorded at path (SIGKILL to every member) when recorded_group finds it. Returns whether it was
/// killed. Errors: recorded_group's; the group cannot be signalled.
pub(super) fn end_leftover(path: &str) -> Result<bool, OrchestratorError> {
    let Some(pgid) = recorded_group(path)? else { return Ok(false) };
    match kill(Pid::from_raw(-pgid), Signal::SIGKILL) {
        // Its last member exited since the look above.
        Ok(()) | Err(Errno::ESRCH) => Ok(true),
        Err(e) => Err(OrchestratorError(format!("cannot kill process group {pgid}: {e}"))),
    }
}

/// The roles whose group is recorded in the run directory dir and still alive, as recorded_group finds each, sorted:
/// what end_leftovers would end. Errors: dir cannot be listed, or recorded_group's for a record.
pub fn leftovers(dir: &str) -> Result<Vec<String>, OrchestratorError> {
    let mut alive = vec![];
    for (role, path) in records(dir)? {
        if recorded_group(&path)?.is_some() {
            alive.push(role);
        }
    }
    Ok(alive)
}

/// End every leftover group recorded in the run directory dir, as end_leftover ends each: what a resume of the run
/// does before it starts any agent. Returns the roles whose group was killed, sorted. Errors: dir cannot be listed,
/// or end_leftover's for a record.
pub fn end_leftovers(dir: &str) -> Result<Vec<String>, OrchestratorError> {
    let mut ended = vec![];
    for (role, path) in records(dir)? {
        if end_leftover(&path)? {
            ended.push(role);
        }
    }
    Ok(ended)
}

/// Each group record in the run directory dir: its role and path, sorted by role. Errors: dir cannot be listed.
fn records(dir: &str) -> Result<Vec<(String, String)>, OrchestratorError> {
    let entries = std::fs::read_dir(dir).map_err(|e| OrchestratorError(format!("cannot list {dir}: {e}")))?;
    let mut records = vec![];
    for entry in entries {
        let entry = entry.map_err(|e| OrchestratorError(format!("cannot list {dir}: {e}")))?;
        let name = entry.file_name();
        let Some(role) = name.to_str().and_then(|n| n.strip_suffix(GROUP_SUFFIX)) else { continue };
        records.push((role.to_string(), format!("{dir}/{}", name.to_string_lossy())));
    }
    records.sort();
    Ok(records)
}

/// When process pid started, as an opaque text that is equal for the same process and differs for another one that
/// later takes its pid: on Linux, /proc's start time in clock ticks since boot (proc(5), stat's field 22); elsewhere,
/// ps's `lstart`, to the second. None when the process does not exist or its start cannot be read.
fn start_time(pid: i32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name, field 2, is in parentheses and may hold spaces and parentheses itself; field 3 starts
        // after the last ')'.
        let (_, rest) = stat.rsplit_once(')')?;
        rest.split_whitespace().nth(22 - 3).map(String::from)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = std::process::Command::new("ps")
            .env("LC_ALL", "C")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .output();
        let out = out.ok().filter(|o| o.status.success())?;
        let started = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (!started.is_empty()).then_some(started)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    use super::*;
    use crate::test_support::{eventually, TempDir};

    /// sh -c script, in a group of its own, which it leads.
    fn group(script: &str) -> Child {
        Command::new("/bin/sh").args(["-c", script]).process_group(0).spawn().expect("sh starts")
    }

    /// Whether the group pgid has a member.
    fn alive(pgid: u32) -> bool {
        kill(Pid::from_raw(-i32::try_from(pgid).unwrap()), None).is_ok()
    }

    #[test]
    fn a_group_whose_leader_started_when_recorded_is_killed() {
        let tmp = TempDir::new();
        let path = tmp.join("builder.acp.group");
        let mut leader = group("sleep 30 & sleep 30");
        record(&path, leader.id()).expect("recorded");

        let killed = end_leftover(&path).expect("ended");

        assert!(killed);
        leader.wait().expect("reaped");
        assert!(eventually(|| !alive(leader.id())), "the leader's sleep outlived it");
    }

    #[test]
    fn a_group_whose_leader_started_at_another_time_is_left_alone() {
        let tmp = TempDir::new();
        let path = tmp.join("builder.acp.group");
        let mut leader = group("sleep 30");
        std::fs::write(&path, format!(r#"{{"pgid":{},"started":"another"}}"#, leader.id())).unwrap();

        let killed = end_leftover(&path).expect("looked");

        assert!(!killed);
        assert!(alive(leader.id()));
        // SAFETY of the test: killing what it started.
        kill(Pid::from_raw(-i32::try_from(leader.id()).unwrap()), Signal::SIGKILL).expect("killed");
        leader.wait().expect("reaped");
    }

    #[test]
    fn a_group_whose_leader_is_gone_and_members_live_is_killed_and_one_with_none_is_not() {
        let tmp = TempDir::new();
        let path = tmp.join("builder.acp.group");
        let pid_file = tmp.join("member");
        // The leader writes its member's pid, then exits, leaving the member in its group.
        let mut leader = Command::new("/bin/sh")
            .args(["-c", "sleep 30 & echo $! > \"$1\"", "sh", &pid_file])
            .process_group(0)
            .spawn()
            .expect("sh starts");
        let pgid = leader.id();
        std::fs::write(&path, format!(r#"{{"pgid":{pgid},"started":"long ago"}}"#)).unwrap();
        leader.wait().expect("the leader exited and is reaped");
        assert!(eventually(|| std::fs::read_to_string(&pid_file).is_ok_and(|t| t.ends_with('\n'))));
        assert!(alive(pgid), "the member lives on");

        let killed = end_leftover(&path).expect("ended");
        assert!(eventually(|| !alive(pgid)), "the member outlived the kill");
        let again = end_leftover(&path).expect("looked");
        let none = end_leftover(&tmp.join("reviewer.acp.group")).expect("no record");

        assert_eq!((killed, again, none), (true, false, false));
    }

    #[test]
    fn leftovers_names_the_live_groups_and_kills_none() {
        let tmp = TempDir::new();
        let mut leader = group("sleep 30");
        record(&tmp.join("builder.acp.group"), leader.id()).expect("recorded");
        std::fs::write(tmp.join("reviewer.acp.group"), r#"{"pgid":2147483646,"started":"gone"}"#).unwrap();

        let alive_roles = leftovers(&tmp.path()).expect("looked");

        assert_eq!(alive_roles, ["builder"]);
        assert!(alive(leader.id()), "leftovers killed it");
        assert_eq!(end_leftovers(&tmp.path()).expect("ended"), ["builder"]);
        leader.wait().expect("reaped");
    }

    #[test]
    fn end_leftovers_ends_each_role_s_group_and_refuses_a_record_it_did_not_write() {
        let tmp = TempDir::new();
        let mut leader = group("sleep 30");
        record(&tmp.join("builder.acp.group"), leader.id()).expect("recorded");
        std::fs::write(tmp.join("notes.txt"), "not a record").unwrap();

        let ended = end_leftovers(&tmp.path()).expect("ended");
        leader.wait().expect("reaped");
        std::fs::write(tmp.join("reviewer.acp.group"), "{}").unwrap();
        let refused = end_leftovers(&tmp.path()).expect_err("a record it did not write");

        assert_eq!(ended, ["builder"]);
        assert!(refused.0.ends_with("reviewer.acp.group is not a process group record"), "{}", refused.0);
    }
}
