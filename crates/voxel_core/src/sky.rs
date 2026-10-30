//! Jour et nuit (#30, design doc §3.9 et §6). L'heure se calcule depuis
//! l'horloge du monde ; le ciel et les lumières de chaque moment de la
//! journée sont de la donnée (`assets/content/sky.ron`, copiée dans la
//! save), interpolée entre deux moments.

use serde::Deserialize;

use crate::registry::parse;
use crate::world::TICKS_PER_SECOND;

/// Inclinaison de la course du soleil : à midi, il est à 60° au-dessus de
/// l'horizon, les ombres ne tombent jamais droit.
const SUN_TILT: f32 = std::f32::consts::FRAC_PI_6;

/// Un moment de la journée. Couleurs en sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Moment {
    /// Heure, dans [0, 24).
    pub hour: f32,
    /// Ciel au-dessus de la tête.
    pub zenith: [f32; 3],
    /// Ciel à l'horizon ; c'est aussi la couleur du brouillard.
    pub horizon: [f32; 3],
    /// Couleur de la lumière du soleil, de la lune la nuit.
    pub light_color: [f32; 3],
    /// Sa force, en lux (Bevy : 10 000 = plein jour).
    pub light: f32,
    /// Lumière ambiante, en cd/m² (Bevy : 80 par défaut).
    pub ambient: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct Settings {
    day_length: f32,
    start_hour: f32,
    moments: Vec<Moment>,
}

#[derive(Debug, Clone)]
pub struct Sky {
    /// Durée d'une journée de 24 h, en secondes.
    pub day_length: f32,
    /// Heure au tick 0.
    pub start_hour: f32,
    /// Heures croissantes, au moins un.
    pub moments: Vec<Moment>,
    /// La plus forte lumière de la journée : sert à [`Sky::daylight`].
    max_light: f32,
}

impl Sky {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let s: Settings = parse(text).map_err(|e| e.to_string())?;
        if s.day_length.is_nan() || s.day_length <= 0.0 {
            return Err("sky : day_length doit être positive".into());
        }
        if !(0.0..24.0).contains(&s.start_hour) {
            return Err("sky : start_hour dans [0, 24)".into());
        }
        if s.moments.is_empty() {
            return Err("sky : au moins un moment".into());
        }
        let mut previous = -1.0;
        for m in &s.moments {
            if !(0.0..24.0).contains(&m.hour) || m.hour <= previous {
                return Err(format!("sky : heures croissantes dans [0, 24), {} ne l'est pas", m.hour));
            }
            previous = m.hour;
            if !(m.light >= 0.0 && m.ambient >= 0.0) {
                return Err(format!("sky : lumières positives à {} h", m.hour));
            }
        }
        let max_light = s.moments.iter().map(|m| m.light).fold(0.0, f32::max);
        Ok(Self { day_length: s.day_length, start_hour: s.start_hour, moments: s.moments, max_light })
    }

    /// L'heure, dans [0, 24), au tick `tick` de l'horloge du monde.
    pub fn hour(&self, tick: u64) -> f32 {
        let seconds = tick as f64 / f64::from(TICKS_PER_SECOND);
        let days = f64::from(self.start_hour) / 24.0 + seconds / f64::from(self.day_length);
        (days.fract() * 24.0) as f32
    }

    /// Le ciel et les lumières à l'heure `hour`, interpolés entre les deux
    /// moments qui l'entourent (par-dessus minuit si besoin).
    pub fn at(&self, hour: f32) -> Moment {
        let hour = hour.rem_euclid(24.0);
        let n = self.moments.len();
        // Le dernier moment avant `hour` ; avant le premier, celui de la veille.
        let i = self.moments.iter().rposition(|m| m.hour <= hour).unwrap_or(n - 1);
        let (a, b) = (self.moments[i], self.moments[(i + 1) % n]);
        let span = (b.hour - a.hour).rem_euclid(24.0);
        if span == 0.0 {
            return a;
        }
        let t = (hour - a.hour).rem_euclid(24.0) / span;
        let mix3 = |x: [f32; 3], y: [f32; 3]| std::array::from_fn(|k| x[k] + (y[k] - x[k]) * t);
        let mix = |x: f32, y: f32| x + (y - x) * t;
        Moment {
            hour,
            zenith: mix3(a.zenith, b.zenith),
            horizon: mix3(a.horizon, b.horizon),
            light_color: mix3(a.light_color, b.light_color),
            light: mix(a.light, b.light),
            ambient: mix(a.ambient, b.ambient),
        }
    }

    /// Force du jour dans [0, 1] : la lumière du moment rapportée à la plus
    /// forte de la journée. La lumière des blocs ne compte que là où le ciel
    /// n'éclaire pas déjà autant (`voxel.wgsl`).
    pub fn daylight(&self, moment: &Moment) -> f32 {
        if self.max_light > 0.0 { (moment.light / self.max_light).clamp(0.0, 1.0) } else { 0.0 }
    }
}

/// Direction vers le soleil à l'heure `hour` : il se lève à l'est (+X) à
/// 6 h, se couche à l'ouest à 18 h.
pub fn sun_direction(hour: f32) -> [f32; 3] {
    let angle = (hour - 6.0) / 24.0 * std::f32::consts::TAU;
    let (up, east) = angle.sin_cos();
    [east, up * SUN_TILT.cos(), up * SUN_TILT.sin()]
}

/// Direction vers la lumière du ciel : le soleil de 6 h à 18 h, la lune (à
/// l'opposé) le reste du temps. Toujours au-dessus de l'horizon.
pub fn light_direction(hour: f32) -> [f32; 3] {
    let sun = sun_direction(hour);
    if sun[1] >= 0.0 { sun } else { sun.map(|c| -c) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(hour: f32, light: f32) -> String {
        format!(
            "(hour: {hour}, zenith: ({light}, 0, 0), horizon: (0, 0, 0), light_color: (1, 1, 1), light: {light}, ambient: 1)"
        )
    }

    fn sky(moments: &[String]) -> Result<Sky, String> {
        Sky::from_ron(&format!("(day_length: 1200, start_hour: 8, moments: [{}])", moments.join(",")))
    }

    #[test]
    fn hour_follows_the_world_clock() {
        let sky = sky(&[moment(12.0, 1.0)]).unwrap();
        let ticks_per_day = 1200 * u64::from(TICKS_PER_SECOND);
        assert_eq!(sky.hour(0), 8.0);
        assert!((sky.hour(ticks_per_day / 2) - 20.0).abs() < 1e-4);
        assert!((sky.hour(ticks_per_day) - 8.0).abs() < 1e-4);
        // Après mille jours, toujours la même heure.
        assert!((sky.hour(1000 * ticks_per_day + ticks_per_day / 4) - 14.0).abs() < 1e-3);
    }

    #[test]
    fn moments_are_mixed_across_midnight() {
        let sky = sky(&[moment(6.0, 100.0), moment(18.0, 300.0), moment(22.0, 500.0)]).unwrap();
        assert_eq!(sky.at(6.0).light, 100.0);
        assert_eq!(sky.at(12.0).light, 200.0);
        assert_eq!(sky.at(12.0).zenith[0], 200.0);
        // 22 h → 6 h passe par minuit : 8 h de 500 à 100.
        assert_eq!(sky.at(2.0).light, 300.0);
        assert_eq!(sky.at(23.0).light, 450.0);
        assert_eq!(sky.daylight(&sky.at(12.0)), 0.4);
    }

    #[test]
    fn a_single_moment_is_the_whole_day() {
        let sky = sky(&[moment(12.0, 7.0)]).unwrap();
        assert_eq!(sky.at(3.0).light, 7.0);
    }

    #[test]
    fn bad_settings_are_refused() {
        assert!(sky(&[]).is_err());
        assert!(sky(&[moment(12.0, 1.0), moment(6.0, 1.0)]).is_err());
        assert!(sky(&[moment(24.0, 1.0)]).is_err());
        assert!(sky(&[moment(12.0, -1.0)]).is_err());
        assert!(Sky::from_ron("(day_length: 0, start_hour: 8, moments: [])").is_err());
    }

    #[test]
    fn light_is_the_sun_by_day_and_the_moon_by_night() {
        for h in 0..96 {
            let hour = h as f32 / 4.0;
            assert!(light_direction(hour)[1] >= 0.0, "{hour} h : lumière sous l'horizon");
        }
        assert!(sun_direction(12.0)[1] > 0.8);
        // À minuit, la lune est là où le soleil est à midi.
        let (moon, noon) = (light_direction(0.0), sun_direction(12.0));
        assert!((0..3).all(|k| (moon[k] - noon[k]).abs() < 1e-5));
    }

    #[test]
    fn shipped_sky_is_valid() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/sky.ron");
        Sky::from_ron(&std::fs::read_to_string(path).unwrap()).unwrap();
    }
}
