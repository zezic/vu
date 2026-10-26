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
        config.edit().mode = crate::measurement::Mode::SamplePeak;
        config.edit().peak_movement = crate::meter::PeakMovement::VintageFr;
        config.edit().frame_rate_limit = FrameRateLimit::Fps55;
        config.edit().shutter_frames = crate::config::ShutterFrames::try_from(2.5)?;
        config.edit().theme = config
            .theme
            .adjusted(crate::frame_schedule::Adjustment::Increase);
        config.edit().presentation = crate::visualizer::Presentation::Visualizer;
        config.edit().visualizer_text_scale = config
            .visualizer_text_scale
            .adjusted(crate::frame_schedule::Adjustment::Increase);
        config.edit().detectors.kick_onset_min_interval = 0.05;
        config.edit().detectors.kick_kernel[0] = 0.4;
        config.edit().detectors.snare_onset_attack = 0.01;
        config.edit().detectors.drop_gap_window = 0.6;
        config.edit().detectors.drop_gap_gain = 2.0;
        config.edit().detectors.drop_gap_threshold = -0.1;
        config.edit().detectors.drop_onset_attack = 0.01;
        config.edit().detectors.drop_kernels.low[0] = 0.5;
        config.edit().detectors.flux_attack = 0.02;
        config.edit().detectors.flux_sensitivity = 2.0;
        config.edit().depth_fade = config
            .depth_fade
            .adjusted(crate::frame_schedule::Adjustment::Increase);
        config.edit().near_opacity = config
            .near_opacity
            .adjusted(crate::frame_schedule::Adjustment::Increase);
        config.edit().tuning.kick_decay = 4.5;
        config.edit().tuning.gains[crate::visualizer::Gain::SpinBand] = 2.5;
        config.edit().tuning.routing[crate::visualizer::BandComponent::Spin] = 9;
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
            "(shutter_frames: 0.0)",
            "(shutter_frames: 17.0)",
            "(shutter_frames: NaN)",
            "(visualizer_text_scale: 0.5)",
            "(visualizer_text_scale: 2.5)",
            "(visualizer_text_scale: NaN)",
            "(detectors: (flux_attack: 0.0))",
            "(detectors: (flux_sensitivity: 9.0))",
            "(detectors: (flux_average: 2.0))",
            "(detectors: (kick_sequence_every: 0))",
            "(detectors: (kick_onset_attack: 0.0))",
            "(detectors: (kick_onset_min_interval: 0.0))",
            "(detectors: (kick_kernel: [2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))",
            "(detectors: (snare_onset_sensitivity: 9.0))",
            "(detectors: (snare_onset_min_threshold: 3.0))",
            "(detectors: (snare_kernel: [2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))",
            "(detectors: (drop_gap_window: 0.0))",
            "(detectors: (drop_gap_window: 5.0))",
            "(detectors: (drop_gap_threshold: 1.5))",
            "(detectors: (drop_gap_threshold: -1.5))",
            "(detectors: (drop_gap_gain: 0.0))",
            "(detectors: (drop_gap_gain: 20.0))",
            "(detectors: (drop_onset_attack: 0.0))",
            "(detectors: (drop_onset_min_threshold: 0.3))",
            "(detectors: (drop_onset_min_interval: 0.0))",
            "(detectors: (drop_low_kernel: [2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))",
            "(detectors: (flip_stiffness: 0.0))",
            "(depth_fade: -0.1)",
            "(depth_fade: 1.1)",
            "(depth_fade: NaN)",
            "(near_opacity: 0.5)",
            "(near_opacity: 1.1)",
            "(near_opacity: NaN)",
            "(tuning: (kick_decay: -1.0))",
            "(tuning: (kick_decay: 21.0))",
            "(tuning: (inputs: (power: (attack: 0.0, release: 4.5, \
             shape: (low: 0.0, high: 1.0, curve: Linear, amount: 1.0, tracking: Manual))))))",
            "(tuning: (inputs: (power: (attack: 18.0, release: 4.5, \
             shape: (low: 0.8, high: 0.2, curve: Linear, amount: 1.0, tracking: Manual))))))",
            "(tuning: (routing: { Spin: 16 }))",
            "(tuning: (gains: { SpinBand: 20.0 }))",
            "(tuning: (style: { DeformBias: 3.0 }))",
            "(frame_rate_limit: Fps99)",
            "(mode: Unknown)",
            "(peak_movement: Unknown)",
            "(theme: \"Unknown theme\")",
        ] {
            fs::write(&path, text)?;
            assert!(Saved::<Config>::load(path.clone()).is_err());
            assert_eq!(fs::read_to_string(&path)?, text);
        }
        let missing: Config = ron::from_str("(frame_rate_limit: Fps48)")?;
        assert_eq!(missing.preamp_db, PreampDb::default());
        assert_eq!(missing.mode, crate::measurement::Mode::Vu);
        assert_eq!(missing.peak_movement, crate::meter::PeakMovement::VintageFr);
        assert_eq!(
            missing.presentation,
            crate::visualizer::Presentation::Gauges
        );
        assert_eq!(
            missing.shutter_frames,
            crate::config::ShutterFrames::default()
        );
        assert_eq!(
            missing.visualizer_text_scale,
            crate::config::VisualizerTextScale::default()
        );
        assert_eq!(
            missing.detectors,
            crate::visualizer::DetectorSettings::default()
        );
        assert_eq!(missing.depth_fade, crate::visualizer::DepthFade::default());
        assert_eq!(
            missing.near_opacity,
            crate::visualizer::NearOpacity::default()
        );
        assert_eq!(
            missing.tuning,
            crate::visualizer::VisualizerTuning::default()
        );
        Ok(())
    }

    #[test]
    fn legacy_drop_fields_load_with_the_new_defaults() -> anyhow::Result<()> {
        let legacy: Config = ron::from_str(
            "(detectors: (drop_quiet_level: 0.6, drop_quiet_time: 0.35, \
             drop_split: 6, drop_onsets: 9, drop_window: 0.12, \
             drop_gap_level: 0.2, drop_gap_time: 0.7, \
             kick_low: 0, kick_high: 4, kick_window: 0.1, \
             snare_bands: (Some(5), Some(6), Some(10), Some(12), None, None, None, None), \
             snare_window: 0.06, snare_min_onsets: 3, snare_min_power: 2.0))",
        )?;
        assert_eq!(
            legacy.detectors,
            crate::visualizer::DetectorSettings::default()
        );
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
