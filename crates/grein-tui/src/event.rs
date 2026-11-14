#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiEvent {
    Tick,
    Tab,
    BackTab,
    Char(char),
    Backspace,
    Submit,
    Quit,
    Resize(u16, u16),
}
