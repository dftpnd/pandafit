pub mod audio;

pub fn register_builtins(reg: &mut crate::codec::CodecRegistry) {
    audio::register(reg);
}
