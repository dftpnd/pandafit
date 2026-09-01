pub mod audio;
pub mod video;

pub fn register_builtins(reg: &mut crate::codec::CodecRegistry) {
    audio::register(reg);
    video::register(reg);
}
