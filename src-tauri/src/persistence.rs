use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::queue::{COMPLETED_JOB_HISTORY_LIMIT, QueueJobStatus, TranscriptionQueue};
use crate::settings::{AppSettings, SettingsPatch};

#[derive(Debug, Clone)]
pub struct AppPersistence {
    root: PathBuf,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct OpenRouterSecrets {
    api_key: String,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct OpenAiSecrets {
    api_key: String,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SonioxSecrets {
    api_key: String,
}

const LIST_VISIBILITY_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListVisibilityKind {
    Meeting,
    Transcript,
    ReportRun,
    LegacyReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListVisibilityTarget {
    pub kind: ListVisibilityKind,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetListVisibilityRequest {
    pub targets: Vec<ListVisibilityTarget>,
    pub hidden: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListVisibilityState {
    pub meetings: BTreeSet<String>,
    pub transcripts: BTreeSet<String>,
    pub report_runs: BTreeSet<String>,
    pub legacy_reports: BTreeSet<String>,
}

impl ListVisibilityState {
    fn ids_mut(&mut self, kind: &ListVisibilityKind) -> &mut BTreeSet<String> {
        match kind {
            ListVisibilityKind::Meeting => &mut self.meetings,
            ListVisibilityKind::Transcript => &mut self.transcripts,
            ListVisibilityKind::ReportRun => &mut self.report_runs,
            ListVisibilityKind::LegacyReport => &mut self.legacy_reports,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ListVisibilityRegistry {
    version: u32,
    roots: BTreeMap<String, ListVisibilityState>,
}

impl Default for ListVisibilityRegistry {
    fn default() -> Self {
        Self {
            version: LIST_VISIBILITY_VERSION,
            roots: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid secret: {0}")]
    InvalidSecret(String),
    #[error("list visibility target id cannot be blank")]
    InvalidListVisibilityTarget,
    #[error("unsupported list visibility version {0}")]
    UnsupportedListVisibilityVersion(u32),
    #[error("app persistence is unavailable")]
    PersistenceUnavailable,
}

impl AppPersistence {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Tighten older installations before any persisted settings or credentials
    /// are loaded. Only this app's own state directory is changed.
    pub fn protect_private_storage(&self) -> Result<(), PersistenceError> {
        ensure_private_directory(&self.root)?;
        for path in [
            self.settings_path(),
            self.queue_path(),
            self.list_visibility_path(),
            self.openai_secrets_path(),
            self.openrouter_secrets_path(),
            self.soniox_secrets_path(),
        ] {
            protect_existing_file(&path)?;
        }
        Ok(())
    }

    pub fn load_settings(&self) -> Result<Option<AppSettings>, PersistenceError> {
        let Some(value) = read_json_if_exists::<serde_json::Value>(&self.settings_path())? else {
            return Ok(None);
        };
        let legacy_permission_onboarding = value.get("permission_onboarding_seen").is_none();
        let legacy_cycle_shortcut = value.get("cycle_microphone_shortcut").is_none();
        let patch = serde_json::from_value::<SettingsPatch>(value)?;
        let mut settings = AppSettings::default();
        if legacy_permission_onboarding {
            settings.permission_onboarding_seen = true;
        }
        settings.apply_patch(patch);
        // A newly introduced default must not reserve a chord already used
        // by an existing modifier-only or keyed Dictation shortcut.
        if legacy_cycle_shortcut && crate::dictation::active_dictation_shortcuts(&settings)
            .iter().any(|(shortcut, _)| crate::dictation::dictation_shortcuts_overlap(
                &settings.cycle_microphone_shortcut, shortcut).unwrap_or(true))
        {
            settings.cycle_microphone_shortcut.clear();
        }
        Ok(Some(settings))
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), PersistenceError> {
        write_json_atomic(&self.settings_path(), settings)
    }

    pub fn load_openrouter_api_key(&self) -> Result<Option<String>, PersistenceError> {
        let Some(secrets) =
            read_json_if_exists::<OpenRouterSecrets>(&self.openrouter_secrets_path())?
        else {
            return Ok(None);
        };
        let api_key = secrets.api_key.trim();
        Ok((!api_key.is_empty()).then(|| api_key.to_string()))
    }

    pub fn save_openrouter_api_key(&self, api_key: &str) -> Result<(), PersistenceError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(PersistenceError::InvalidSecret(
                "OpenRouter API key cannot be blank".to_string(),
            ));
        }
        write_json_atomic(
            &self.openrouter_secrets_path(),
            &OpenRouterSecrets {
                api_key: api_key.to_string(),
            },
        )
    }

    pub fn delete_openrouter_api_key(&self) -> Result<(), PersistenceError> {
        match std::fs::remove_file(self.openrouter_secrets_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn openrouter_api_key_configured(&self) -> Result<bool, PersistenceError> {
        Ok(self.load_openrouter_api_key()?.is_some())
    }

    pub fn load_openai_api_key(&self) -> Result<Option<String>, PersistenceError> {
        let Some(secrets) = read_json_if_exists::<OpenAiSecrets>(&self.openai_secrets_path())?
        else {
            return Ok(None);
        };
        let api_key = secrets.api_key.trim();
        Ok((!api_key.is_empty()).then(|| api_key.to_string()))
    }

    pub fn save_openai_api_key(&self, api_key: &str) -> Result<(), PersistenceError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(PersistenceError::InvalidSecret(
                "OpenAI API key cannot be blank".to_string(),
            ));
        }
        write_json_atomic(
            &self.openai_secrets_path(),
            &OpenAiSecrets {
                api_key: api_key.to_string(),
            },
        )
    }

    pub fn delete_openai_api_key(&self) -> Result<(), PersistenceError> {
        match std::fs::remove_file(self.openai_secrets_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn openai_api_key_configured(&self) -> Result<bool, PersistenceError> {
        Ok(self.load_openai_api_key()?.is_some())
    }

    pub fn load_soniox_api_key(&self) -> Result<Option<String>, PersistenceError> {
        let Some(secrets) = read_json_if_exists::<SonioxSecrets>(&self.soniox_secrets_path())?
        else {
            return Ok(None);
        };
        let api_key = secrets.api_key.trim();
        Ok((!api_key.is_empty()).then(|| api_key.to_string()))
    }

    pub fn save_soniox_api_key(&self, api_key: &str) -> Result<(), PersistenceError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(PersistenceError::InvalidSecret(
                "Soniox API key cannot be blank".to_string(),
            ));
        }
        write_json_atomic(
            &self.soniox_secrets_path(),
            &SonioxSecrets {
                api_key: api_key.to_string(),
            },
        )
    }

    pub fn delete_soniox_api_key(&self) -> Result<(), PersistenceError> {
        match std::fs::remove_file(self.soniox_secrets_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn soniox_api_key_configured(&self) -> Result<bool, PersistenceError> {
        Ok(self.load_soniox_api_key()?.is_some())
    }

    pub fn load_queue(&self) -> Result<Option<TranscriptionQueue>, PersistenceError> {
        let Some(mut queue) = read_json_if_exists::<TranscriptionQueue>(&self.queue_path())? else {
            return Ok(None);
        };
        let original = queue.clone();
        queue.recover_running_as_pending();
        queue.normalize_legacy_issues();
        queue.discard_non_actionable_outcomes();
        queue.hydrate_activity_kinds();
        queue.prune_completed_history(COMPLETED_JOB_HISTORY_LIMIT);
        if queue != original {
            self.save_queue(&queue)?;
        }
        Ok(Some(queue))
    }

    pub fn save_queue(&self, queue: &TranscriptionQueue) -> Result<(), PersistenceError> {
        let mut persisted = queue.clone();
        persisted.discard_non_actionable_outcomes();
        persisted.prune_completed_history(COMPLETED_JOB_HISTORY_LIMIT);
        write_json_atomic(&self.queue_path(), &persisted)
    }

    pub fn load_list_visibility(
        &self,
        save_root: &Path,
    ) -> Result<ListVisibilityState, PersistenceError> {
        let registry = self.load_list_visibility_registry()?;
        let key = normalize_save_root(save_root)?;
        Ok(registry.roots.get(&key).cloned().unwrap_or_default())
    }

    pub fn set_list_visibility(
        &self,
        save_root: &Path,
        request: &SetListVisibilityRequest,
    ) -> Result<ListVisibilityState, PersistenceError> {
        if request
            .targets
            .iter()
            .any(|target| target.id.trim().is_empty())
        {
            return Err(PersistenceError::InvalidListVisibilityTarget);
        }

        let key = normalize_save_root(save_root)?;
        let mut registry = self.load_list_visibility_registry()?;
        let state = registry.roots.entry(key).or_default();
        for target in &request.targets {
            let ids = state.ids_mut(&target.kind);
            if request.hidden {
                ids.insert(target.id.clone());
            } else {
                ids.remove(&target.id);
            }
        }
        let state = state.clone();
        write_json_atomic(&self.list_visibility_path(), &registry)?;
        Ok(state)
    }

    fn load_list_visibility_registry(&self) -> Result<ListVisibilityRegistry, PersistenceError> {
        let Some(registry) =
            read_json_if_exists::<ListVisibilityRegistry>(&self.list_visibility_path())?
        else {
            return Ok(ListVisibilityRegistry::default());
        };
        if registry.version != LIST_VISIBILITY_VERSION {
            return Err(PersistenceError::UnsupportedListVisibilityVersion(
                registry.version,
            ));
        }
        Ok(registry)
    }

    fn settings_path(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    fn queue_path(&self) -> PathBuf {
        self.root.join("transcription-queue.json")
    }

    fn openrouter_secrets_path(&self) -> PathBuf {
        self.root.join("openrouter-secrets.json")
    }

    fn openai_secrets_path(&self) -> PathBuf {
        self.root.join("openai-secrets.json")
    }

    fn soniox_secrets_path(&self) -> PathBuf {
        self.root.join("soniox-secrets.json")
    }

    fn list_visibility_path(&self) -> PathBuf {
        self.root.join("list-visibility.json")
    }
}

fn normalize_save_root(path: &Path) -> Result<String, PersistenceError> {
    let expanded = expand_leading_tilde(path);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()?.join(expanded)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    Ok(normalized.to_string_lossy().into_owned())
}

fn expand_leading_tilde(path: &Path) -> PathBuf {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return path.to_path_buf();
    };
    if path == Path::new("~") {
        return home;
    }
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    home.join(rest)
}

fn read_json_if_exists<T: serde::de::DeserializeOwned>(
    path: &Path,
) -> Result<Option<T>, PersistenceError> {
    if !protect_existing_file(path)? {
        return Ok(None);
    }

    let bytes = std::fs::read(path)?;
    let value = serde_json::from_slice(&bytes)?;
    Ok(Some(value))
}

fn ensure_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    if !std::fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "app state directory must not be a symbolic link",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn protect_existing_file(path: &Path) -> std::io::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "app state must be a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "app state must not be a hard link",
            ));
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(true)
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), PersistenceError> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "app state needs a parent directory",
        )
    })?;
    ensure_private_directory(parent)?;
    // NamedTempFile creates an exclusive, unpredictable 0600 file on Unix.
    // A pre-existing <name>.tmp symlink cannot redirect or expose the write.
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec_pretty(value)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

impl TranscriptionQueue {
    pub fn recover_running_as_pending(&mut self) {
        for job in self.jobs_mut() {
            if job.status == QueueJobStatus::Running {
                job.status = QueueJobStatus::Pending;
                job.error = Some("recovered from interrupted app session".to_string());
            }
        }
    }
}
