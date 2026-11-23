use std::{env, fs, path::PathBuf};
use zed_extension_api::{self as zed, Result, serde_json::Value, settings::LspSettings};

/// GitHub repository whose releases carry the `lean-infoview-proxy` binaries.
const PROXY_REPO: &str = "reuixiy7/zed-lean4";
const PROXY_NAME: &str = "lean-infoview-proxy";
/// The proxy release matching this extension, tagged `v<version>`.
const PROXY_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
const DEFAULT_INFOVIEW_FILE: &str = ".lake/infoview.md";

/// Proxy flags turned on by setting these to `false`.
const PROXY_FLAGS: [(&str, &str); 4] = [
    ("infoview_hover", "--no-hover"),
    ("infoview_auto_open", "--no-auto-open"),
    ("abbreviations", "--no-abbreviations"),
    ("declaration_status", "--no-status"),
];

fn setting<'a>(settings: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    settings.and_then(|settings| settings.get(key))
}

fn bool_setting(settings: Option<&Value>, key: &str, default: bool) -> bool {
    setting(settings, key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn path_setting(settings: Option<&Value>, key: &str, worktree: &zed::Worktree) -> Option<PathBuf> {
    let path = PathBuf::from(setting(settings, key)?.as_str()?);
    Some(if path.is_absolute() {
        path
    } else {
        PathBuf::from(worktree.root_path()).join(path)
    })
}

fn path_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn command(command: String, args: Vec<String>) -> zed::Command {
    zed::Command {
        command,
        args,
        env: vec![],
    }
}

/// `name`, with `.exe` on Windows.
fn executable(name: &str) -> String {
    match zed::current_platform().0 {
        zed::Os::Windows => format!("{name}.exe"),
        zed::Os::Mac | zed::Os::Linux => name.to_owned(),
    }
}

/// Absolute path of a file in the extension's working directory.
fn work_path(relative: &str) -> Result<String> {
    let cwd = env::current_dir().map_err(|e| format!("Failed to get current directory: {e}"))?;
    Ok(path_string(cwd.join(relative)))
}

/// The architecture as release assets name it.
fn arch_name() -> Result<&'static str> {
    match zed::current_platform().1 {
        zed::Architecture::Aarch64 => Ok("aarch64"),
        zed::Architecture::X8664 => Ok("x86_64"),
        zed::Architecture::X86 => Err("32-bit x86 is not supported".into()),
    }
}

/// Archive extension and type of release assets on this platform.
fn archive() -> (&'static str, zed::DownloadedFileType) {
    match zed::current_platform().0 {
        zed::Os::Windows => ("zip", zed::DownloadedFileType::Zip),
        zed::Os::Mac | zed::Os::Linux => ("tar.gz", zed::DownloadedFileType::GzipTar),
    }
}

/// Downloads the asset `asset_name` of `release` and unpacks it into `dir`.
fn download_asset(
    language_server_id: &zed::LanguageServerId,
    release: &zed::GithubRelease,
    asset_name: &str,
    dir: &str,
) -> Result<()> {
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| format!("No asset found matching: {asset_name:?}"))?;
    zed::set_language_server_installation_status(
        language_server_id,
        &zed::LanguageServerInstallationStatus::Downloading,
    );
    zed::download_file(&asset.download_url, dir, archive().1)
        .map_err(|e| format!("Failed to download {asset_name}: {e}"))
}

fn latest_release(repo: &str) -> Result<zed::GithubRelease> {
    zed::latest_github_release(
        repo,
        zed::GithubReleaseOptions {
            require_assets: true,
            pre_release: false,
        },
    )
}

/// The Lean server command: `binary` setting, `lake` in PATH or elan, or installs elan.
fn lake_command(
    language_server_id: &zed::LanguageServerId,
    worktree: &zed::Worktree,
    binary: Option<zed::settings::CommandSettings>,
    settings: Option<&Value>,
) -> Result<zed::Command> {
    // Check lsp.lean4-lsp.binary
    if let Some(binary) = binary
        && let Some(path) = binary.path
    {
        return Ok(command(path, binary.arguments.unwrap_or_default()));
    }

    let mut lake_args = Vec::new();
    if let Some(project_dir) = path_setting(settings, "lean_project_dir", worktree) {
        lake_args.extend(["--dir".into(), path_string(project_dir)]);
    }
    lake_args.extend(["serve".into(), "--".into()]);

    // Check $PATH
    if let Some(path) = worktree.which("lake") {
        return Ok(command(path, lake_args));
    }

    // Check $ELAN_HOME or default directory
    let shell_env = worktree.shell_env();
    let env_var = |names: &[&str]| {
        shell_env
            .iter()
            .find_map(|(k, v)| names.contains(&k.as_str()).then(|| PathBuf::from(v)))
    };
    let elan_home = env_var(&["ELAN_HOME"])
        .or_else(|| Some(env_var(&["HOME", "USERPROFILE"])?.join(".elan")))
        .ok_or("Failed to find ELAN_HOME, HOME, or USERPROFILE")?;
    let lake_path = path_string(elan_home.join("bin").join(executable("lake")));
    if worktree.which(&lake_path).is_some() {
        return Ok(command(lake_path, lake_args));
    }

    // Check lsp.lean4-lsp.settings
    if !bool_setting(settings, "elan_auto_install", false) {
        return Err(
            "Failed to find lake in PATH or ELAN_HOME/default path. Enable lsp.lean4-lsp.settings.elan_auto_install or configure lsp.lean4-lsp.binary.path."
                .into(),
        );
    }
    let toolchain = setting(settings, "elan_default_toolchain")
        .and_then(Value::as_str)
        .unwrap_or("stable");

    // Install elan and lean 4 toolchain
    let release = latest_release("leanprover/elan")?;
    let target = match zed::current_platform().0 {
        zed::Os::Windows => "pc-windows-msvc",
        zed::Os::Mac => "apple-darwin",
        zed::Os::Linux => "unknown-linux-gnu",
    };
    let asset_name = format!("elan-{}-{target}.{}", arch_name()?, archive().0);
    let version_dir = format!("elan-{}", release.version);
    download_asset(language_server_id, &release, &asset_name, &version_dir)?;

    let elan_init = work_path(&format!("{version_dir}/{}", executable("elan-init")))?;
    zed::make_file_executable(&elan_init)?;
    zed::Command::new(&elan_init)
        .args([
            "-y",
            "--default-toolchain",
            &format!("leanprover/lean4:{toolchain}"),
        ])
        .output()?;
    fs::remove_dir_all(&version_dir).ok();

    Ok(command(lake_path, lake_args))
}

/// Names of downloaded proxy version directories in the working directory.
fn proxy_dirs() -> Vec<String> {
    let prefix = format!("{PROXY_NAME}-");
    let Ok(entries) = fs::read_dir(".") else {
        return Vec::new();
    };
    let mut dirs: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix))
        .collect();
    dirs.sort();
    dirs
}

fn proxy_binary(dir: &str) -> String {
    format!("{dir}/{}", executable(PROXY_NAME))
}

/// The proxy matching this extension; downloaded only if it is not there yet.
fn download_proxy(language_server_id: &zed::LanguageServerId) -> Result<String> {
    let wanted = format!("{PROXY_NAME}-{PROXY_VERSION}");
    if fs::metadata(proxy_binary(&wanted)).is_ok() {
        return work_path(&proxy_binary(&wanted));
    }

    let release = zed::github_release_by_tag_name(PROXY_REPO, PROXY_VERSION)
        .or_else(|_| latest_release(PROXY_REPO))?;
    let os = match zed::current_platform().0 {
        zed::Os::Windows => "windows",
        zed::Os::Mac => "macos",
        zed::Os::Linux => "linux",
    };
    let asset_name = format!("{PROXY_NAME}-{}-{os}.{}", arch_name()?, archive().0);
    let version_dir = format!("{PROXY_NAME}-{}", release.version);
    let binary = proxy_binary(&version_dir);
    if fs::metadata(&binary).is_err() {
        download_asset(language_server_id, &release, &asset_name, &version_dir)?;
        zed::make_file_executable(&binary)?;
    }
    for dir in proxy_dirs() {
        if dir != version_dir {
            fs::remove_dir_all(dir).ok();
        }
    }
    work_path(&binary)
}

/// The newest previously downloaded proxy, for when downloading fails (e.g. offline).
fn installed_proxy() -> Option<String> {
    let binary = proxy_binary(proxy_dirs().last()?);
    fs::metadata(&binary).ok()?;
    work_path(&binary).ok()
}

#[derive(Default)]
struct Lean4Extension {
    cached_proxy_path: Option<String>,
}

impl Lean4Extension {
    /// The infoview proxy: `infoview_proxy_path` setting, or the released binary.
    fn proxy_path(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        settings: Option<&Value>,
        worktree: &zed::Worktree,
    ) -> Option<String> {
        if let Some(path) = path_setting(settings, "infoview_proxy_path", worktree) {
            return Some(path_string(path));
        }
        if let Some(path) = &self.cached_proxy_path
            && fs::metadata(path).is_ok()
        {
            return Some(path.clone());
        }
        let path = download_proxy(language_server_id)
            .ok()
            .or_else(installed_proxy)?;
        self.cached_proxy_path = Some(path.clone());
        Some(path)
    }
}

impl zed::Extension for Lean4Extension {
    fn new() -> Self {
        Self::default()
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let LspSettings {
            binary, settings, ..
        } = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        let settings = settings.as_ref();

        let lake = lake_command(language_server_id, worktree, binary, settings)?;

        if !bool_setting(settings, "infoview", true) {
            return Ok(lake);
        }
        let Some(proxy) = self.proxy_path(language_server_id, settings, worktree) else {
            return Ok(lake);
        };

        let goals_file = path_setting(settings, "infoview_file", worktree).unwrap_or_else(|| {
            path_setting(settings, "lean_project_dir", worktree)
                .unwrap_or_else(|| PathBuf::from(worktree.root_path()))
                .join(DEFAULT_INFOVIEW_FILE)
        });
        let mut args = vec!["--goals-file".into(), path_string(goals_file)];
        for (key, flag) in PROXY_FLAGS {
            if !bool_setting(settings, key, true) {
                args.push(flag.into());
            }
        }
        args.push("--".into());
        args.push(lake.command);
        args.extend(lake.args);

        Ok(zed::Command {
            command: proxy,
            args,
            env: lake.env,
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<Value>> {
        LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .map(|lsp_settings| lsp_settings.initialization_options)
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<Value>> {
        LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .map(|lsp_settings| lsp_settings.settings)
    }
}

zed::register_extension!(Lean4Extension);
