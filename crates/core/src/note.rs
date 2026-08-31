#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Справка: учит, ни на что не жалуется.
    Info,
    /// Последствие уже принятого решения.
    Warning,
    /// Запрет: гасит кнопку сборки.
    Blocker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteTarget {
    Whole,
    Track(usize),
    Trim,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub level: Level,
    pub target: NoteTarget,
    pub text: String,
}

impl Note {
    pub fn info(target: NoteTarget, text: impl Into<String>) -> Self {
        Self { level: Level::Info, target, text: text.into() }
    }
    pub fn warning(target: NoteTarget, text: impl Into<String>) -> Self {
        Self { level: Level::Warning, target, text: text.into() }
    }
    pub fn blocker(target: NoteTarget, text: impl Into<String>) -> Self {
        Self { level: Level::Blocker, target, text: text.into() }
    }
}
