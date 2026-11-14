use super::*;

fn from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |name| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

#[test]
fn path_follows_xdg_config_home() {
    let env = from(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")]);
    let config = ConfigDir::for_env(&env, &|| None);
    assert_eq!(config.path(), "/xdg/ai-agents-orchestrator");
    assert_eq!(config.setup_checked_path(), "/xdg/ai-agents-orchestrator/setup-checked");
    let env = from(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/u/")]);
    let config = ConfigDir::for_env(&env, &|| None);
    assert_eq!(config.path(), "/home/u/.config/ai-agents-orchestrator");
    assert_eq!(config.prompts_dir(), "/home/u/.config/ai-agents-orchestrator/prompts");
    assert_eq!(config.workflows_dir(), "/home/u/.config/ai-agents-orchestrator/workflows");
    assert_eq!(config.setup_checked_path(), "/home/u/.config/ai-agents-orchestrator/setup-checked");
    assert_eq!(ConfigDir::for_env(&from(&[]), &|| None).path(), "~/.config/ai-agents-orchestrator");
    // Without HOME, the user's home from /etc/passwd, as os.path.expanduser takes it.
    assert_eq!(ConfigDir::for_env(&from(&[]), &|| Some("/home/p".into())).path(), "/home/p/.config/ai-agents-orchestrator");
    let passwd = "root:x:0:0:root:/root:/bin/sh\nbad line\nu:x:1001:1001::/home/u:/bin/sh\n";
    assert_eq!(home_of(passwd, "1001").as_deref(), Some("/home/u"));
    assert_eq!(home_of(passwd, "7"), None);
    assert!(passwd_home().is_some_and(|home| home.starts_with('/')));
    assert_eq!(ConfigDir::at("/").prompts_dir(), "/prompts");
    assert!(ConfigDir::from_env().path().ends_with("/ai-agents-orchestrator"));
}

#[test]
fn child_processes_get_no_pathspec_switches() {
    let vars = GIT_PATHSPEC_ENV
        .iter()
        .map(|k| (*k, "1"))
        .chain([("KEEP_ME", "1"), ("PATH", "/usr/bin")])
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
    let kept = without_withheld(vars);
    assert_eq!(kept, [("KEEP_ME".into(), "1".into()), ("PATH".into(), "/usr/bin".into())]);
    assert!(child_env().iter().all(|(k, _)| !GIT_PATHSPEC_ENV.iter().any(|c| k == c)));
}
