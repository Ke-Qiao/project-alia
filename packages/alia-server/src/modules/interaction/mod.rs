pub mod initiative;
pub mod turn_detector;
pub mod voice_input;

use crate::config::EasyTurnSection;
use std::sync::Arc;
use turn_detector::TurnDetector;

pub fn build_turn_detector(
    config: &EasyTurnSection,
) -> anyhow::Result<Option<Arc<dyn TurnDetector>>> {
    if !config.enabled {
        return Ok(None);
    }
    #[cfg(feature = "turn-easy")]
    {
        Ok(Some(Arc::new(
            crate::adapters::easy_turn::EasyTurnDetector::new(config)?,
        )))
    }
    #[cfg(not(feature = "turn-easy"))]
    anyhow::bail!("Easy-Turn is configured but this binary was built without the turn-easy feature")
}
