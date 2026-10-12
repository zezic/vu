use std::f64::consts::TAU;

const RESPONSE: f64 = TAU * 4.0;

/// A critically damped needle, integrated over the actual frame exposure.
pub(super) struct Motion {
    position: f64,
    velocity: f64,
}

pub(super) struct Exposure {
    pub minimum: f64,
    pub average: f64,
    pub maximum: f64,
}

impl Motion {
    #[cfg(test)]
    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn new() -> Self {
        Self {
            position: 0.0,
            velocity: 0.0,
        }
    }

    pub fn advance(&mut self, target: f32, dt: f32) -> Exposure {
        assert!(
            dt.is_finite() && dt >= 0.0,
            "frame duration must be finite and nonnegative"
        );
        let start = self.position;
        if dt == 0.0 {
            return Exposure {
                minimum: start,
                average: start,
                maximum: start,
            };
        }
        let target = f64::from(target.clamp(0.0, 1.0));
        let dt = f64::from(dt);
        let displacement = start - target;
        let slope = self.velocity + RESPONSE * displacement;
        let decay = (-RESPONSE * dt).exp();
        let end = target + (displacement + slope * dt) * decay;
        let velocity = (self.velocity - RESPONSE * slope * dt) * decay;
        // Integrating the differential equation gives the exposure's mean position.
        let average = target
            - (velocity - self.velocity + 2.0 * RESPONSE * (end - start))
                / (RESPONSE * RESPONSE * dt);
        let mut minimum = start.min(end).clamp(0.0, 1.0);
        let mut maximum = start.max(end).clamp(0.0, 1.0);
        if slope != 0.0 {
            let turning_time = self.velocity / (RESPONSE * slope);
            if turning_time > 0.0 && turning_time < dt {
                let turning_position = (target
                    + (displacement + slope * turning_time) * (-RESPONSE * turning_time).exp())
                .clamp(0.0, 1.0);
                minimum = minimum.min(turning_position);
                maximum = maximum.max(turning_position);
            }
        }
        self.position = end.clamp(0.0, 1.0);
        self.velocity = if end != self.position { 0.0 } else { velocity };
        Exposure {
            minimum,
            average: average.clamp(minimum, maximum),
            maximum,
        }
    }
}
