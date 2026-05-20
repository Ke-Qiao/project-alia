use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct NeckMapper {
    servo_minmax: Vec<[f32; 2]>,
    neck_angles: Vec<[f32; 3]>,
    sigma: f32,
    matrix: Vec<Vec<f32>>,
    matrix_size: [usize; 2],
    neck_angle_minmax: [f32; 6],
    prev_rot: [f32; 3],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NeckMatrixConfig {
    servo_min_max: BTreeMap<String, Vec<f32>>,
    neck_angles: Vec<Vec<f32>>,
    matrix_size: Vec<usize>,
    #[serde(rename = "sigma")]
    sigma: f32,
    matrix: Vec<Vec<f32>>,
    neck_angle_min_max: Vec<f32>,
}

impl NeckMapper {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading neck matrix config {}", path.display()))?;
        Self::from_json(&content)
    }

    pub fn from_json(content: &str) -> Result<Self> {
        let config: NeckMatrixConfig =
            serde_json::from_str(content).context("parsing neck matrix config")?;
        if config.sigma <= f32::EPSILON {
            bail!("neck matrix sigma must be positive");
        }
        if config.matrix_size.len() != 2 {
            bail!("MatrixSize must contain [rows, cols]");
        }
        if config.neck_angle_min_max.len() != 6 {
            bail!("NeckAngleMinMax must contain [x_min, x_max, y_min, y_max, z_min, z_max]");
        }

        let rows = config.matrix_size[0];
        let cols = config.matrix_size[1];
        if config.neck_angles.len() != rows {
            bail!(
                "NeckAngles row count {} does not match MatrixSize rows {}",
                config.neck_angles.len(),
                rows
            );
        }
        if config.matrix.len() != rows {
            bail!(
                "Matrix row count {} does not match MatrixSize rows {}",
                config.matrix.len(),
                rows
            );
        }
        if config.servo_min_max.len() < cols {
            bail!(
                "ServoMinMax entry count {} is less than MatrixSize cols {}",
                config.servo_min_max.len(),
                cols
            );
        }

        let mut servo_minmax = Vec::with_capacity(cols);
        for i in 0..cols {
            let key = format!("servo_{i}");
            let pair = config
                .servo_min_max
                .get(&key)
                .with_context(|| format!("missing ServoMinMax.{key}"))?;
            if pair.len() < 2 {
                bail!("ServoMinMax.{key} must contain [min, max]");
            }
            let min = pair[0].min(pair[1]);
            let max = pair[0].max(pair[1]);
            servo_minmax.push([min, max]);
        }

        let neck_angles = config
            .neck_angles
            .into_iter()
            .enumerate()
            .map(|(index, values)| {
                if values.len() != 3 {
                    bail!("NeckAngles[{index}] must contain [x, y, z]");
                }
                Ok([values[0], values[1], values[2]])
            })
            .collect::<Result<Vec<_>>>()?;

        for (index, row) in config.matrix.iter().enumerate() {
            if row.len() != cols {
                bail!(
                    "Matrix[{index}] length {} does not match cols {cols}",
                    row.len()
                );
            }
        }

        Ok(Self {
            servo_minmax,
            neck_angles,
            sigma: config.sigma,
            matrix: config.matrix,
            matrix_size: [rows, cols],
            neck_angle_minmax: [
                config.neck_angle_min_max[0],
                config.neck_angle_min_max[1],
                config.neck_angle_min_max[2],
                config.neck_angle_min_max[3],
                config.neck_angle_min_max[4],
                config.neck_angle_min_max[5],
            ],
            prev_rot: [0.0, 0.0, 0.0],
        })
    }

    pub fn servo_count(&self) -> usize {
        self.matrix_size[1]
    }

    pub fn map(&mut self, rot: [f32; 3], apply_smoothing: bool) -> Vec<f32> {
        let mut rot = self.clamp_rotation(rot);
        if apply_smoothing {
            for (prev, value) in self.prev_rot.iter_mut().zip(rot) {
                *prev = *prev * 0.7 + value * 0.3;
            }
            rot = self.prev_rot;
        }

        let mut basis = Vec::with_capacity(self.neck_angles.len());
        for angle in &self.neck_angles {
            basis.push(self.gaussian_rbf(rot, *angle));
        }

        let cols = self.matrix_size[1];
        let mut out = vec![0.0; cols];
        for (row_index, weight) in basis.iter().enumerate() {
            for (col, value) in out.iter_mut().enumerate().take(cols) {
                *value += weight * self.matrix[row_index][col];
            }
        }

        for (value, [min, max]) in out.iter_mut().zip(&self.servo_minmax) {
            *value = value.clamp(*min, *max);
        }
        out
    }

    fn clamp_rotation(&self, rot: [f32; 3]) -> [f32; 3] {
        [
            rot[0].clamp(self.neck_angle_minmax[0], self.neck_angle_minmax[1]),
            rot[1].clamp(self.neck_angle_minmax[2], self.neck_angle_minmax[3]),
            rot[2].clamp(self.neck_angle_minmax[4], self.neck_angle_minmax[5]),
        ]
    }

    fn gaussian_rbf(&self, x: [f32; 3], y: [f32; 3]) -> f32 {
        let dx = x[0] - y[0];
        let dy = x[1] - y[1];
        let dz = x[2] - y[2];
        let dist_sq = dx * dx + dy * dy + dz * dz;
        (-dist_sq / (2.0 * self.sigma * self.sigma)).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_identity_matrix_like_python_rbf() {
        let json = r#"{
          "ServoMinMax": {"servo_0": [0, 180], "servo_1": [10, 170]},
          "NeckAngles": [[0, 0, 0], [10, 0, 0]],
          "MatrixSize": [2, 2],
          "sigma": 5.0,
          "Matrix": [[90, 90], [120, 80]],
          "NeckAngleMinMax": [-20, 20, -20, 20, -20, 20]
        }"#;
        let mut mapper = NeckMapper::from_json(json).unwrap();
        let out = mapper.map([0.0, 0.0, 0.0], false);
        assert_eq!(out.len(), 2);
        assert!(out[0] > 90.0);
        assert!(out[1] > 80.0);
    }
}
