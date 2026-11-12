//! InvisBrowser chrome UI kit. Owned by the CHROME track — see `docs/agents/CHROME.md` for the contract.
//!
//! Milestones 1-2: theme, tab strip, toolbar, address-bar text field, progress line, input routing
//! and keyboard shortcuts. The chrome never opens windows or touches the GPU: it fills a
//! [`draw::DrawList`] and returns [`Action`]s.

pub use invisium_draw as draw;

mod chrome;
mod field;
mod theme;
pub mod pages;

pub use chrome::Chrome;
pub use theme::Theme;

#[derive(Clone, Debug, PartialEq)]
pub enum Security {
    Internal,
    Secure,
    Insecure,
    Tor,
    Onion,
}

#[derive(Clone, Debug)]
pub struct TabInfo {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub security: Security,
    pub blocked: u32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum TorIndicator {
    #[default]
    Off,
    Connecting(u8),
    On,
    Failed,
}

/// A switch shown in the privacy panel.
#[derive(Clone, Debug)]
pub struct Toggle {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub on: bool,
    pub section: String,
    pub enabled: bool,
}

/// Everything the chrome displays that it does not own.
#[derive(Clone, Debug, Default)]
pub struct ChromeState {
    pub tabs: Vec<TabInfo>,
    pub active: usize,
    pub can_back: bool,
    pub can_forward: bool,
    pub tor: TorIndicator,
    pub toggles: Vec<Toggle>,
    pub status: Option<String>,
    pub progress: Option<f32>,
    pub zoom_percent: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    NewTab,
    CloseTab(u64),
    SelectTab(u64),
    MoveTab { id: u64, to: usize },
    Navigate(String),
    Back,
    Forward,
    Reload,
    Stop,
    Toggle(String),
    OpenInternal(&'static str),
    NewIdentity,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    FocusPage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F5,
    F6,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    MouseMove { x: f32, y: f32 },
    MouseDown { x: f32, y: f32, button: MouseButton, clicks: u8 },
    MouseUp { x: f32, y: f32, button: MouseButton },
    MouseLeave,
    Wheel { x: f32, y: f32, dy: f32 },
    Key { key: Key, mods: Mods },
    Text(String),
    WindowFocus(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Default,
    Pointer,
    Text,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub actions: Vec<Action>,
    pub consumed: bool,
    pub redraw: bool,
    pub cursor: Option<CursorKind>,
}

pub trait Clipboard {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, text: &str);
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_m2;
