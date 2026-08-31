// Позволяет обращаться к собственным типам по имени крейта. Нужно для общей
// тестовой фикстуры, которая одним и тем же текстом включается и сюда, и в crates/app.
extern crate self as pandafit_core;

pub mod estimated;
pub mod media;
pub mod note;
pub mod plan;
pub mod codec;
pub mod codecs;
pub mod profile;
pub mod estimate;

pub use estimated::{Confidence, Estimated};
pub use media::{ColorInfo, MediaInfo, Track, TrackKind};
pub use note::{Level, Note, NoteTarget};
pub use plan::{Opts, Plan, Target, TargetSource, TimeRange, TrackAction, PRESETS};
pub use codec::{CodecProfile, CodecRegistry, EncodeCtx};
pub use profile::{BitrateProfile, Sample};
pub use estimate::{estimate, SizeBreakdown, TrackSize, Verdict};
