use crate::media::{Track, TrackKind};
use crate::note::Note;
use crate::plan::Opts;
use crate::Estimated;
use std::collections::BTreeMap;

pub struct EncodeCtx<'a> {
    pub src: &'a Track,
    pub opts: &'a Opts,
    /// Длительность результата после обрезки.
    pub duration_s: f64,
}

pub trait CodecProfile: Send + Sync {
    fn id(&self) -> &'static str;
    fn kind(&self) -> TrackKind;
    /// Подпись для выпадающего списка.
    fn label(&self) -> &'static str;
    /// Разумные значения по умолчанию при выборе этого профиля.
    fn default_opts(&self, src: &Track) -> Opts;
    fn estimate_bps(&self, ctx: &EncodeCtx) -> Estimated<u64>;
    /// Аргументы ffmpeg для выходного потока с номером `out_idx` внутри своего типа.
    fn args(&self, ctx: &EncodeCtx, out_idx: usize) -> Vec<String>;
    fn notes(&self, ctx: &EncodeCtx) -> Vec<Note>;
    /// Сохранит ли профиль слой Dolby Vision. По умолчанию — нет.
    fn preserves_dolby_vision(&self) -> bool {
        false
    }
}

#[derive(Default)]
pub struct CodecRegistry {
    profiles: BTreeMap<&'static str, Box<dyn CodecProfile>>,
}

impl CodecRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, profile: Box<dyn CodecProfile>) {
        self.profiles.insert(profile.id(), profile);
    }

    pub fn get(&self, id: &str) -> Option<&dyn CodecProfile> {
        self.profiles.get(id).map(|b| b.as_ref())
    }

    pub fn for_kind(&self, kind: TrackKind) -> Vec<&dyn CodecProfile> {
        self.profiles
            .values()
            .filter(|p| p.kind() == kind)
            .map(|b| b.as_ref())
            .collect()
    }

    pub fn with_builtins() -> Self {
        let mut reg = Self::new();
        crate::codecs::register_builtins(&mut reg);
        reg
    }
}
