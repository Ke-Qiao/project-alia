//! The two supported native sources share one output frame clock. Conversation
//! stays at unity gain; a short local cue is an attenuated overlay, never a new
//! conversation owner. This is mixing only, not echo cancellation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MixerSource {
    Voice = 0,
    LocalCue = 1,
}

impl MixerSource {
    pub(crate) const ALL: [Self; 2] = [Self::Voice, Self::LocalCue];

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

pub(crate) fn mix_sample(voice: i16, cue: i16) -> i16 {
    // Integer arithmetic leaves the voice untouched when no cue is present,
    // attenuates the cue to 0.2, and limits the final mixed peak without wrap.
    let cue = (i32::from(cue) + i32::from(cue.signum()) * 2) / 5;
    (i32::from(voice) + cue).clamp(i16::MIN.into(), i16::MAX.into()) as i16
}
