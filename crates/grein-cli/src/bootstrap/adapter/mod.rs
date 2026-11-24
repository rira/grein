mod tui_to_engine;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_autocomplete;
#[cfg(test)]
mod tests_hardening;
#[cfg(test)]
mod tests_session_prefs;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use grein_core::error::Result as CoreResult;
use grein_core::port::outbound::PreferenceRepository;
use grein_core::prefs::{
    default_prefs_db_path, ImportReport, SettingsFormat, SettingsPatch, SettingsSnapshot,
    UserPrefsStore,
};

pub use tui_to_engine::{EngineClientPort, InitialSessionMode};

/// Lazy [`PreferenceRepository`] backing [`EngineClientPort`].
///
/// Captures the configured path at composition time (or reads from env when
/// built via [`LazyPrefsRepository::from_env`]) and avoids creating an empty
/// preferences database on disk during read-only loads.
pub struct LazyPrefsRepository {
    fixed_path: Option<Option<PathBuf>>,
    cached: Mutex<Option<(PathBuf, Arc<UserPrefsStore>)>>,
}

impl LazyPrefsRepository {
    pub fn for_path(path: Option<PathBuf>) -> Self {
        Self {
            fixed_path: Some(path),
            cached: Mutex::new(None),
        }
    }

    pub fn from_env() -> Self {
        Self {
            fixed_path: None,
            cached: Mutex::new(None),
        }
    }

    fn resolve_path(&self) -> Option<PathBuf> {
        match &self.fixed_path {
            Some(path) => path.clone(),
            None => default_prefs_db_path(),
        }
    }

    fn open_cached(&self, path: &Path) -> CoreResult<Arc<UserPrefsStore>> {
        let mut guard = self.cached.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached_path, store)) = guard.as_ref() {
            if cached_path == path {
                return Ok(Arc::clone(store));
            }
        }
        let store = Arc::new(UserPrefsStore::open(path)?);
        *guard = Some((path.to_path_buf(), Arc::clone(&store)));
        Ok(store)
    }

    fn store_if_exists(&self) -> CoreResult<Option<Arc<UserPrefsStore>>> {
        let Some(path) = self.resolve_path() else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        self.open_cached(&path).map(Some)
    }

    fn store_for_write(&self) -> CoreResult<Option<Arc<UserPrefsStore>>> {
        let Some(path) = self.resolve_path() else {
            return Ok(None);
        };
        self.open_cached(&path).map(Some)
    }
}

impl PreferenceRepository for LazyPrefsRepository {
    fn get(&self, namespace: &str, key: &str) -> CoreResult<Option<String>> {
        match self.store_if_exists()? {
            Some(store) => store.get(namespace, key),
            None => Ok(None),
        }
    }

    fn set(&self, namespace: &str, key: &str, value: &str) -> CoreResult<()> {
        if let Some(store) = self.store_for_write()? {
            store.set(namespace, key, value)?;
        }
        Ok(())
    }

    fn delete(&self, namespace: &str, key: &str) -> CoreResult<bool> {
        match self.store_for_write()? {
            Some(store) => store.delete(namespace, key),
            None => Ok(false),
        }
    }

    fn list_namespace(&self, namespace: &str) -> CoreResult<BTreeMap<String, String>> {
        match self.store_if_exists()? {
            Some(store) => store.list_namespace(namespace),
            None => Ok(BTreeMap::new()),
        }
    }

    fn keybindings(&self) -> CoreResult<BTreeMap<String, String>> {
        match self.store_if_exists()? {
            Some(store) => store.keybindings(),
            None => Ok(BTreeMap::new()),
        }
    }

    fn set_keybinding(&self, action: &str, binding: &str) -> CoreResult<()> {
        if let Some(store) = self.store_for_write()? {
            store.set_keybinding(action, binding)?;
        }
        Ok(())
    }

    fn remove_keybinding(&self, action: &str) -> CoreResult<bool> {
        match self.store_for_write()? {
            Some(store) => store.remove_keybinding(action),
            None => Ok(false),
        }
    }

    fn clear_keybindings(&self) -> CoreResult<usize> {
        match self.store_for_write()? {
            Some(store) => store.clear_keybindings(),
            None => Ok(0),
        }
    }

    fn theme(&self) -> CoreResult<Option<String>> {
        match self.store_if_exists()? {
            Some(store) => store.theme(),
            None => Ok(None),
        }
    }

    fn set_theme(&self, name: &str) -> CoreResult<()> {
        if let Some(store) = self.store_for_write()? {
            store.set_theme(name)?;
        }
        Ok(())
    }

    fn theme_prefs(&self) -> CoreResult<BTreeMap<String, String>> {
        match self.store_if_exists()? {
            Some(store) => store.theme_prefs(),
            None => Ok(BTreeMap::new()),
        }
    }

    fn set_theme_pref(&self, key: &str, value: &str) -> CoreResult<()> {
        if let Some(store) = self.store_for_write()? {
            store.set_theme_pref(key, value)?;
        }
        Ok(())
    }

    fn clear_theme_prefs(&self) -> CoreResult<usize> {
        match self.store_for_write()? {
            Some(store) => store.clear_theme_prefs(),
            None => Ok(0),
        }
    }

    fn ui_prefs(&self) -> CoreResult<BTreeMap<String, String>> {
        match self.store_if_exists()? {
            Some(store) => store.ui_prefs(),
            None => Ok(BTreeMap::new()),
        }
    }

    fn set_ui_pref(&self, key: &str, value: &str) -> CoreResult<()> {
        if let Some(store) = self.store_for_write()? {
            store.set_ui_pref(key, value)?;
        }
        Ok(())
    }

    fn clear_ui_prefs(&self) -> CoreResult<usize> {
        match self.store_for_write()? {
            Some(store) => store.clear_ui_prefs(),
            None => Ok(0),
        }
    }

    fn export_snapshot(&self, only_sections: Option<&[String]>) -> CoreResult<SettingsSnapshot> {
        match self.store_if_exists()? {
            Some(store) => store.export_snapshot(only_sections),
            None => UserPrefsStore::in_memory()?.export_snapshot(only_sections),
        }
    }

    fn export_string(
        &self,
        format: SettingsFormat,
        only_sections: Option<&[String]>,
    ) -> CoreResult<String> {
        match self.store_if_exists()? {
            Some(store) => store.export_string(format, only_sections),
            None => UserPrefsStore::in_memory()?.export_string(format, only_sections),
        }
    }

    fn import_patch(
        &self,
        patch: &SettingsPatch,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> CoreResult<ImportReport> {
        match self.store_for_write()? {
            Some(store) => store.import_patch(patch, only_sections, replace_all),
            None => Ok(ImportReport::default()),
        }
    }

    fn import_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> CoreResult<ImportReport> {
        match self.store_for_write()? {
            Some(store) => store.import_str(raw, format, only_sections, replace_all),
            None => Ok(ImportReport::default()),
        }
    }

    fn import_theme_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        replace_theme: bool,
    ) -> CoreResult<ImportReport> {
        match self.store_for_write()? {
            Some(store) => store.import_theme_str(raw, format, replace_theme),
            None => Ok(ImportReport::default()),
        }
    }
}

#[cfg(test)]
macro_rules! impl_default_auxiliary_ports {
    ($ty:ty) => {
        #[async_trait::async_trait]
        impl crate::engine::ShellUseCasePort for $ty {
            async fn run_shell_command(
                &self,
                _thread_id: &str,
                _command: &str,
            ) -> Result<crate::engine::ShellCommandResult, crate::engine::EngineError> {
                unreachable!()
            }
        }
        impl crate::engine::TerminalPtyUseCasePort for $ty {}
        impl crate::engine::WorktreeUseCasePort for $ty {}
        impl crate::engine::AutocompleteContextPort for $ty {}
    };
}

#[cfg(test)]
pub(super) use impl_default_auxiliary_ports;
