mod support;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
use support::Directory;

fn invoke(home: Option<&Path>, cwd: &Path, directory: Option<&Path>, args: &[&str]) -> Output {
    static LAUNCH: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _launch = LAUNCH.lock().unwrap_or_else(|e| e.into_inner());
    let mut command = Command::new(env!("CARGO_BIN_EXE_ting"));
    command.current_dir(cwd);
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    if let Some(home) = home {
        command.env(variable, home);
    } else {
        command.env_remove(variable);
    }
    if let Some(directory) = directory {
        command.arg("--data-dir").arg(directory);
    }
    // Results as JSON, which the tests read.
    command.arg("--json").args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.output().unwrap()
}
fn success(home: &Path, cwd: &Path, directory: Option<&Path>, args: &[&str]) -> Value {
    let output = invoke(Some(home), cwd, directory, args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
struct Background {
    home: PathBuf,
    data: PathBuf,
}
impl Drop for Background {
    fn drop(&mut self) {
        if self.data.join("runtime.json").exists() {
            let _ = invoke(Some(&self.home), &self.home, Some(&self.data), &["stop"]);
        }
    }
}

#[test]
fn default_node_is_the_same_from_different_working_directories() {
    let root = Directory::new();
    let home = root.0.join("user home with spaces");
    let first = root.0.join("first working directory");
    let second = root.0.join("second working directory");
    for path in [&home, &first, &second] {
        std::fs::create_dir_all(path).unwrap();
    }
    let init = [
        "init",
        "--dev",
        "--name",
        "home node",
        "--control",
        "127.0.0.1:0",
        "--p2p",
        "127.0.0.1:0",
        "--api-bind",
        "127.0.0.1:0",
    ];
    let initialized = success(&home, &first, None, &init);
    let data = home.join(".ting");
    assert_eq!(
        PathBuf::from(initialized["initialized"].as_str().unwrap()),
        data
    );
    assert!(data.join("config.json").is_file());
    let _default_guard = Background {
        home: home.clone(),
        data: data.clone(),
    };
    success(&home, &first, None, &["start"]);
    assert_eq!(
        success(&home, &second, None, &["status"])["name"],
        "home node"
    );
    // A development chain starts with its seed; the block mined follows it.
    let seed = ting_node::seed::ids(&ting_node::config::development_genesis()).len();
    assert_eq!(success(&home, &first, None, &["status"])["height"], seed);
    success(&home, &second, None, &["mine"]);
    assert_eq!(
        success(&home, &first, None, &["status"])["height"],
        seed + 1
    );
    success(&home, &second, None, &["stop"]);
    assert!(!data.join("runtime.json").exists());
    assert!(!first.join(".ting").exists());
    assert!(!second.join(".ting").exists());

    // Explicit relative overrides still select a caller-relative, independent node.
    let relative = Path::new("secondary-node");
    success(&home, &first, Some(relative), &init);
    let custom = first.join(relative);
    let _custom_guard = Background {
        home: home.clone(),
        data: custom.clone(),
    };
    success(&home, &first, Some(relative), &["start"]);
    assert_eq!(
        success(&home, &second, Some(&custom), &["status"])["height"],
        seed
    );
    success(&home, &second, Some(&custom), &["stop"]);
    let stopped = invoke(Some(&home), &second, None, &["status"]);
    assert!(!stopped.status.success());
    assert!(String::from_utf8_lossy(&stopped.stderr).contains("node is not running"));
}

#[test]
fn missing_initialization_and_home_have_actionable_errors() {
    let root = Directory::new();
    let missing = invoke(Some(&root.0), &root.0, None, &["start"]);
    assert!(!missing.status.success());
    let error = String::from_utf8_lossy(&missing.stderr);
    assert!(error.contains("node is not initialized"));
    assert!(error.contains("init"));
    assert!(error.contains(&root.0.join(".ting").display().to_string()));
    assert!(
        !root.0.join(".ting").exists(),
        "start must not create a stray directory"
    );
    let no_home = invoke(None, &root.0, None, &["start"]);
    assert!(!no_home.status.success());
    assert!(String::from_utf8_lossy(&no_home.stderr).contains("use --data-dir"));
    let relative_home = invoke(Some(Path::new("relative-home")), &root.0, None, &["start"]);
    assert!(!relative_home.status.success());
    assert!(String::from_utf8_lossy(&relative_home.stderr).contains("absolute path"));
    // Plain init creates a production node: the built-in genesis, and a peer
    // listener others can reach. Before the launch time is set it refuses and
    // names the alternatives, and creates nothing.
    let explicit = root.0.join("explicit-node");
    let production = invoke(None, &root.0, Some(&explicit), &["init"]);
    match ting_node::config::production_genesis() {
        Some(expected) => {
            assert!(production.status.success());
            assert_eq!(ting_node::config::genesis(&explicit).unwrap(), expected);
            let config = ting_node::config::Config::load(&explicit).unwrap();
            assert_eq!(config.p2p.to_string(), "0.0.0.0:18442");
        }
        None => {
            assert!(!production.status.success());
            let error = String::from_utf8_lossy(&production.stderr);
            assert!(error.contains("has not launched yet"), "{error}");
            assert!(error.contains("--dev"), "{error}");
            assert!(!explicit.join("genesis.bin").exists());
        }
    }
    let development = root.0.join("development-node");
    let initialized = invoke(None, &root.0, Some(&development), &["init", "--dev"]);
    assert!(initialized.status.success());
    assert_eq!(
        ting_node::config::Config::load(&development)
            .unwrap()
            .p2p
            .to_string(),
        "127.0.0.1:18442"
    );
}
