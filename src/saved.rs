use std::{
    fs,
    io::ErrorKind,
    ops::Deref,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context;
use serde::{de::DeserializeOwned, Serialize};

const SAVE_INTERVAL: Duration = Duration::from_secs(2);

enum OnDisk {
    Current,
    Behind,
}

/// Editing marks the value for the next flush; reads cannot mark it dirty.
pub struct Saved<T> {
    value: T,
    path: PathBuf,
    on_disk: OnDisk,
    last_write: Instant,
}

impl<T: Default + DeserializeOwned> Saved<T> {
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let value = match fs::read_to_string(&path) {
            Ok(text) => ron::from_str(&text)
                .with_context(|| format!("Invalid settings in {}", path.display()))?,
            Err(error) if error.kind() == ErrorKind::NotFound => T::default(),
            Err(error) => {
                return Err(error).with_context(|| format!("Could not read {}", path.display()))
            }
        };
        Ok(Self {
            value,
            path,
            on_disk: OnDisk::Current,
            last_write: Instant::now(),
        })
    }
}

impl<T> Saved<T> {
    pub fn edit(&mut self) -> &mut T {
        self.on_disk = OnDisk::Behind;
        &mut self.value
    }
}

impl<T: Serialize> Saved<T> {
    pub fn flush_if_due(&mut self, now: Instant) -> anyhow::Result<()> {
        if matches!(self.on_disk, OnDisk::Behind)
            && now.duration_since(self.last_write) >= SAVE_INTERVAL
        {
            self.flush()?;
        }
        Ok(())
    }

    /// The rename replaces the whole file; a failed write remains pending for retry.
    pub fn flush(&mut self) -> anyhow::Result<()> {
        if matches!(self.on_disk, OnDisk::Current) {
            return Ok(());
        }
        self.last_write = Instant::now();
        let text = ron::ser::to_string_pretty(&self.value, ron::ser::PrettyConfig::new())?;
        let parent = self
            .path
            .parent()
            .expect("settings files have a parent directory");
        fs::create_dir_all(parent)
            .with_context(|| format!("Could not create {}", parent.display()))?;
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, text)
            .and_then(|()| fs::rename(&temporary, &self.path))
            .with_context(|| format!("Could not save {}", self.path.display()))?;
        self.on_disk = OnDisk::Current;
        Ok(())
    }
}

impl<T> Deref for Saved<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Config, PreampDb},
        frame_schedule::FrameRateLimit,
    };

    #[test]
    fn saves_changed_settings_atomically_and_restores_them_after_restart() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config/vu-config.ron");
        let mut config = Saved::<Config>::load(path.clone())?;
        assert_eq!(*config, Config::default());
        config.flush()?;
        assert!(!path.exists());
        config.edit().preamp_db = PreampDb::try_from(-12.0)?;
        config.edit().frame_rate_limit = FrameRateLimit::Fps55;
        config.flush_if_due(config.last_write + SAVE_INTERVAL - Duration::from_millis(1))?;
        assert!(!path.exists());
        config.flush_if_due(config.last_write + SAVE_INTERVAL)?;
        let loaded = Saved::<Config>::load(path.clone())?;
        assert_eq!(*loaded, *config);
        assert!(!path.with_extension("tmp").exists());
        config.edit().frame_rate_limit = FrameRateLimit::VSync;
        config.flush()?;
        assert_eq!(
            Saved::<Config>::load(path)?.frame_rate_limit,
            FrameRateLimit::VSync
        );
        Ok(())
    }

    #[test]
    fn refuses_invalid_settings_without_overwriting_the_file() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("vu-config.ron");
        for text in [
            "(preamp_db: 97.0)",
            "(preamp_db: NaN)",
            "(frame_rate_limit: Fps99)",
        ] {
            fs::write(&path, text)?;
            assert!(Saved::<Config>::load(path.clone()).is_err());
            assert_eq!(fs::read_to_string(&path)?, text);
        }
        let missing: Config = ron::from_str("(frame_rate_limit: Fps48)")?;
        assert_eq!(missing.preamp_db, PreampDb::default());
        Ok(())
    }

    #[test]
    fn keeps_a_failed_save_pending_until_it_can_be_written() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let parent = directory.path().join("config");
        let path = parent.join("vu-config.ron");
        let mut config = Saved::<Config>::load(path.clone())?;
        config.edit().preamp_db = PreampDb::try_from(24.0)?;
        fs::write(&parent, "not a directory")?;
        assert!(config.flush().is_err());
        fs::remove_file(&parent)?;
        config.flush()?;
        assert_eq!(Saved::<Config>::load(path)?.preamp_db.db(), 24.0);
        Ok(())
    }
}
